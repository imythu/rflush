use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use icu_casemap::CaseMapper;
use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

const DIRECTORY_VISIBILITY_ATTEMPTS: usize = 20;
const DIRECTORY_VISIBILITY_DELAY: Duration = Duration::from_millis(250);

pub(crate) fn openlist_canonical_key(value: &str) -> String {
    value.nfc().collect()
}

pub(crate) fn openlist_identity_key(value: &str) -> String {
    let canonical = openlist_canonical_key(value);
    CaseMapper::new()
        .fold_string(&canonical)
        .as_ref()
        .nfc()
        .collect()
}

#[derive(Clone)]
pub struct OpenListClient {
    base_url: String,
    api_key: String,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    code: i64,
    message: String,
    data: Option<T>,
}

#[derive(Debug)]
enum OpenListRequestError {
    Transport(String),
    Http { status: StatusCode, body: String },
    Decode(String),
    Business { code: i64, message: String },
    MissingData,
}

#[derive(Debug)]
enum ObjectLookupError {
    Request(OpenListRequestError),
    Conflict(String),
}

impl fmt::Display for OpenListRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "请求 OpenList 失败: {error}"),
            Self::Http { status, body } => {
                write!(formatter, "OpenList HTTP 错误 {status}: {body}")
            }
            Self::Decode(error) => write!(formatter, "解析 OpenList 响应失败: {error}"),
            Self::Business { code, message } => {
                write!(formatter, "OpenList 业务错误 {code}: {message}")
            }
            Self::MissingData => formatter.write_str("OpenList 成功响应缺少 data"),
        }
    }
}

impl fmt::Display for ObjectLookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(error) => error.fmt(formatter),
            Self::Conflict(message) => formatter.write_str(message),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct OpenListTask {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub state: i32,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub total_bytes: i64,
    #[serde(default)]
    pub error: String,
}

impl OpenListTask {
    pub fn succeeded(&self) -> bool {
        self.state == 2
    }

    pub fn terminal_failure(&self) -> bool {
        matches!(self.state, 4 | 7)
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct OpenListObject {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub is_dir: bool,
    #[serde(default)]
    pub hashinfo: Option<String>,
    #[serde(default)]
    pub hash_info: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestFileState {
    Missing,
    MissingDirectory { path: String },
    Present { hash_verified: bool },
}

#[derive(Debug)]
enum ResolvedDirectory {
    Present(String),
    Missing(String),
}

#[derive(Debug)]
struct ResolvedManifestInspection {
    state: ManifestFileState,
    source_directory: String,
    source_name: String,
    target_directory: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestInspectError {
    Transient(String),
    Conflict(String),
}

impl fmt::Display for ManifestInspectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transient(message) | Self::Conflict(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ManifestInspectError {}

impl ManifestInspectError {
    fn from_lookup(error: ObjectLookupError) -> Self {
        match error {
            ObjectLookupError::Request(error) => Self::from_request(error),
            ObjectLookupError::Conflict(message) => Self::Conflict(message),
        }
    }

    fn from_request(error: OpenListRequestError) -> Self {
        let transient = match &error {
            OpenListRequestError::Transport(_) => true,
            OpenListRequestError::Http { status, .. } => {
                status.is_server_error()
                    || matches!(
                        *status,
                        StatusCode::REQUEST_TIMEOUT
                            | StatusCode::TOO_EARLY
                            | StatusCode::TOO_MANY_REQUESTS
                    )
            }
            OpenListRequestError::Decode(_) | OpenListRequestError::MissingData => true,
            OpenListRequestError::Business { code, message } => {
                matches!(*code, 502 | 503 | 504) || is_temporary_business_error(message)
            }
        };
        let message = error.to_string();
        if transient {
            Self::Transient(message)
        } else {
            Self::Conflict(message)
        }
    }
}

#[derive(Debug, Deserialize)]
struct CopyResult {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    tasks: Vec<OpenListTask>,
}

#[derive(Serialize)]
struct CopyRequest<'a> {
    src_dir: &'a str,
    dst_dir: &'a str,
    names: &'a [&'a str],
    overwrite: bool,
    skip_existing: bool,
    merge: bool,
}

#[derive(Serialize)]
struct PathRequest<'a> {
    path: &'a str,
    password: &'a str,
}

#[derive(Serialize)]
struct MkdirRequest<'a> {
    path: &'a str,
}

#[derive(Serialize)]
struct ListRequest<'a> {
    path: &'a str,
    password: &'a str,
    page: usize,
    per_page: usize,
    refresh: bool,
}

#[derive(Deserialize)]
struct ListResult {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    content: Vec<OpenListObject>,
    #[serde(default)]
    total: usize,
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

#[derive(Serialize)]
struct RemoveRequest<'a> {
    dir: &'a str,
    names: &'a [&'a str],
}

impl OpenListClient {
    pub fn new(base_url: &str, api_key: &str) -> Result<Self, String> {
        let base_url = base_url.trim().trim_end_matches('/');
        if base_url.is_empty() {
            return Err("OpenList 地址不能为空".to_string());
        }
        if api_key.trim().is_empty() {
            return Err("OpenList API Key 不能为空".to_string());
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("构建 OpenList HTTP 客户端失败: {e}"))?;
        Ok(Self {
            base_url: base_url.to_string(),
            api_key: api_key.trim().to_string(),
            client,
        })
    }

