use std::path::Path;
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Deserialize)]
struct CopyResult {
    #[serde(default)]
    tasks: Vec<OpenListTask>,
}

#[derive(Debug, Deserialize)]
struct ListResult {
    #[serde(default)]
    content: Option<Vec<OpenListObject>>,
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
struct ListRequest<'a> {
    path: &'a str,
    password: &'a str,
    page: i64,
    per_page: i64,
    refresh: bool,
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
                    // Merge preserves unrelated files in a shared top-level directory.
                    merge: true,
                },
            )
            .await?;
        Ok(data.tasks)
    }

    pub async fn task_info(&self, task_id: &str) -> Result<OpenListTask, String> {
        let path = format!("/api/task/copy/info?tid={}", urlencoding::encode(task_id));
        self.post(&path, &()).await
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

    pub async fn remove(&self, dir: &str, name: &str) -> Result<(), String> {
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            return Err("OpenList 删除名称必须是单个文件或目录名".to_string());
        }
        let names = [name];
        self.post_empty("/api/fs/remove", &RemoveRequest { dir, names: &names })
            .await
    }

    pub async fn remove_if_exists(&self, dir: &str, name: &str) -> Result<(), String> {
        let path = if dir == "/" {
            format!("/{name}")
        } else {
            format!("{}/{name}", dir.trim_end_matches('/'))
        };
        match self.stat(&path).await {
            Ok(_) => self.remove(dir, name).await,
            Err(error) if is_not_found_error(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub async fn remove_empty_directory_if_exists(&self, path: &str) -> Result<(), String> {
        match self.stat(path).await {
            Ok(object) if !object.is_dir => return Ok(()),
            Ok(_) => {}
            Err(error) if is_not_found_error(&error) => return Ok(()),
            Err(error) => return Err(error),
        }
        let listed: ListResult = self
            .post(
                "/api/fs/list",
                &ListRequest {
                    path,
                    password: "",
                    page: 1,
                    per_page: 1,
                    refresh: true,
                },
            )
            .await?;
        if listed.content.unwrap_or_default().is_empty() {
            let (parent, name) = remote_parent_and_name(path)?;
            self.remove(&parent, &name).await?;
        }
        Ok(())
    }

    async fn post<B, T>(&self, path: &str, body: &B) -> Result<T, String>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        self.post_envelope(path, body)
            .await?
            .ok_or_else(|| "OpenList 成功响应缺少 data".to_string())
    }

    async fn post_empty<B>(&self, path: &str, body: &B) -> Result<(), String>
    where
        B: Serialize + ?Sized,
    {
        self.post_envelope::<_, serde_json::Value>(path, body)
            .await
            .map(|_| ())
    }

    async fn post_envelope<B, T>(&self, path: &str, body: &B) -> Result<Option<T>, String>
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
            .map_err(|e| format!("请求 OpenList 失败: {e}"))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("读取 OpenList 响应失败: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "OpenList HTTP 错误 {}: {}",
                status,
                String::from_utf8_lossy(&bytes)
            ));
        }
        let envelope: Envelope<T> =
            serde_json::from_slice(&bytes).map_err(|e| format!("解析 OpenList 响应失败: {e}"))?;
        if envelope.code != StatusCode::OK.as_u16() as i64 {
            return Err(format!(
                "OpenList 业务错误 {}: {}",
                envelope.code, envelope.message
            ));
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

fn remote_parent_and_name(path: &str) -> Result<(String, String), String> {
    let path = path.trim_end_matches('/');
    let (parent, name) = path
        .rsplit_once('/')
        .ok_or_else(|| format!("OpenList 路径缺少父目录: {path}"))?;
    if !valid_child_name(name) {
        return Err(format!("OpenList 路径名称无效: {name}"));
    }
    Ok((
        if parent.is_empty() { "/" } else { parent }.to_string(),
        name.to_string(),
    ))
}

fn is_not_found_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("not found")
        || error.contains("object not found")
        || error.contains("file does not exist")
}

#[cfg(test)]
mod tests {
    use super::{OpenListTask, is_not_found_error, valid_child_name};

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
}
