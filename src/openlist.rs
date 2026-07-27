use std::fmt;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const DIRECTORY_VISIBILITY_ATTEMPTS: usize = 20;
const DIRECTORY_VISIBILITY_DELAY: Duration = Duration::from_millis(250);

type ManifestDirectoryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(String, String), String>> + Send + 'a>>;

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
        matches!(self.state, 4 | 5 | 7) || !self.error.trim().is_empty()
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestFileState {
    Missing,
    Present,
}

#[derive(Debug, Deserialize)]
struct CopyResult {
    #[serde(default)]
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
    #[serde(default)]
    content: Vec<OpenListObject>,
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
        let data: CopyResult = self
            .post(
                "/api/fs/copy",
                &CopyRequest {
                    src_dir,
                    dst_dir,
                    names: &names,
                    overwrite: false,
                    skip_existing: false,
                    merge: false,
                },
            )
            .await?;
        Ok(data.tasks)
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

    pub async fn stat(&self, path: &str) -> Result<OpenListObject, String> {
        self.post("/api/fs/get", &PathRequest { path, password: "" })
            .await
    }

    pub async fn stat_if_exists(&self, path: &str) -> Result<Option<OpenListObject>, String> {
        match self.stat(path).await {
            Ok(object) => Ok(Some(object)),
            Err(error) if is_not_found_error(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub async fn create_directory_if_missing(&self, path: &str) -> Result<(), String> {
        match self.refreshed_directory_if_exists(path).await? {
            Some(object) if object.is_dir => Ok(()),
            Some(_) => Err(format!("OpenList 目标路径应为目录但实际是文件: {path}")),
            None => {
                let create_result = self
                    .post_empty("/api/fs/mkdir", &MkdirRequest { path })
                    .await;
                for attempt in 0..DIRECTORY_VISIBILITY_ATTEMPTS {
                    match self.stat_if_exists(path).await? {
                        Some(object) if object.is_dir => return Ok(()),
                        Some(_) => {
                            return Err(format!(
                                "OpenList 目标路径应为目录但实际是文件: {path}"
                            ));
                        }
                        None if attempt + 1 < DIRECTORY_VISIBILITY_ATTEMPTS => {
                            tokio::time::sleep(DIRECTORY_VISIBILITY_DELAY).await;
                        }
                        None => {}
                    }
                }
                Err(create_result.err().unwrap_or_else(|| {
                    format!("OpenList 创建目录后长时间不可见: {path}")
                }))
            }
        }
    }

    async fn refreshed_directory_if_exists(
        &self,
        path: &str,
    ) -> Result<Option<OpenListObject>, String> {
        let (parent, name) = remote_parent_and_name(path)?;
        let result: ListResult = self
            .post(
                "/api/fs/list",
                &ListRequest {
                    path: &parent,
                    password: "",
                    page: 1,
                    per_page: 0,
                    refresh: true,
                },
            )
            .await?;
        Ok(result.content.into_iter().find(|object| object.name == name))
    }

    pub async fn create_directory_tree_if_missing(&self, path: &str) -> Result<(), String> {
        if path == "/" {
            return Ok(());
        }
        if !path.starts_with('/') || path.contains('\0') || path.contains('\\') {
            return Err(format!("OpenList 目标目录路径无效: {path:?}"));
        }
        let components = path
            .split('/')
            .filter(|component| !component.is_empty())
            .collect::<Vec<_>>();
        if components.is_empty() || components.iter().any(|name| !valid_child_name(name)) {
            return Err(format!("OpenList 目标目录路径无效: {path:?}"));
        }

        let mut current = String::new();
        for component in components {
            current.push('/');
            current.push_str(component);
            self.create_directory_if_missing(&current).await?;
        }
        Ok(())
    }

    pub async fn inspect_manifest_file(
        &self,
        src_root: &str,
        dst_root: &str,
        relative_path: &str,
        expected_size: i64,
    ) -> Result<ManifestFileState, String> {
        if expected_size < 0 {
            return Err(format!(
                "OpenList 文件大小无效: {relative_path}={expected_size}"
            ));
        }
        let components = manifest_components(relative_path)?;
        let (file_name, directories) = components
            .split_last()
            .ok_or_else(|| "OpenList manifest 文件路径为空".to_string())?;
        self.create_directory_tree_if_missing(dst_root).await?;
        let (source_dir, target_dir) = self
            .ensure_manifest_directories(src_root.to_string(), dst_root.to_string(), directories)
            .await?;
        let source_path = remote_child_path(&source_dir, file_name)?;
        let source = self.stat(&source_path).await?;
        validate_manifest_file(&source, file_name, expected_size, "源", &source_path)?;

        let target_path = remote_child_path(&target_dir, file_name)?;
        match self.stat_if_exists(&target_path).await? {
            None => Ok(ManifestFileState::Missing),
            Some(target) => {
                validate_manifest_file(&target, file_name, expected_size, "目标", &target_path)?;
                Ok(ManifestFileState::Present)
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
        if self
            .inspect_manifest_file(src_root, dst_root, relative_path, expected_size)
            .await?
            == ManifestFileState::Present
        {
            return Ok(Vec::new());
        }
        let components = manifest_components(relative_path)?;
        let (file_name, directories) = components
            .split_last()
            .ok_or_else(|| "OpenList manifest 文件路径为空".to_string())?;
        let source_dir = append_remote_components(src_root, directories)?;
        let target_dir = append_remote_components(dst_root, directories)?;
        self.copy(&source_dir, &target_dir, file_name).await
    }

    pub async fn remove_manifest_file_if_exists(
        &self,
        root: &str,
        relative_path: &str,
        expected_size: i64,
    ) -> Result<(), String> {
        let components = manifest_components(relative_path)?;
        let (file_name, directories) = components
            .split_last()
            .ok_or_else(|| "OpenList manifest 文件路径为空".to_string())?;
        let directory = append_remote_components(root, directories)?;
        let path = remote_child_path(&directory, file_name)?;
        let Some(object) = self.stat_if_exists(&path).await? else {
            return Ok(());
        };
        validate_manifest_file(&object, file_name, expected_size, "待删除源", &path)?;
        self.remove(&directory, file_name).await
    }

    fn ensure_manifest_directories<'a>(
        &'a self,
        source_dir: String,
        target_dir: String,
        directories: &'a [&'a str],
    ) -> ManifestDirectoryFuture<'a> {
        Box::pin(async move {
            let Some((directory, remaining)) = directories.split_first() else {
                return Ok((source_dir, target_dir));
            };
            let next_source = remote_child_path(&source_dir, directory)?;
            let source = self.stat(&next_source).await?;
            if !source.is_dir || source.name != *directory {
                return Err(format!("OpenList 源 manifest 路径应为目录: {next_source}"));
            }
            let next_target = remote_child_path(&target_dir, directory)?;
            match self.stat_if_exists(&next_target).await? {
                Some(target) if target.is_dir && target.name == *directory => {}
                Some(_) => {
                    return Err(format!(
                        "OpenList 目标 manifest 路径应为目录: {next_target}"
                    ));
                }
                None => self.create_directory_if_missing(&next_target).await?,
            }
            self.ensure_manifest_directories(next_source, next_target, remaining)
                .await
        })
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
    if object.name != expected_name {
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
    Ok((if parent.is_empty() { "/" } else { parent }.to_string(), name.to_string()))
}

fn is_not_found_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("not found")
        || error.contains("object not found")
        || error.contains("file does not exist")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::{Arc, Mutex};

    use axum::extract::{Query, State};
    use axum::http::StatusCode as AxumStatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::{routing::post, Json, Router};
    use serde_json::{json, Value};

    use super::{
        is_not_found_error, valid_child_name, validate_manifest_file, ManifestFileState,
        OpenListClient, OpenListObject, OpenListTask,
    };

    #[derive(Default)]
    struct FakeOpenListState {
        copies: Vec<(String, String, String)>,
        removes: Vec<(String, String)>,
        mkdirs: Vec<String>,
        objects: BTreeMap<String, Value>,
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
        state.objects
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
        assert_eq!(body.get("refresh").and_then(Value::as_bool), Some(true));
        let parent = body
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("/");
        let prefix = if parent == "/" {
            "/".to_string()
        } else {
            format!("{}/", parent.trim_end_matches('/'))
        };
        let content = state
            .lock()
            .unwrap()
            .objects
            .iter()
            .filter_map(|(path, object)| {
                let child = path.strip_prefix(&prefix)?;
                (!child.is_empty() && !child.contains('/')).then_some(object.clone())
            })
            .collect::<Vec<_>>();
        success(json!({"content": content, "total": content.len()}))
    }

    async fn fake_copy(
        State(state): State<SharedFakeState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
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

    #[test]
    fn task_state_classification_is_strict() {
        let mut task = OpenListTask {
            id: "1".to_string(),
            name: String::new(),
            state: 2,
            status: String::new(),
            progress: 100.0,
            total_bytes: 0,
            error: String::new(),
        };
        assert!(task.succeeded());
        assert!(!task.terminal_failure());
        task.state = 7;
        assert!(!task.succeeded());
        assert!(task.terminal_failure());
        task.state = 5;
        assert!(task.terminal_failure());
        task.state = 1;
        task.error = "destination directory not found".to_string();
        assert!(task.terminal_failure());
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
        assert!(client
            .task_info_if_exists("padded-missing-task")
            .await
            .is_err());
        assert!(client
            .task_info_if_exists("uppercase-missing-task")
            .await
            .is_err());
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

    #[test]
    fn only_explicit_missing_errors_are_idempotent() {
        assert!(is_not_found_error(
            "OpenList business error: object not found"
        ));
        assert!(!is_not_found_error("request timed out"));
        assert!(!is_not_found_error("permission denied"));
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
        };
        assert!(validate_manifest_file(&source, "episode.mkv", 100, "目标", "/x").is_ok());
        assert!(validate_manifest_file(
            &OpenListObject {
                name: source.name.clone(),
                size: source.size,
                is_dir: true,
            },
            "episode.mkv",
            100,
            "目标",
            "/x",
        )
        .is_err());
        assert!(validate_manifest_file(&source, "other.mkv", 100, "目标", "/x").is_err());
        assert!(validate_manifest_file(&source, "episode.mkv", 101, "目标", "/x").is_err());
    }

    #[tokio::test]
    async fn manifest_copy_recurses_only_through_declared_file_path() {
        let mut fake = FakeOpenListState::default();
        fake.mkdir_visibility_delay = 2;
        fake.objects
            .insert("/dst".to_string(), object("dst", 0, true));
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
        assert_eq!(
            state.mkdirs,
            vec!["/dst/Show".to_string(), "/dst/Show/Season".to_string()]
        );
        drop(state);
        server.abort();
    }

    #[tokio::test]
    async fn manifest_copy_creates_only_missing_target_directories_top_down() {
        let mut fake = FakeOpenListState::default();
        fake.mkdir_visibility_delay = 2;
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
        client
            .copy_manifest_file(
                "/src",
                "/cmcc/Download/media/2024",
                "Show/E01.mkv",
                300,
            )
            .await
            .unwrap();

        let state = state.lock().unwrap();
        assert_eq!(
            state.mkdirs,
            vec![
                "/cmcc/Download/media".to_string(),
                "/cmcc/Download/media/2024".to_string(),
                "/cmcc/Download/media/2024/Show".to_string(),
            ]
        );
        assert_eq!(
            state.copies,
            vec![(
                "/src/Show".to_string(),
                "/cmcc/Download/media/2024/Show".to_string(),
                "E01.mkv".to_string(),
            )]
        );
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
        assert!(error.contains("大小不匹配"));
        assert!(state.lock().unwrap().copies.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn existing_manifest_file_does_not_submit_copy() {
        let mut fake = FakeOpenListState::default();
        for (path, value) in [
            ("/dst", object("dst", 0, true)),
            ("/src/Show", object("Show", 0, true)),
            ("/dst/Show", object("Show", 0, true)),
            ("/src/Show/E01.mkv", object("E01.mkv", 300, false)),
            ("/dst/Show/E01.mkv", object("E01.mkv", 300, false)),
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
            ManifestFileState::Present
        );
        assert!(state.lock().unwrap().copies.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn manifest_file_removal_refuses_a_directory() {
        let mut fake = FakeOpenListState::default();
        fake.objects
            .insert("/src/Show/E01.mkv".to_string(), object("E01.mkv", 0, true));
        let state = Arc::new(Mutex::new(fake));
        let app = Router::new()
            .route("/api/fs/get", post(fake_get))
            .route("/api/fs/remove", post(fake_remove))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = OpenListClient::new(&format!("http://{address}"), "test-key").unwrap();

        let error = client
            .remove_manifest_file_if_exists("/src", "Show/E01.mkv", 300)
            .await
            .unwrap_err();
        assert!(error.contains("目录"));
        assert!(state.lock().unwrap().removes.is_empty());
        server.abort();
    }
}