    pub async fn copy(
        &self,
        src_dir: &str,
        dst_dir: &str,
        name: &str,
    ) -> Result<Vec<OpenListTask>, String> {
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            return Err("OpenList 复制名称必须是单个文件或目录名".to_string());
        }
        let names = [name];
        // OpenList returns `data: null` when the operation is completed
        // synchronously (for example when no background task is needed).
        // That is a successful copy, not a malformed response.
        let data: Option<CopyResult> = self
            .post_envelope_structured(
                "/api/fs/copy",
                &CopyRequest {
                    src_dir,
                    dst_dir,
                    names: &names,
                    overwrite: false,
                    skip_existing: true,
                    merge: false,
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(data.map(|result| result.tasks).unwrap_or_default())
    }

    #[allow(dead_code)]
    pub async fn task_info(&self, task_id: &str) -> Result<OpenListTask, String> {
        let path = format!("/api/task/copy/info?tid={}", urlencoding::encode(task_id));
        self.post(&path, &()).await
    }

    pub async fn task_info_if_exists(&self, task_id: &str) -> Result<Option<OpenListTask>, String> {
        let path = format!("/api/task/copy/info?tid={}", urlencoding::encode(task_id));
        match self.post_structured(&path, &()).await {
            Ok(task) => Ok(Some(task)),
            Err(OpenListRequestError::Business { code: 404, message })
                if message == "task not found" =>
            {
                Ok(None)
            }
            Err(error) => Err(error.to_string()),
        }
    }

    pub async fn stat_if_exists(&self, path: &str) -> Result<Option<OpenListObject>, String> {
        match self
            .post_structured("/api/fs/get", &PathRequest { path, password: "" })
            .await
        {
            Ok(object) => Ok(Some(object)),
            Err(OpenListRequestError::Business { code, message })
                if is_object_not_found_business_error(code, &message) =>
            {
                Ok(None)
            }
            Err(error) => Err(error.to_string()),
        }
    }

    pub async fn create_directory_if_missing(&self, path: &str) -> Result<(), String> {
        let components = absolute_remote_directory_components(path)?;
        let (name, parent_components) = components
            .split_last()
            .ok_or_else(|| format!("OpenList 目录路径无效: {path:?}"))?;
        let parent = match self
            .resolve_directory_components(parent_components, "待创建目录的父级")
            .await
            .map_err(|error| error.to_string())?
        {
            ResolvedDirectory::Present(parent) => parent,
            ResolvedDirectory::Missing(missing) => {
                return Err(format!("OpenList 待创建目录的父级不存在: {missing}"));
            }
        };
        let resolved_path = remote_child_path(&parent, name)?;
        match self.refreshed_object_if_exists(&resolved_path).await? {
            Some(object) if object.is_dir => Ok(()),
            Some(_) => Err(format!(
                "OpenList 目标路径应为目录但实际是文件: {resolved_path}"
            )),
            None => {
                let create_result = self
                    .post_empty(
                        "/api/fs/mkdir",
                        &MkdirRequest {
                            path: &resolved_path,
                        },
                    )
                    .await;
                for attempt in 0..DIRECTORY_VISIBILITY_ATTEMPTS {
                    match self.refreshed_object_if_exists(&resolved_path).await? {
                        Some(object) if object.is_dir => return Ok(()),
                        Some(_) => {
                            return Err(format!(
                                "OpenList 目标路径应为目录但实际是文件: {resolved_path}"
                            ));
                        }
                        None if attempt + 1 < DIRECTORY_VISIBILITY_ATTEMPTS => {
                            tokio::time::sleep(DIRECTORY_VISIBILITY_DELAY).await;
                        }
                        None => {}
                    }
                }
                let reason = create_result
                    .err()
                    .unwrap_or_else(|| format!("创建目录后长时间不可见: {resolved_path}"));
                Err(format!(
                    "OpenList 目录创建未确认，已停止自动重试: {resolved_path}: {reason}"
                ))
            }
        }
    }

    async fn refreshed_object_if_exists(
        &self,
        path: &str,
    ) -> Result<Option<OpenListObject>, String> {
        self.refreshed_object_if_exists_structured(path)
            .await
            .map_err(|error| error.to_string())
    }

    async fn refreshed_object_if_exists_structured(
        &self,
        path: &str,
    ) -> Result<Option<OpenListObject>, ObjectLookupError> {
        let (parent, name) = remote_parent_and_name(path).map_err(ObjectLookupError::Conflict)?;
        const PAGE_SIZE: usize = 200;
        let mut page = 1;
        let mut exact_match = None;
        let mut case_conflict = None;
        let canonical_name = openlist_canonical_key(&name);
        let folded_name = openlist_identity_key(&name);
        loop {
            let result: ListResult = self
                .post_structured(
                    "/api/fs/list",
                    &ListRequest {
                        path: &parent,
                        password: "",
                        page,
                        per_page: PAGE_SIZE,
                        refresh: page == 1,
                    },
                )
                .await
                .map_err(ObjectLookupError::Request)?;
            for object in &result.content {
                let object_canonical = openlist_canonical_key(&object.name);
                if object.name == name || object_canonical == canonical_name {
                    if exact_match
                        .as_ref()
                        .is_some_and(|matched: &OpenListObject| matched.name != object.name)
                    {
                        case_conflict.get_or_insert_with(|| object.name.clone());
                    } else {
                        exact_match.get_or_insert_with(|| object.clone());
                    }
                } else if case_conflict.is_none()
                    && openlist_identity_key(&object.name) == folded_name
                {
                    case_conflict = Some(object.name.clone());
                }
            }
            let seen = (page - 1)
                .saturating_mul(PAGE_SIZE)
                .saturating_add(result.content.len());
            if result.content.is_empty() || seen >= result.total {
                return match case_conflict {
                    Some(conflict) => Err(ObjectLookupError::Conflict(format!(
                        "OpenList 路径存在仅大小写或 Unicode 规范形式不同的同名对象，已拒绝操作: {path} 与 {conflict:?}"
                    ))),
                    None => Ok(exact_match),
                };
            }
            page += 1;
        }
    }

    async fn resolve_directory_components(
        &self,
        components: &[&str],
        side: &str,
    ) -> Result<ResolvedDirectory, ManifestInspectError> {
        let mut resolved = "/".to_string();
        for component in components {
            let candidate =
                remote_child_path(&resolved, component).map_err(ManifestInspectError::Conflict)?;
            match self
                .refreshed_object_if_exists_structured(&candidate)
                .await
                .map_err(ManifestInspectError::from_lookup)?
            {
                Some(object) if object.is_dir => {
                    resolved = remote_child_path(&resolved, &object.name)
                        .map_err(ManifestInspectError::Conflict)?;
                }
                Some(_) => {
                    return Err(ManifestInspectError::Conflict(format!(
                        "OpenList {side}路径应为目录但实际是文件: {candidate}"
                    )));
                }
                None => return Ok(ResolvedDirectory::Missing(candidate)),
            }
        }
        Ok(ResolvedDirectory::Present(resolved))
    }

    pub async fn inspect_manifest_file(
        &self,
        src_root: &str,
        dst_root: &str,
        relative_path: &str,
        expected_size: i64,
    ) -> Result<ManifestFileState, ManifestInspectError> {
        self.inspect_manifest_file_resolved(src_root, dst_root, relative_path, expected_size)
            .await
            .map(|inspection| inspection.state)
    }

    async fn inspect_manifest_file_resolved(
        &self,
        src_root: &str,
        dst_root: &str,
        relative_path: &str,
        expected_size: i64,
    ) -> Result<ResolvedManifestInspection, ManifestInspectError> {
        if expected_size < 0 {
            return Err(ManifestInspectError::Conflict(format!(
                "OpenList 文件大小无效: {relative_path}={expected_size}"
            )));
        }
        let components =
            manifest_components(relative_path).map_err(ManifestInspectError::Conflict)?;
        let (file_name, directories) = components.split_last().ok_or_else(|| {
            ManifestInspectError::Conflict("OpenList manifest 文件路径为空".to_string())
        })?;

        let mut source_components = absolute_remote_directory_components(src_root)
            .map_err(ManifestInspectError::Conflict)?;
        source_components.extend_from_slice(directories);
        let source_dir = match self
            .resolve_directory_components(&source_components, "源 manifest")
            .await?
        {
            ResolvedDirectory::Present(path) => path,
            ResolvedDirectory::Missing(path) => {
                return Err(ManifestInspectError::Conflict(format!(
                    "OpenList 源 manifest 目录不存在: {path}"
                )));
            }
        };
        let source_path =
            remote_child_path(&source_dir, file_name).map_err(ManifestInspectError::Conflict)?;
        let source = self
            .refreshed_object_if_exists_structured(&source_path)
            .await
            .map_err(ManifestInspectError::from_lookup)?
            .ok_or_else(|| {
                ManifestInspectError::Conflict(format!(
                    "OpenList 源 manifest 文件不存在: {source_path}"
                ))
            })?;
        validate_manifest_file(&source, file_name, expected_size, "源", &source_path)
            .map_err(ManifestInspectError::Conflict)?;
        let source_name = source.name.clone();

        let mut target_components = absolute_remote_directory_components(dst_root)
            .map_err(ManifestInspectError::Conflict)?;
        target_components.extend_from_slice(directories);
        let target_dir = match self
            .resolve_directory_components(&target_components, "目标 manifest")
            .await?
        {
            ResolvedDirectory::Present(path) => path,
            ResolvedDirectory::Missing(path) => {
                return Ok(ResolvedManifestInspection {
                    state: ManifestFileState::MissingDirectory { path },
                    source_directory: source_dir,
                    source_name,
                    target_directory: String::new(),
                });
            }
        };

        let target_path =
            remote_child_path(&target_dir, file_name).map_err(ManifestInspectError::Conflict)?;
        match self
            .refreshed_object_if_exists_structured(&target_path)
            .await
            .map_err(ManifestInspectError::from_lookup)?
        {
            None => Ok(ResolvedManifestInspection {
                state: ManifestFileState::Missing,
                source_directory: source_dir,
                source_name,
                target_directory: target_dir,
            }),
            Some(target) => {
                validate_manifest_file(&target, file_name, expected_size, "目标", &target_path)
                    .map_err(ManifestInspectError::Conflict)?;
                let hash_verified = validate_common_manifest_hashes(&source, &target, &target_path)
                    .map_err(ManifestInspectError::Conflict)?;
                Ok(ResolvedManifestInspection {
                    state: ManifestFileState::Present { hash_verified },
                    source_directory: source_dir,
                    source_name,
                    target_directory: target_dir,
                })
            }
        }
    }

    pub async fn copy_manifest_file(
        &self,
        src_root: &str,
        dst_root: &str,
        relative_path: &str,
        expected_size: i64,
    ) -> Result<Vec<OpenListTask>, String> {
        let inspection = self
            .inspect_manifest_file_resolved(src_root, dst_root, relative_path, expected_size)
            .await
            .map_err(|error| error.to_string())?;
        match inspection.state {
            ManifestFileState::Present { .. } => return Ok(Vec::new()),
            ManifestFileState::Missing => {}
            ManifestFileState::MissingDirectory { path } => {
                return Err(format!(
                    "OpenList 目标目录缺失，必须先确认创建后再复制: {path}"
                ));
            }
        }
        self.copy(
            &inspection.source_directory,
            &inspection.target_directory,
            &inspection.source_name,
        )
        .await
    }

    pub async fn remove_manifest_file_if_exists(
        &self,
        source_root: &str,
        target_root: &str,
        relative_path: &str,
        expected_size: i64,
    ) -> Result<(), String> {
        let Some((source_directory, file_name)) = self
            .validated_manifest_removal(source_root, target_root, relative_path, expected_size)
            .await?
        else {
            return Ok(());
        };
        self.remove(&source_directory, &file_name).await
    }

    pub async fn manifest_file_removal_needed(
        &self,
        source_root: &str,
        target_root: &str,
        relative_path: &str,
        expected_size: i64,
    ) -> Result<bool, String> {
        self.validated_manifest_removal(source_root, target_root, relative_path, expected_size)
            .await
            .map(|removal| removal.is_some())
    }

    async fn validated_manifest_removal(
        &self,
        source_root: &str,
        target_root: &str,
        relative_path: &str,
        expected_size: i64,
    ) -> Result<Option<(String, String)>, String> {
        let components = manifest_components(relative_path)?;
        let (file_name, directories) = components
            .split_last()
            .ok_or_else(|| "OpenList manifest 文件路径为空".to_string())?;
        let source_directory = append_remote_components(source_root, directories)?;
        let target_directory = append_remote_components(target_root, directories)?;
        let source_path = remote_child_path(&source_directory, file_name)?;
        let target_path = remote_child_path(&target_directory, file_name)?;
        if openlist_identity_key(&source_path) == openlist_identity_key(&target_path) {
            return Err(format!(
                "OpenList 源与目标文件路径相同，已拒绝删除: {source_path}"
            ));
        }

        let mut target_components = absolute_remote_directory_components(target_root)?;
        target_components.extend_from_slice(directories);
        let target_directory = match self
            .resolve_directory_components(&target_components, "已核验目标")
            .await
            .map_err(|error| error.to_string())?
        {
            ResolvedDirectory::Present(path) => path,
            ResolvedDirectory::Missing(path) => {
                return Err(format!(
                    "OpenList 已核验目标目录不存在，已拒绝删除源文件: {path}"
                ));
            }
        };
        let target_path = remote_child_path(&target_directory, file_name)?;
        let target = self
            .refreshed_object_if_exists(&target_path)
            .await?
            .ok_or_else(|| {
                format!("OpenList 已核验目标文件不存在，已拒绝删除源文件: {target_path}")
            })?;
        validate_manifest_file(
            &target,
            file_name,
            expected_size,
            "已核验目标",
            &target_path,
        )?;

        let mut source_components = absolute_remote_directory_components(source_root)?;
        source_components.extend_from_slice(directories);
        let source_directory = match self
            .resolve_directory_components(&source_components, "待删除源")
            .await
            .map_err(|error| error.to_string())?
        {
            ResolvedDirectory::Present(path) => path,
            ResolvedDirectory::Missing(_) => return Ok(None),
        };
        let source_path = remote_child_path(&source_directory, file_name)?;
        let Some(initial_source) = self.refreshed_object_if_exists(&source_path).await? else {
            return Ok(None);
        };
        validate_manifest_file(
            &initial_source,
            file_name,
            expected_size,
            "待删除源",
            &source_path,
        )?;
        let source_path = remote_child_path(&source_directory, &initial_source.name)?;

        // Cached get is identity evidence only; the refreshed parent listing is authoritative.
        let reference = self.stat_if_exists(&source_path).await?;

        let target_directory = match self
            .resolve_directory_components(&target_components, "最终核验目标")
            .await
            .map_err(|error| error.to_string())?
        {
            ResolvedDirectory::Present(path) => path,
            ResolvedDirectory::Missing(path) => {
                return Err(format!(
                    "OpenList 最终核验目标目录不存在，已拒绝删除源文件: {path}"
                ));
            }
        };
        let target_path = remote_child_path(&target_directory, file_name)?;
        let target = self
            .refreshed_object_if_exists(&target_path)
            .await?
            .ok_or_else(|| {
                format!("OpenList 最终核验目标文件不存在，已拒绝删除源文件: {target_path}")
            })?;
        validate_manifest_file(
            &target,
            file_name,
            expected_size,
            "最终核验目标",
            &target_path,
        )?;

        let source_directory = match self
            .resolve_directory_components(&source_components, "最终核验待删除源")
            .await
            .map_err(|error| error.to_string())?
        {
            ResolvedDirectory::Present(path) => path,
            ResolvedDirectory::Missing(_) => return Ok(None),
        };
        let source_candidate = remote_child_path(&source_directory, file_name)?;
        let Some(current) = self.refreshed_object_if_exists(&source_candidate).await? else {
            return Ok(None);
        };
        validate_manifest_file(
            &current,
            file_name,
            expected_size,
            "最终核验待删除源",
            &source_candidate,
        )?;
        let current_source_path = remote_child_path(&source_directory, &current.name)?;
        let reference = reference.ok_or_else(|| {
            format!("OpenList 待删除源文件缺少可核验的删除前身份快照，已拒绝删除: {source_path}")
        })?;
        validate_manifest_file(
            &reference,
            file_name,
            expected_size,
            "待删除源身份快照",
            &source_path,
        )?;
        if !validate_common_manifest_hashes(&reference, &current, &current_source_path)? {
            return Err(format!(
                "OpenList 待删除源文件缺少共同哈希，无法证明对象未被替换，已拒绝删除: {current_source_path}"
            ));
        }
        if !validate_common_manifest_hashes(&current, &target, &target_path)? {
            return Err(format!(
                "OpenList 当前源文件与已核验目标文件缺少共同哈希，已拒绝删除: {current_source_path} -> {target_path}"
            ));
        }
        Ok(Some((source_directory, current.name)))
    }

    async fn remove(&self, dir: &str, name: &str) -> Result<(), String> {
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            return Err("OpenList 删除名称必须是单个文件或目录名".to_string());
        }
        let names = [name];
        self.post_empty("/api/fs/remove", &RemoveRequest { dir, names: &names })
            .await
    }

    async fn post<B, T>(&self, path: &str, body: &B) -> Result<T, String>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        self.post_structured(path, body)
            .await
            .map_err(|error| error.to_string())
    }

    async fn post_empty<B>(&self, path: &str, body: &B) -> Result<(), String>
    where
        B: Serialize + ?Sized,
    {
        self.post_envelope_structured::<_, serde_json::Value>(path, body)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn post_structured<B, T>(&self, path: &str, body: &B) -> Result<T, OpenListRequestError>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        self.post_envelope_structured(path, body)
            .await?
            .ok_or(OpenListRequestError::MissingData)
    }

    async fn post_envelope_structured<B, T>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<Option<T>, OpenListRequestError>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let response = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header("Authorization", &self.api_key)
            .json(body)
            .send()
            .await
            .map_err(|error| OpenListRequestError::Transport(error.to_string()))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| OpenListRequestError::Transport(error.to_string()))?;
        if !status.is_success() {
            return Err(OpenListRequestError::Http {
                status,
                body: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }
        let envelope: Envelope<T> = serde_json::from_slice(&bytes)
            .map_err(|error| OpenListRequestError::Decode(error.to_string()))?;
        if envelope.code != StatusCode::OK.as_u16() as i64 {
            return Err(OpenListRequestError::Business {
                code: envelope.code,
                message: envelope.message,
            });
        }
        Ok(envelope.data)
    }
}

fn valid_child_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && Path::new(name)
            .file_name()
            .is_some_and(|value| value == name)
}

fn manifest_components(path: &str) -> Result<Vec<&str>, String> {
    if path.is_empty() || path.starts_with(['/', '\\']) || path.contains('\\') {
        return Err(format!("OpenList manifest 文件路径无效: {path:?}"));
    }
    let components = path.split('/').collect::<Vec<_>>();
    if components
        .iter()
        .any(|component| !valid_child_name(component))
    {
        return Err(format!("OpenList manifest 文件路径无效: {path:?}"));
    }
    Ok(components)
}

fn absolute_remote_directory_components(path: &str) -> Result<Vec<&str>, String> {
    if !path.starts_with('/') || path.contains('\0') || path.contains('\\') {
        return Err(format!("OpenList 目标目录路径无效: {path:?}"));
    }
    let components = path
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    if components.iter().any(|name| !valid_child_name(name)) {
        return Err(format!("OpenList 目标目录路径无效: {path:?}"));
    }
    Ok(components)
}

fn append_remote_components(root: &str, components: &[&str]) -> Result<String, String> {
    components.iter().try_fold(root.to_string(), |path, name| {
        remote_child_path(&path, name)
    })
}

fn validate_manifest_file(
    object: &OpenListObject,
    expected_name: &str,
    expected_size: i64,
    side: &str,
    path: &str,
) -> Result<(), String> {
    if object.is_dir {
        return Err(format!(
            "OpenList {side} manifest 路径应为文件但实际是目录: {path}"
        ));
    }
    if openlist_canonical_key(&object.name) != openlist_canonical_key(expected_name) {
        return Err(format!(
            "OpenList {side}文件名不匹配: {:?} != {:?}",
            object.name, expected_name
        ));
    }
    if object.size != expected_size {
        return Err(format!(
            "OpenList {side}文件大小不匹配: {path} ({} != {expected_size})",
            object.size
        ));
    }
    Ok(())
}

fn validate_common_manifest_hashes(
    source: &OpenListObject,
    target: &OpenListObject,
    path: &str,
) -> Result<bool, String> {
    let source_hashes = normalized_object_hashes(source)?;
    let target_hashes = normalized_object_hashes(target)?;
    let mut verified = false;
    for (algorithm, source_hash) in source_hashes {
        let Some(target_hash) = target_hashes.get(&algorithm) else {
            continue;
        };
        if !hash_values_equal(&source_hash, target_hash) {
            return Err(format!(
                "OpenList 源目标文件哈希不匹配: {path} ({algorithm}: {source_hash} != {target_hash})"
            ));
        }
        verified = true;
    }
    Ok(verified)
}

fn normalized_object_hashes(object: &OpenListObject) -> Result<BTreeMap<String, String>, String> {
    let mut hashes = BTreeMap::new();
    if let Some(raw) = object
        .hashinfo
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let parsed = serde_json::from_str::<Option<BTreeMap<String, String>>>(raw)
            .map_err(|error| format!("解析 OpenList hashinfo 失败: {error}"))?
            .unwrap_or_default();
        merge_object_hashes(&mut hashes, parsed)?;
    }
    if let Some(hash_info) = &object.hash_info {
        merge_object_hashes(&mut hashes, hash_info.clone())?;
    }
    Ok(hashes)
}

fn merge_object_hashes(
    hashes: &mut BTreeMap<String, String>,
    incoming: BTreeMap<String, String>,
) -> Result<(), String> {
    for (algorithm, value) in incoming {
        let algorithm = algorithm.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if algorithm.is_empty() || value.is_empty() {
            continue;
        }
        if let Some(previous) = hashes.get(&algorithm)
            && !hash_values_equal(previous, &value)
        {
            return Err(format!(
                "OpenList 对象包含冲突哈希: {algorithm} ({previous} != {value})"
            ));
        }
        hashes.insert(algorithm, value);
    }
    Ok(())
}

fn hash_values_equal(left: &str, right: &str) -> bool {
    let hexadecimal =
        |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    if hexadecimal(left) && hexadecimal(right) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

fn remote_child_path(parent: &str, name: &str) -> Result<String, String> {
    if !valid_child_name(name) {
        return Err(format!("OpenList 路径名称无效: {name}"));
    }
    Ok(if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", parent.trim_end_matches('/'))
    })
}

fn remote_parent_and_name(path: &str) -> Result<(String, String), String> {
    let path = path.trim_end_matches('/');
    if !path.starts_with('/') || path.is_empty() || path == "/" {
        return Err(format!("OpenList 目录路径无效: {path:?}"));
    }
    let (parent, name) = path
        .rsplit_once('/')
        .ok_or_else(|| format!("OpenList 目录路径无效: {path:?}"))?;
    if !valid_child_name(name) {
        return Err(format!("OpenList 目录路径无效: {path:?}"));
    }
    Ok((
        if parent.is_empty() { "/" } else { parent }.to_string(),
        name.to_string(),
    ))
}

fn is_object_not_found_business_error(code: i64, message: &str) -> bool {
    code == 500 && (message == "object not found" || message.ends_with(": object not found"))
}

fn is_temporary_business_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "timeout",
        "timed out",
        "temporarily",
        "temporary",
        "try again",
        "service unavailable",
        "too many requests",
        "rate limit",
        "connection reset",
        "connection refused",
        "超时",
        "暂时",
        "稍后",
        "限流",
        "连接重置",
        "连接被拒绝",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    use std::sync::{Arc, Mutex};

    use axum::extract::{Query, State};
    use axum::http::StatusCode as AxumStatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::{Json, Router, routing::post};
    use serde_json::{Value, json};

    use super::{
        CopyResult, ListResult, ManifestFileState, ManifestInspectError, OpenListClient,
        OpenListObject, OpenListRequestError, OpenListTask, openlist_identity_key,
        valid_child_name, validate_manifest_file,
    };

    #[test]
    fn identity_key_uses_full_unicode_case_folding() {
        assert_eq!(
            openlist_identity_key("/Media/ΟΣ.mkv"),
            openlist_identity_key("/media/ος.MKV")
        );
        assert_eq!(
            openlist_identity_key("/Media/Straße.mkv"),
            openlist_identity_key("/media/STRASSE.MKV")
        );
    }

    #[test]
    fn openlist_collection_results_accept_missing_or_null_fields() {
        for raw in [r#"{"total":0}"#, r#"{"content":null,"total":0}"#] {
            let result: ListResult = serde_json::from_str(raw).unwrap();
            assert!(result.content.is_empty(), "response: {raw}");
        }

        for raw in [r#"{}"#, r#"{"tasks":null}"#] {
            let result: CopyResult = serde_json::from_str(raw).unwrap();
            assert!(result.tasks.is_empty(), "response: {raw}");
        }
    }

    #[derive(Default)]
    struct FakeOpenListState {
        copies: Vec<(String, String, String)>,
        removes: Vec<(String, String)>,
        mkdirs: Vec<String>,
        objects: BTreeMap<String, Value>,
        get_overrides: BTreeMap<String, Value>,
        stale_get_paths: BTreeSet<String>,
        list_snapshots: HashMap<String, Vec<Value>>,
        list_removals_after_refresh: HashMap<String, Vec<String>>,
        mkdir_visibility_delay: usize,
        pending_mkdirs: HashMap<String, (String, usize)>,
    }

    type SharedFakeState = Arc<Mutex<FakeOpenListState>>;

    fn object(name: &str, size: i64, is_dir: bool) -> Value {
        json!({"name": name, "size": size, "is_dir": is_dir})
    }

    fn success(data: Value) -> Json<Value> {
        Json(json!({"code": 200, "message": "success", "data": data}))
    }

    fn missing() -> Json<Value> {
        Json(json!({"code": 500, "message": "object not found", "data": null}))
    }

    async fn fake_get(
        State(state): State<SharedFakeState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let path = body
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut state = state.lock().unwrap();
        let ready = state
            .pending_mkdirs
            .get_mut(&path)
            .and_then(|(_, remaining)| {
                if *remaining == 0 {
                    Some(())
                } else {
                    *remaining -= 1;
                    None
                }
            })
            .is_some();
        if ready {
            let (name, _) = state.pending_mkdirs.remove(&path).unwrap();
            state.objects.insert(path.clone(), object(&name, 0, true));
        }
        if state.stale_get_paths.contains(&path) {
            return missing();
        }
        if let Some(object) = state.get_overrides.get(&path) {
            return success(object.clone());
        }
        state
            .objects
            .get(&path)
            .cloned()
            .map(success)
            .unwrap_or_else(missing)
    }

    async fn fake_mkdir(
        State(state): State<SharedFakeState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let path = body
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let name = path.rsplit('/').next().unwrap_or_default().to_string();
        let mut state = state.lock().unwrap();
        state.mkdirs.push(path.clone());
        if state.mkdir_visibility_delay == 0 {
            state.objects.insert(path, object(&name, 0, true));
        } else {
            let delay = state.mkdir_visibility_delay;
            state.pending_mkdirs.insert(path, (name, delay - 1));
        }
        success(Value::Null)
    }

    async fn fake_list(
        State(state): State<SharedFakeState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let parent = body.get("path").and_then(Value::as_str).unwrap_or("/");
        let prefix = if parent == "/" {
            "/".to_string()
        } else {
            format!("{}/", parent.trim_end_matches('/'))
        };
        let page = body.get("page").and_then(Value::as_u64).unwrap_or(1) as usize;
        let per_page = body.get("per_page").and_then(Value::as_u64).unwrap_or(200) as usize;
        let refresh = body
            .get("refresh")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert_eq!(refresh, page == 1);
        let mut state = state.lock().unwrap();
        let snapshot_key = parent.to_string();
        let all_content = if refresh {
            let ready_paths = state
                .pending_mkdirs
                .iter_mut()
                .filter_map(|(path, (_, remaining))| {
                    let child = path.strip_prefix(&prefix)?;
                    if child.is_empty() || child.contains('/') {
                        return None;
                    }
                    if *remaining == 0 {
                        Some(path.clone())
                    } else {
                        *remaining -= 1;
                        None
                    }
                })
                .collect::<Vec<_>>();
            for path in ready_paths {
                let (name, _) = state.pending_mkdirs.remove(&path).unwrap();
                state.objects.insert(path, object(&name, 0, true));
            }
            let mut direct_children = BTreeMap::new();
            for (path, value) in &state.objects {
                let Some(child) = path.strip_prefix(&prefix) else {
                    continue;
                };
                if child.is_empty() {
                    continue;
                }
                if let Some((directory, _)) = child.split_once('/') {
                    direct_children
                        .entry(directory.to_string())
                        .or_insert_with(|| object(directory, 0, true));
                } else {
                    direct_children.insert(child.to_string(), value.clone());
                }
            }
            let snapshot = direct_children.into_values().collect::<Vec<_>>();
            state
                .list_snapshots
                .insert(snapshot_key.clone(), snapshot.clone());
            if let Some(paths) = state.list_removals_after_refresh.remove(&snapshot_key) {
                for path in paths {
                    state.objects.remove(&path);
                }
            }
            snapshot
        } else {
            state
                .list_snapshots
                .get(&snapshot_key)
                .cloned()
                .expect("later list pages must use the first page snapshot")
        };
        let total = all_content.len();
        let content = all_content
            .into_iter()
            .skip(page.saturating_sub(1).saturating_mul(per_page))
            .take(per_page)
            .collect::<Vec<_>>();
        success(json!({"content": content, "total": total}))
    }

    async fn fake_copy(
        State(state): State<SharedFakeState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        assert_eq!(body.get("overwrite").and_then(Value::as_bool), Some(false));
        assert_eq!(
            body.get("skip_existing").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(body.get("merge").and_then(Value::as_bool), Some(false));
        let src_dir = body
            .get("src_dir")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let dst_dir = body
            .get("dst_dir")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let name = body
            .get("names")
            .and_then(Value::as_array)
            .and_then(|names| names.first())
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut state = state.lock().unwrap();
        state.copies.push((src_dir, dst_dir, name));
        let id = format!("copy-{}", state.copies.len());
        success(json!({"tasks": [{"id": id, "state": 0}]}))
    }

    async fn fake_copy_without_task(Json(_body): Json<Value>) -> Json<Value> {
        success(Value::Null)
    }

    async fn fake_remove(
        State(state): State<SharedFakeState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let dir = body
            .get("dir")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let name = body
            .get("names")
            .and_then(Value::as_array)
            .and_then(|names| names.first())
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        state.lock().unwrap().removes.push((dir, name));
        success(Value::Null)
    }

    async fn fake_task_lookup(Query(query): Query<HashMap<String, String>>) -> Response {
        match query.get("tid").map(String::as_str) {
            Some("missing-task") => Json(json!({
                "code": 404,
                "message": "task not found",
                "data": null
            }))
            .into_response(),
            Some("padded-missing-task") => Json(json!({
                "code": 404,
                "message": " Task Not Found ",
                "data": null
            }))
            .into_response(),
            Some("uppercase-missing-task") => Json(json!({
                "code": 404,
                "message": "TASK NOT FOUND",
                "data": null
            }))
            .into_response(),
            Some("wrong-code-missing-task") => Json(json!({
                "code": 500,
                "message": "task not found",
                "data": null
            }))
            .into_response(),
            Some("localized-missing-task") => Json(json!({
                "code": 404,
                "message": "任务不存在",
                "data": null
            }))
            .into_response(),
            Some("unauthorized-task") => {
                (AxumStatusCode::UNAUTHORIZED, "task not found").into_response()
            }
            Some("http-404") => {
                (AxumStatusCode::NOT_FOUND, "reverse proxy task not found").into_response()
            }
            Some("object-missing") => Json(json!({
                "code": 404,
                "message": "storage/user/object not found",
                "data": null
            }))
            .into_response(),
            Some("malformed") => (AxumStatusCode::OK, "not-json").into_response(),
            _ => Json(json!({
                "code": 200,
                "message": "success",
                "data": {"id": "task", "state": 0}
            }))
            .into_response(),
        }
    }

    async fn fake_stat_lookup(Json(body): Json<Value>) -> Response {
        match body.get("path").and_then(Value::as_str) {
            Some("/exact-missing") => Json(json!({
                "code": 500,
                "message": "object not found",
                "data": null
            }))
            .into_response(),
            Some("/wrapped-missing") => Json(json!({
                "code": 500,
                "message": "failed get object: object not found",
                "data": null
            }))
            .into_response(),
            Some("/storage-missing") => Json(json!({
                "code": 500,
                "message": "storage not found",
                "data": null
            }))
            .into_response(),
            Some("/wrong-code") => Json(json!({
                "code": 404,
                "message": "object not found",
                "data": null
            }))
            .into_response(),
            Some("/broad-message") => Json(json!({
                "code": 500,
                "message": "object not found while listing storage",
                "data": null
            }))
            .into_response(),
            Some("/http-404") => {
                (AxumStatusCode::NOT_FOUND, "reverse proxy object not found").into_response()
            }
            Some("/malformed") => (AxumStatusCode::OK, "not-json").into_response(),
            _ => success(object("present", 1, false)).into_response(),
        }
    }

    #[tokio::test]
    async fn copy_request_skips_existing_without_overwriting_or_merging() {
        let state = Arc::new(Mutex::new(FakeOpenListState::default()));
        let app = Router::new()
            .route("/api/fs/copy", post(fake_copy))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let tasks = client.copy("/src", "/dst", "episode.mkv").await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            state.lock().unwrap().copies,
            vec![(
                "/src".to_string(),
                "/dst".to_string(),
                "episode.mkv".to_string(),
            )]
        );
        server.abort();
    }

    #[tokio::test]
    async fn copy_accepts_a_successful_synchronous_null_data_response() {
        let app = Router::new().route("/api/fs/copy", post(fake_copy_without_task));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        assert!(
            client
                .copy("/src", "/dst", "episode.mkv")
                .await
                .unwrap()
                .is_empty()
        );

        server.abort();
    }

    #[test]
    fn task_state_classification_is_strict() {
        let task = OpenListTask {
            id: "1".to_string(),
            name: String::new(),
            state: 0,
            status: String::new(),
            progress: 0.0,
            total_bytes: 0,
            error: "previous attempt failed".to_string(),
        };
        for state in 0..=9 {
            let mut observed = task.clone();
            observed.state = state;
            assert_eq!(observed.succeeded(), state == 2, "state {state}");
            assert_eq!(
                observed.terminal_failure(),
                matches!(state, 4 | 7),
                "state {state}"
            );
        }
        let mut unknown = task;
        unknown.state = 99;
        assert!(!unknown.succeeded());
        assert!(!unknown.terminal_failure());
    }

    #[test]
    fn manifest_inspect_error_classification_is_fail_closed() {
        for error in [
            OpenListRequestError::Transport("connection reset".to_string()),
            OpenListRequestError::Http {
                status: AxumStatusCode::SERVICE_UNAVAILABLE,
                body: "maintenance".to_string(),
            },
            OpenListRequestError::Http {
                status: AxumStatusCode::TOO_MANY_REQUESTS,
                body: "rate limited".to_string(),
            },
            OpenListRequestError::Decode("temporary invalid JSON".to_string()),
            OpenListRequestError::Business {
                code: 500,
                message: "upstream timeout, try again".to_string(),
            },
        ] {
            assert!(matches!(
                ManifestInspectError::from_request(error),
                ManifestInspectError::Transient(_)
            ));
        }

        for error in [
            OpenListRequestError::Http {
                status: AxumStatusCode::UNAUTHORIZED,
                body: "bad token".to_string(),
            },
            OpenListRequestError::Business {
                code: 500,
                message: "storage not found".to_string(),
            },
            OpenListRequestError::Business {
                code: 500,
                message: "object not found".to_string(),
            },
        ] {
            assert!(matches!(
                ManifestInspectError::from_request(error),
                ManifestInspectError::Conflict(_)
            ));
        }
    }

    #[tokio::test]
    async fn task_missing_requires_the_exact_business_error() {
        let app = Router::new().route("/api/task/copy/info", post(fake_task_lookup));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        assert_eq!(
            client.task_info_if_exists("missing-task").await.unwrap(),
            None
        );
        assert!(
            client
                .task_info_if_exists("padded-missing-task")
                .await
                .is_err()
        );
        assert!(
            client
                .task_info_if_exists("uppercase-missing-task")
                .await
                .is_err()
        );
        assert!(
            client
                .task_info_if_exists("wrong-code-missing-task")
                .await
                .is_err()
        );
        assert!(
            client
                .task_info_if_exists("localized-missing-task")
                .await
                .is_err()
        );
        assert!(
            client
                .task_info_if_exists("unauthorized-task")
                .await
                .is_err()
        );
        assert!(client.task_info_if_exists("http-404").await.is_err());
        assert!(client.task_info_if_exists("object-missing").await.is_err());
        assert!(client.task_info_if_exists("malformed").await.is_err());
        server.abort();

        let unused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unused_address = unused.local_addr().unwrap();
        drop(unused);
        let unavailable =
            OpenListClient::new(&format!("http://{unused_address}"), "test-key").unwrap();
        assert!(unavailable.task_info_if_exists("task").await.is_err());
    }

    #[tokio::test]
    async fn stat_missing_requires_the_exact_business_error() {
        let app = Router::new().route("/api/fs/get", post(fake_stat_lookup));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        assert_eq!(client.stat_if_exists("/exact-missing").await.unwrap(), None);
        assert_eq!(
            client.stat_if_exists("/wrapped-missing").await.unwrap(),
            None
        );
        for path in [
            "/storage-missing",
            "/wrong-code",
            "/broad-message",
            "/http-404",
            "/malformed",
        ] {
            assert!(client.stat_if_exists(path).await.is_err(), "path {path}");
        }
        assert_eq!(
            client.stat_if_exists("/present").await.unwrap(),
            Some(OpenListObject {
                name: "present".to_string(),
                size: 1,
                is_dir: false,
                hashinfo: None,
                hash_info: None,
            })
        );
        server.abort();

        let unused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unused_address = unused.local_addr().unwrap();
        drop(unused);
        let unavailable =
            OpenListClient::new(&format!("http://{unused_address}"), "test-key").unwrap();
        assert!(unavailable.stat_if_exists("/anything").await.is_err());
    }

    #[test]
    fn recursive_listing_only_accepts_direct_child_names() {
        assert!(valid_child_name("Season 01"));
        assert!(!valid_child_name("../Season 01"));
        assert!(!valid_child_name("Season/01"));
    }

    #[test]
    fn existing_file_requires_matching_type_name_and_size() {
        let source = OpenListObject {
            name: "episode.mkv".to_string(),
            size: 100,
            is_dir: false,
            hashinfo: None,
            hash_info: None,
        };
        assert!(validate_manifest_file(&source, "episode.mkv", 100, "目标", "/x").is_ok());
        assert!(
            validate_manifest_file(
                &OpenListObject {
                    name: source.name.clone(),
                    size: source.size,
                    is_dir: true,
                    hashinfo: None,
                    hash_info: None,
                },
                "episode.mkv",
                100,
                "目标",
                "/x",
            )
            .is_err()
        );
        assert!(validate_manifest_file(&source, "other.mkv", 100, "目标", "/x").is_err());
        assert!(validate_manifest_file(&source, "episode.mkv", 101, "目标", "/x").is_err());
    }

    #[tokio::test]
    async fn manifest_copy_recurses_only_through_declared_file_path() {
        let mut fake = FakeOpenListState::default();
        fake.objects
            .insert("/dst".to_string(), object("dst", 0, true));
        fake.objects
            .insert("/dst/Show".to_string(), object("Show", 0, true));
        fake.objects
            .insert("/dst/Show/Season".to_string(), object("Season", 0, true));
        fake.objects
            .insert("/src/Show".to_string(), object("Show", 0, true));
        fake.objects
            .insert("/src/Show/Season".to_string(), object("Season", 0, true));
        fake.objects.insert(
            "/src/Show/Season/E01.mkv".to_string(),
            object("E01.mkv", 300, false),
        );
        // This shared-root file is deliberately absent from the manifest.
        fake.objects.insert(
            "/src/Show/unrelated.bin".to_string(),
            object("unrelated.bin", 999, false),
        );
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .route("/api/fs/copy", post(fake_copy))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();
        let tasks = client
            .copy_manifest_file("/src", "/dst", "Show/Season/E01.mkv", 300)
            .await
            .unwrap();

        assert_eq!(tasks.len(), 1);
        let state = state.lock().unwrap();
        assert_eq!(
            state.copies,
            vec![(
                "/src/Show/Season".to_string(),
                "/dst/Show/Season".to_string(),
                "E01.mkv".to_string(),
            ),]
        );
        assert!(state.mkdirs.is_empty());
        drop(state);
        server.abort();
    }

    #[tokio::test]
    async fn refreshed_target_listing_prevents_copy_when_get_cache_is_stale() {
        let mut fake = FakeOpenListState::default();
        for (path, value) in [
            ("/dst", object("dst", 0, true)),
            ("/dst/Show", object("Show", 0, true)),
            ("/src/Show", object("Show", 0, true)),
            (
                "/src/Show/E01.mkv",
                json!({
                    "name": "E01.mkv",
                    "size": 300,
                    "is_dir": false,
                    "hashinfo": "{\"md5\":\"AABBCCDD\"}"
                }),
            ),
            (
                "/dst/Show/E01.mkv",
                json!({
                    "name": "E01.mkv",
                    "size": 300,
                    "is_dir": false,
                    "hash_info": {"md5": "aabbccdd"}
                }),
            ),
        ] {
            fake.objects.insert(path.to_string(), value);
        }
        fake.stale_get_paths.extend([
            "/dst".to_string(),
            "/dst/Show".to_string(),
            "/dst/Show/E01.mkv".to_string(),
        ]);
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .route("/api/fs/copy", post(fake_copy))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        assert_eq!(
            client
                .inspect_manifest_file("/src", "/dst", "Show/E01.mkv", 300)
                .await
                .unwrap(),
            ManifestFileState::Present {
                hash_verified: true,
            }
        );
        let tasks = client
            .copy_manifest_file("/src", "/dst", "Show/E01.mkv", 300)
            .await
            .unwrap();
        assert!(tasks.is_empty());
        let state = state.lock().unwrap();
        assert!(state.mkdirs.is_empty());
        assert!(state.copies.is_empty());
        drop(state);
        server.abort();
    }

    #[tokio::test]
    async fn later_pages_reuse_the_first_refreshed_snapshot() {
        let mut fake = FakeOpenListState::default();
        fake.objects
            .insert("/dst".to_string(), object("dst", 0, true));
        fake.objects.insert(
            "/src/zz-episode.mkv".to_string(),
            object("zz-episode.mkv", 300, false),
        );
        for index in 0..250 {
            let name = format!("item-{index:03}");
            fake.objects
                .insert(format!("/dst/{name}"), object(&name, index as i64, false));
        }
        let target_path = "/dst/zz-episode.mkv".to_string();
        fake.objects
            .insert(target_path.clone(), object("zz-episode.mkv", 300, false));
        fake.list_removals_after_refresh
            .insert("/dst".to_string(), vec![target_path.clone()]);
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/copy", post(fake_copy))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let tasks = client
            .copy_manifest_file("/src", "/dst", "zz-episode.mkv", 300)
            .await
            .unwrap();
        assert!(tasks.is_empty());
        let state = state.lock().unwrap();
        assert!(!state.objects.contains_key(&target_path));
        assert!(state.copies.is_empty());
        drop(state);
        server.abort();
    }

    #[tokio::test]
    async fn exact_name_on_first_page_and_case_variant_on_later_page_are_a_conflict() {
        let mut fake = FakeOpenListState::default();
        fake.objects.insert(
            "/src/Episode.MKV".to_string(),
            json!({
                "name": "Episode.MKV",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aabbccdd"}
            }),
        );
        for index in 0..199 {
            let name = format!("{index:03}-item");
            fake.objects
                .insert(format!("/dst/{name}"), object(&name, index, false));
        }
        fake.objects.insert(
            "/dst/Episode.MKV".to_string(),
            json!({
                "name": "Episode.MKV",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aabbccdd"}
            }),
        );
        fake.objects.insert(
            "/dst/episode.mkv".to_string(),
            json!({
                "name": "episode.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aabbccdd"}
            }),
        );
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .route("/api/fs/copy", post(fake_copy))
            .route("/api/fs/remove", post(fake_remove))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .remove_manifest_file_if_exists("/src", "/dst", "Episode.MKV", 300)
            .await
            .unwrap_err();
        assert!(error.contains("大小写"));
        let state = state.lock().unwrap();
        assert!(state.mkdirs.is_empty());
        assert!(state.copies.is_empty());
        assert!(state.removes.is_empty());
        drop(state);
        server.abort();
    }

    #[tokio::test]
    async fn case_insensitive_name_conflicts_block_all_mutations() {
        let mut fake = FakeOpenListState::default();
        for (path, value) in [
            ("/dst", object("dst", 0, true)),
            ("/dst/Season", object("Season", 0, true)),
            ("/dst/Episode.MKV", object("Episode.MKV", 300, false)),
            ("/src/episode.mkv", object("episode.mkv", 300, false)),
        ] {
            fake.objects.insert(path.to_string(), value);
        }
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .route("/api/fs/copy", post(fake_copy))
            .route("/api/fs/remove", post(fake_remove))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let directory_error = client
            .create_directory_if_missing("/dst/season")
            .await
            .unwrap_err();
        assert!(directory_error.contains("大小写"));
        let copy_error = client
            .copy_manifest_file("/src", "/dst", "episode.mkv", 300)
            .await
            .unwrap_err();
        assert!(copy_error.contains("大小写"));
        let remove_error = client
            .remove_manifest_file_if_exists("/src", "/dst", "episode.mkv", 300)
            .await
            .unwrap_err();
        assert!(remove_error.contains("大小写"));
        let state = state.lock().unwrap();
        assert!(state.mkdirs.is_empty());
        assert!(state.copies.is_empty());
        assert!(state.removes.is_empty());
        drop(state);
        server.abort();
    }

    #[tokio::test]
    async fn unicode_case_name_conflict_blocks_directory_creation() {
        let mut fake = FakeOpenListState::default();
        fake.objects
            .insert("/dst".to_string(), object("dst", 0, true));
        fake.objects
            .insert("/dst/Ä".to_string(), object("Ä", 0, true));
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .create_directory_if_missing("/dst/ä")
            .await
            .unwrap_err();
        assert!(error.contains("大小写"));
        assert!(state.lock().unwrap().mkdirs.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn unicode_normalization_equivalent_name_is_reused_without_mutation() {
        let decomposed = "e\u{301}";
        let mut fake = FakeOpenListState::default();
        fake.objects
            .insert("/dst".to_string(), object("dst", 0, true));
        fake.objects
            .insert(format!("/dst/{decomposed}"), object(decomposed, 0, true));
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        client.create_directory_if_missing("/dst/é").await.unwrap();
        assert!(state.lock().unwrap().mkdirs.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn directory_creation_uses_the_resolved_unicode_parent() {
        let decomposed_parent = "Cafe\u{301}";
        let mut fake = FakeOpenListState::default();
        fake.objects
            .insert("/dst".to_string(), object("dst", 0, true));
        fake.objects.insert(
            format!("/dst/{decomposed_parent}"),
            object(decomposed_parent, 0, true),
        );
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        client
            .create_directory_if_missing("/dst/Café/Season")
            .await
            .unwrap();

        assert_eq!(
            state.lock().unwrap().mkdirs,
            vec![format!("/dst/{decomposed_parent}/Season")]
        );
        server.abort();
    }

    #[tokio::test]
    async fn source_leaf_case_conflict_blocks_copy_submission() {
        let mut fake = FakeOpenListState::default();
        for (path, value) in [
            ("/dst", object("dst", 0, true)),
            ("/src/Episode.mkv", object("Episode.mkv", 300, false)),
            ("/src/episode.mkv", object("episode.mkv", 300, false)),
        ] {
            fake.objects.insert(path.to_string(), value);
        }
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/copy", post(fake_copy))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .copy_manifest_file("/src", "/dst", "Episode.mkv", 300)
            .await
            .unwrap_err();

        assert!(error.contains("大小写"));
        assert!(state.lock().unwrap().copies.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn source_root_unicode_variants_are_a_conflict() {
        let decomposed = "Cafe\u{301}";
        let mut fake = FakeOpenListState::default();
        for (path, value) in [
            ("/archive", object("archive", 0, true)),
            ("/archive/Café", object("Café", 0, true)),
            (
                &format!("/archive/{decomposed}"),
                object(decomposed, 0, true),
            ),
            ("/archive/Café/E01.mkv", object("E01.mkv", 300, false)),
            ("/dst", object("dst", 0, true)),
        ] {
            fake.objects.insert(path.to_string(), value);
        }
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/copy", post(fake_copy))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .inspect_manifest_file("/archive/Café", "/dst", "E01.mkv", 300)
            .await
            .unwrap_err();

        assert!(matches!(error, ManifestInspectError::Conflict(_)));
        assert!(error.to_string().contains("Unicode"));
        assert!(state.lock().unwrap().copies.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn copy_uses_resolved_unicode_source_and_target_names() {
        let source_root = "Cafe\u{301}";
        let season = "Saiso\u{301}n";
        let episode = "E\u{301}pisode.mkv";
        let mut fake = FakeOpenListState::default();
        for (path, value) in [
            ("/archive".to_string(), object("archive", 0, true)),
            (
                format!("/archive/{source_root}"),
                object(source_root, 0, true),
            ),
            (
                format!("/archive/{source_root}/{season}"),
                object(season, 0, true),
            ),
            (
                format!("/archive/{source_root}/{season}/{episode}"),
                object(episode, 300, false),
            ),
            ("/dst".to_string(), object("dst", 0, true)),
            (format!("/dst/{season}"), object(season, 0, true)),
        ] {
            fake.objects.insert(path, value);
        }
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/copy", post(fake_copy))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        client
            .copy_manifest_file("/archive/Café", "/dst", "Saisón/Épisode.mkv", 300)
            .await
            .unwrap();

        assert_eq!(
            state.lock().unwrap().copies,
            vec![(
                format!("/archive/{source_root}/{season}"),
                format!("/dst/{season}"),
                episode.to_string(),
            )]
        );
        server.abort();
    }

    #[tokio::test]
    async fn duplicate_unicode_normalization_variants_are_a_conflict() {
        let decomposed = "e\u{301}";
        let mut fake = FakeOpenListState::default();
        fake.objects
            .insert("/dst/é".to_string(), object("é", 0, true));
        fake.objects
            .insert(format!("/dst/{decomposed}"), object(decomposed, 0, true));
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .create_directory_if_missing("/dst/é")
            .await
            .unwrap_err();
        assert!(error.contains("Unicode"));
        assert!(state.lock().unwrap().mkdirs.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn unicode_equivalent_source_and_target_are_rejected_before_removal() {
        let client = OpenListClient::new("http://127.0.0.1:1", "test-key").unwrap();

        let error = client
            .remove_manifest_file_if_exists(
                "/archive/Café",
                "/archive/Cafe\u{301}",
                "episode.mkv",
                300,
            )
            .await
            .unwrap_err();

        assert!(error.contains("源与目标文件路径相同"));
    }

    #[tokio::test]
    async fn removal_uses_resolved_unicode_source_name() {
        let directory = "Cafe\u{301}";
        let file_name = "E\u{301}pisode.mkv";
        let file = || {
            json!({
                "name": file_name,
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aabbccdd"}
            })
        };
        let mut fake = FakeOpenListState::default();
        for (path, value) in [
            ("/archive".to_string(), object("archive", 0, true)),
            (format!("/archive/{directory}"), object(directory, 0, true)),
            (format!("/archive/{directory}/{file_name}"), file()),
            ("/dst".to_string(), object("dst", 0, true)),
            (format!("/dst/{directory}"), object(directory, 0, true)),
            (format!("/dst/{directory}/{file_name}"), file()),
        ] {
            fake.objects.insert(path, value);
        }
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/remove", post(fake_remove))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        client
            .remove_manifest_file_if_exists("/archive/Café", "/dst/Café", "Épisode.mkv", 300)
            .await
            .unwrap();

        assert_eq!(
            state.lock().unwrap().removes,
            vec![(format!("/archive/{directory}"), file_name.to_string())]
        );
        server.abort();
    }

    #[tokio::test]
    async fn missing_target_directory_is_reported_without_side_effects() {
        let mut fake = FakeOpenListState::default();
        for (path, value) in [
            ("/cmcc", object("cmcc", 0, true)),
            ("/cmcc/Download", object("Download", 0, true)),
            ("/src/Show", object("Show", 0, true)),
            ("/src/Show/E01.mkv", object("E01.mkv", 300, false)),
        ] {
            fake.objects.insert(path.to_string(), value);
        }
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .route("/api/fs/copy", post(fake_copy))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();
        for _ in 0..2 {
            assert_eq!(
                client
                    .inspect_manifest_file(
                        "/src",
                        "/cmcc/Download/media/2024",
                        "Show/E01.mkv",
                        300,
                    )
                    .await
                    .unwrap(),
                ManifestFileState::MissingDirectory {
                    path: "/cmcc/Download/media".to_string(),
                }
            );
        }
        let error = client
            .copy_manifest_file("/src", "/cmcc/Download/media/2024", "Show/E01.mkv", 300)
            .await
            .unwrap_err();
        assert!(error.contains("/cmcc/Download/media"));

        let state = state.lock().unwrap();
        assert!(state.mkdirs.is_empty());
        assert!(state.copies.is_empty());
        drop(state);
        server.abort();
    }

    #[tokio::test]
    async fn target_directory_file_conflict_is_read_only() {
        let mut fake = FakeOpenListState::default();
        for (path, value) in [
            ("/dst", object("dst", 0, true)),
            ("/dst/Show", object("Show", 123, false)),
            ("/src/Show", object("Show", 0, true)),
            ("/src/Show/E01.mkv", object("E01.mkv", 300, false)),
        ] {
            fake.objects.insert(path.to_string(), value);
        }
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .route("/api/fs/copy", post(fake_copy))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .inspect_manifest_file("/src", "/dst", "Show/E01.mkv", 300)
            .await
            .unwrap_err();
        assert!(matches!(&error, ManifestInspectError::Conflict(_)));
        assert!(error.to_string().contains("应为目录"));
        let state = state.lock().unwrap();
        assert!(state.mkdirs.is_empty());
        assert!(state.copies.is_empty());
        drop(state);
        server.abort();
    }

    #[tokio::test]
    async fn directory_dedupe_scans_all_refreshed_list_pages() {
        let mut fake = FakeOpenListState::default();
        fake.objects
            .insert("/dst".to_string(), object("dst", 0, true));
        for index in 0..250 {
            let name = format!("item-{index:03}");
            fake.objects
                .insert(format!("/dst/{name}"), object(&name, 0, true));
        }
        fake.objects
            .insert("/dst/zz-target".to_string(), object("zz-target", 0, true));
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();
        client
            .create_directory_if_missing("/dst/zz-target")
            .await
            .unwrap();
        assert!(state.lock().unwrap().mkdirs.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn directory_creation_is_confirmed_through_refreshed_listing() {
        let mut fake = FakeOpenListState::default();
        fake.mkdir_visibility_delay = 2;
        fake.objects
            .insert("/dst".to_string(), object("dst", 0, true));
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        client
            .create_directory_if_missing("/dst/new-directory")
            .await
            .unwrap();
        let state = state.lock().unwrap();
        assert_eq!(state.mkdirs, vec!["/dst/new-directory".to_string()]);
        assert!(state.objects.contains_key("/dst/new-directory"));
        drop(state);
        server.abort();
    }

    #[tokio::test]
    async fn mismatched_target_is_never_removed_or_overwritten() {
        let mut fake = FakeOpenListState::default();
        for (path, value) in [
            ("/dst", object("dst", 0, true)),
            ("/src/Show", object("Show", 0, true)),
            ("/dst/Show", object("Show", 0, true)),
            ("/src/Show/E01.mkv", object("E01.mkv", 300, false)),
            ("/dst/Show/E01.mkv", object("E01.mkv", 299, false)),
        ] {
            fake.objects.insert(path.to_string(), value);
        }
        fake.stale_get_paths.extend([
            "/dst".to_string(),
            "/dst/Show".to_string(),
            "/dst/Show/E01.mkv".to_string(),
        ]);
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .route("/api/fs/copy", post(fake_copy))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .inspect_manifest_file("/src", "/dst", "Show/E01.mkv", 300)
            .await
            .unwrap_err();
        assert!(matches!(&error, ManifestInspectError::Conflict(_)));
        assert!(error.to_string().contains("大小不匹配"));
        assert!(state.lock().unwrap().copies.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn common_source_and_target_hash_mismatch_is_rejected() {
        let mut fake = FakeOpenListState::default();
        for (path, value) in [
            ("/dst", object("dst", 0, true)),
            ("/src/Show", object("Show", 0, true)),
            ("/dst/Show", object("Show", 0, true)),
            (
                "/src/Show/E01.mkv",
                json!({
                    "name": "E01.mkv",
                    "size": 300,
                    "is_dir": false,
                    "hash_info": {"sha1": "11111111", "md5": "aaaaaaaa"}
                }),
            ),
            (
                "/dst/Show/E01.mkv",
                json!({
                    "name": "E01.mkv",
                    "size": 300,
                    "is_dir": false,
                    "hashinfo": "{\"md5\":\"bbbbbbbb\",\"sha256\":\"22222222\"}"
                }),
            ),
        ] {
            fake.objects.insert(path.to_string(), value);
        }
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .route("/api/fs/copy", post(fake_copy))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .inspect_manifest_file("/src", "/dst", "Show/E01.mkv", 300)
            .await
            .unwrap_err();
        assert!(matches!(&error, ManifestInspectError::Conflict(_)));
        let message = error.to_string();
        assert!(message.contains("md5"));
        assert!(message.contains("哈希不匹配"));
        assert!(state.lock().unwrap().copies.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn stale_source_get_hash_cannot_override_the_refreshed_listing() {
        let source_path = "/src/Show/E01.mkv";
        let mut fake = FakeOpenListState::default();
        for (path, value) in [
            ("/dst", object("dst", 0, true)),
            ("/src/Show", object("Show", 0, true)),
            ("/dst/Show", object("Show", 0, true)),
            (
                source_path,
                json!({
                    "name": "E01.mkv",
                    "size": 300,
                    "is_dir": false,
                    "hash_info": {"md5": "bbbbbbbb"}
                }),
            ),
            (
                "/dst/Show/E01.mkv",
                json!({
                    "name": "E01.mkv",
                    "size": 300,
                    "is_dir": false,
                    "hash_info": {"md5": "aaaaaaaa"}
                }),
            ),
        ] {
            fake.objects.insert(path.to_string(), value);
        }
        fake.get_overrides.insert(
            source_path.to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aaaaaaaa"}
            }),
        );
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .inspect_manifest_file("/src", "/dst", "Show/E01.mkv", 300)
            .await
            .unwrap_err();

        assert!(matches!(error, ManifestInspectError::Conflict(_)));
        assert!(error.to_string().contains("哈希不匹配"));
        server.abort();
    }

    #[tokio::test]
    async fn existing_manifest_file_does_not_submit_copy() {
        let mut fake = FakeOpenListState::default();
        for (path, value) in [
            ("/dst", object("dst", 0, true)),
            ("/src/Show", object("Show", 0, true)),
            ("/dst/Show", object("Show", 0, true)),
            (
                "/src/Show/E01.mkv",
                json!({
                    "name": "E01.mkv",
                    "size": 300,
                    "is_dir": false,
                    "hash_info": {"md5": "aaaaaaaa"}
                }),
            ),
            (
                "/dst/Show/E01.mkv",
                json!({
                    "name": "E01.mkv",
                    "size": 300,
                    "is_dir": false,
                    "hashinfo": "{\"sha1\":\"11111111\"}"
                }),
            ),
        ] {
            fake.objects.insert(path.to_string(), value);
        }
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/mkdir", post(fake_mkdir))
            .route("/api/fs/copy", post(fake_copy))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        assert_eq!(
            client
                .inspect_manifest_file("/src", "/dst", "Show/E01.mkv", 300)
                .await
                .unwrap(),
            ManifestFileState::Present {
                hash_verified: false,
            }
        );
        assert!(state.lock().unwrap().copies.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn refreshed_matching_manifest_file_is_removed() {
        let mut fake = FakeOpenListState::default();
        fake.objects.insert(
            "/src/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aabbccdd"}
            }),
        );
        fake.get_overrides.insert(
            "/src/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hashinfo": "{\"md5\":\"AABBCCDD\"}"
            }),
        );
        fake.objects.insert(
            "/dst/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aabbccdd"}
            }),
        );
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/remove", post(fake_remove))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        assert!(
            client
                .manifest_file_removal_needed("/src", "/dst", "Show/E01.mkv", 300)
                .await
                .unwrap()
        );
        assert!(state.lock().unwrap().removes.is_empty());
        client
            .remove_manifest_file_if_exists("/src", "/dst", "Show/E01.mkv", 300)
            .await
            .unwrap();
        assert_eq!(
            state.lock().unwrap().removes,
            vec![("/src/Show".to_string(), "E01.mkv".to_string())]
        );
        server.abort();
    }

    #[tokio::test]
    async fn current_source_and_target_without_a_common_hash_are_not_removed() {
        let mut fake = FakeOpenListState::default();
        fake.objects.insert(
            "/src/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aaaaaaaa"}
            }),
        );
        fake.get_overrides.insert(
            "/src/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aaaaaaaa"}
            }),
        );
        fake.objects.insert(
            "/dst/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"sha1": "11111111"}
            }),
        );
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/remove", post(fake_remove))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .remove_manifest_file_if_exists("/src", "/dst", "Show/E01.mkv", 300)
            .await
            .unwrap_err();
        assert!(error.contains("缺少共同哈希"));
        assert!(state.lock().unwrap().removes.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn current_source_and_target_hash_mismatch_is_not_removed() {
        let mut fake = FakeOpenListState::default();
        fake.objects.insert(
            "/src/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aaaaaaaa"}
            }),
        );
        fake.objects.insert(
            "/dst/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "bbbbbbbb"}
            }),
        );
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/remove", post(fake_remove))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .remove_manifest_file_if_exists("/src", "/dst", "Show/E01.mkv", 300)
            .await
            .unwrap_err();
        assert!(error.contains("哈希不匹配"));
        assert!(state.lock().unwrap().removes.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn stale_get_snapshot_cannot_remove_refreshed_replacement() {
        let mut fake = FakeOpenListState::default();
        fake.objects.insert(
            "/src/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "bbbbbbbb"}
            }),
        );
        fake.get_overrides.insert(
            "/src/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aaaaaaaa"}
            }),
        );
        fake.objects.insert(
            "/dst/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "bbbbbbbb"}
            }),
        );
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/remove", post(fake_remove))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .remove_manifest_file_if_exists("/src", "/dst", "Show/E01.mkv", 300)
            .await
            .unwrap_err();
        assert!(error.contains("哈希不匹配"));
        assert!(state.lock().unwrap().removes.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn stale_get_snapshot_does_not_remove_a_refreshed_missing_file() {
        let mut fake = FakeOpenListState::default();
        fake.get_overrides.insert(
            "/src/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aaaaaaaa"}
            }),
        );
        fake.objects.insert(
            "/dst/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aaaaaaaa"}
            }),
        );
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/remove", post(fake_remove))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        client
            .remove_manifest_file_if_exists("/src", "/dst", "Show/E01.mkv", 300)
            .await
            .unwrap();
        assert!(state.lock().unwrap().removes.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn missing_get_snapshot_cannot_remove_a_refreshed_file() {
        let mut fake = FakeOpenListState::default();
        fake.objects.insert(
            "/src/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aaaaaaaa"}
            }),
        );
        fake.objects.insert(
            "/dst/Show/E01.mkv".to_string(),
            json!({
                "name": "E01.mkv",
                "size": 300,
                "is_dir": false,
                "hash_info": {"md5": "aaaaaaaa"}
            }),
        );
        fake.stale_get_paths.insert("/src/Show/E01.mkv".to_string());
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/remove", post(fake_remove))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .remove_manifest_file_if_exists("/src", "/dst", "Show/E01.mkv", 300)
            .await
            .unwrap_err();
        assert!(error.contains("身份快照"));
        assert!(state.lock().unwrap().removes.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn manifest_file_removal_refuses_a_directory() {
        let mut fake = FakeOpenListState::default();
        fake.objects
            .insert("/src/Show/E01.mkv".to_string(), object("E01.mkv", 0, true));
        fake.objects.insert(
            "/dst/Show/E01.mkv".to_string(),
            object("E01.mkv", 300, false),
        );
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/list", post(fake_list))
            .route("/api/fs/remove", post(fake_remove))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .remove_manifest_file_if_exists("/src", "/dst", "Show/E01.mkv", 300)
            .await
            .unwrap_err();
        assert!(error.contains("目录"));
        assert!(state.lock().unwrap().removes.is_empty());
        server.abort();
    }
}
