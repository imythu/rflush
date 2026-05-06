use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use futures::stream;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{info, info_span};

use crate::brush::BrushTaskRequest;
use crate::brush::scheduler::BrushScheduler;
use crate::collector::DownloaderSnapshotCollector;
use crate::config::{AppConfig, GlobalConfig, RssConfig, RssSubscription};
use crate::db::{Database, DownloadHistoryRecord, DownloadRunRecord, PaginatedRunRecords};
use crate::download::naming::sanitize_component;
use crate::downloader::DownloaderSpaceStats;
use crate::downloader::factory as downloader_factory;
use crate::engine::DownloadEngine;
use crate::error::AppError;
use crate::history::RunSummary;
use crate::sign_in::scheduler::SignInScheduler;
use crate::site::factory as site_factory;

#[derive(Clone)]
pub struct AppState {
    base_dir: PathBuf,
    db: Database,
    engine: DownloadEngine,
    jobs: Arc<JobRegistry>,
    scheduler: Arc<BrushScheduler>,
    sign_in_scheduler: Arc<SignInScheduler>,
    collector: Arc<DownloaderSnapshotCollector>,
}

struct JobRegistry {
    next_id: AtomicU64,
    jobs: Mutex<HashMap<u64, ManagedJob>>,
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            jobs: Mutex::new(HashMap::new()),
        }
    }
}

#[derive(Clone)]
struct ManagedJob {
    info: JobInfo,
    task_id: Option<i64>,
    shutdown: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Serialize)]
struct JobInfo {
    id: u64,
    scope: String,
    task_id: Option<i64>,
    status: String,
    started_at: String,
    finished_at: Option<String>,
    run_id: Option<i64>,
    summary: Option<RunSummary>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct RunRecordsQuery {
    page: Option<usize>,
    page_size: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct BrushTorrentsQuery {
    page: Option<usize>,
    page_size: Option<usize>,
    keyword: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateRssRequest {
    name: String,
    url: String,
    auto_start: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct TaskBatchRequest {
    ids: Vec<i64>,
    delete_files: Option<bool>,
}

#[derive(Debug, Serialize)]
struct TaskRecordsResponse {
    task: RssSubscription,
    page: usize,
    page_size: usize,
    total_records: usize,
    records: Vec<DownloadHistoryRecord>,
}

#[derive(Debug, Serialize)]
struct BrushTaskTorrentsResponse {
    task: crate::brush::BrushTaskRecord,
    page: usize,
    page_size: usize,
    total_records: usize,
    records: Vec<crate::brush::BrushTorrentRecord>,
}

#[derive(RustEmbed)]
#[folder = "frontend/dist"]
struct FrontendAssets;

impl AppState {
    pub fn new(
        base_dir: PathBuf,
        db: Database,
        engine: DownloadEngine,
        scheduler: Arc<BrushScheduler>,
        sign_in_scheduler: Arc<SignInScheduler>,
        collector: Arc<DownloaderSnapshotCollector>,
    ) -> Self {
        Self {
            base_dir,
            db,
            engine,
            jobs: Arc::new(JobRegistry::default()),
            scheduler,
            sign_in_scheduler,
            collector,
        }
    }

    async fn build_config_for_all(&self) -> Result<AppConfig, AppError> {
        let settings = self.db.get_settings().await?;
        let rss = self
            .db
            .list_rss()
            .await?
            .into_iter()
            .filter(|item| item.enabled)
            .map(|item| RssConfig {
                name: item.name,
                url: item.url,
            })
            .collect();
        Ok(AppConfig {
            global: settings,
            rss,
        })
    }

    async fn build_config_for_task(&self, task: &RssSubscription) -> Result<AppConfig, AppError> {
        Ok(AppConfig {
            global: self.db.get_settings().await?,
            rss: vec![RssConfig {
                name: task.name.clone(),
                url: task.url.clone(),
            }],
        })
    }
}

impl JobRegistry {
    async fn create(&self, scope: String, task_id: Option<i64>, shutdown: Arc<AtomicBool>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let mut jobs = self.jobs.lock().await;
        jobs.insert(
            id,
            ManagedJob {
                info: JobInfo {
                    id,
                    scope,
                    task_id,
                    status: "queued".to_string(),
                    started_at: Utc::now().to_rfc3339(),
                    finished_at: None,
                    run_id: None,
                    summary: None,
                    error: None,
                },
                task_id,
                shutdown,
            },
        );
        id
    }

    async fn get(&self, id: u64) -> Option<JobInfo> {
        let jobs = self.jobs.lock().await;
        jobs.get(&id).map(|job| job.info.clone())
    }

    async fn active_for_task(&self, task_id: i64) -> Option<JobInfo> {
        let jobs = self.jobs.lock().await;
        jobs.values()
            .find(|job| {
                job.task_id == Some(task_id)
                    && matches!(job.info.status.as_str(), "queued" | "running")
            })
            .map(|job| job.info.clone())
    }

    async fn mark_running(&self, id: u64) {
        let mut jobs = self.jobs.lock().await;
        if let Some(job) = jobs.get_mut(&id) {
            job.info.status = "running".to_string();
        }
    }

    async fn mark_completed(&self, id: u64, run_id: i64, summary: RunSummary) {
        let mut jobs = self.jobs.lock().await;
        if let Some(job) = jobs.get_mut(&id) {
            job.info.status = "completed".to_string();
            job.info.finished_at = Some(Utc::now().to_rfc3339());
            job.info.run_id = Some(run_id);
            job.info.summary = Some(summary);
        }
    }

    async fn mark_failed(&self, id: u64, error: String) {
        let mut jobs = self.jobs.lock().await;
        if let Some(job) = jobs.get_mut(&id) {
            job.info.status = "failed".to_string();
            job.info.finished_at = Some(Utc::now().to_rfc3339());
            job.info.error = Some(error);
        }
    }

    async fn mark_paused(&self, id: u64, run_id: Option<i64>, summary: Option<RunSummary>) {
        let mut jobs = self.jobs.lock().await;
        if let Some(job) = jobs.get_mut(&id) {
            job.info.status = "paused".to_string();
            job.info.finished_at = Some(Utc::now().to_rfc3339());
            job.info.run_id = run_id;
            job.info.summary = summary;
        }
    }

    async fn stop_tasks(&self, task_ids: &[i64]) {
        let jobs = self.jobs.lock().await;
        for job in jobs.values() {
            if job
                .task_id
                .is_some_and(|task_id| task_ids.contains(&task_id))
                && matches!(job.info.status.as_str(), "queued" | "running")
            {
                job.shutdown.store(true, Ordering::Relaxed);
            }
        }
    }

    async fn stop_all(&self) {
        let jobs = self.jobs.lock().await;
        for job in jobs.values() {
            if matches!(job.info.status.as_str(), "queued" | "running") {
                job.shutdown.store(true, Ordering::Relaxed);
            }
        }
    }
}

pub async fn serve(
    base_dir: PathBuf,
    addr: SocketAddr,
    db: Database,
    scheduler: Arc<BrushScheduler>,
    sign_in_scheduler: Arc<SignInScheduler>,
    collector: Arc<DownloaderSnapshotCollector>,
) -> Result<(), AppError> {
    let engine = DownloadEngine::new(
        base_dir.clone(),
        Arc::new(crate::net::rate_limiter::SharedRateLimiter::new()),
    );
    let state = AppState::new(
        base_dir,
        db,
        engine,
        scheduler,
        sign_in_scheduler,
        collector,
    );
    let app = app_router(state);
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| AppError::Server {
            message: format!("failed to bind {}: {}", addr, e),
        })?;
    info!("web server listening on http://{}", addr);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                info!("failed to listen for Ctrl+C: {}", error);
            } else {
                info!("Ctrl+C received, shutting down web server");
            }
        })
        .await
        .map_err(|e| AppError::Server {
            message: format!("server exited: {}", e),
        })
}

fn app_router(state: AppState) -> Router {
    Router::new()
        .route("/api/settings", get(get_settings).put(update_settings))
        .route("/api/rss", get(list_rss).post(create_rss))
        .route("/api/rss/{id}", delete(delete_rss))
        .route("/api/tasks/{id}/start", post(start_task))
        .route("/api/tasks/{id}/pause", post(pause_task))
        .route("/api/tasks/{id}/delete", post(delete_task))
        .route("/api/tasks/{id}/records", get(get_task_records))
        .route("/api/tasks/start", post(start_tasks))
        .route("/api/tasks/pause", post(pause_tasks))
        .route("/api/tasks/delete", post(delete_tasks))
        .route("/api/tasks/start-all", post(start_all_tasks))
        .route("/api/tasks/pause-all", post(pause_all_tasks))
        .route("/api/tasks/delete-all", post(delete_all_tasks))
        .route("/api/history", get(get_history))
        .route("/api/runs", get(get_runs))
        .route("/api/runs/{id}/records", get(get_run_records))
        .route("/api/jobs/run-all", post(run_all))
        .route("/api/jobs/run/{id}", post(run_one))
        // 站点管理
        .route("/api/sites", get(list_sites).post(create_site))
        .route("/api/sites/{id}", put(update_site).delete(delete_site))
        .route("/api/sites/{id}/test", post(test_site))
        .route("/api/sites/{id}/stats", get(get_site_stats))
        // 自动签到
        .route(
            "/api/sign-in-tasks",
            get(list_sign_in_tasks).post(create_sign_in_task),
        )
        .route(
            "/api/sign-in-tasks/{id}",
            put(update_sign_in_task).delete(delete_sign_in_task),
        )
        .route("/api/sign-in-tasks/{id}/start", post(start_sign_in_task))
        .route("/api/sign-in-tasks/{id}/stop", post(stop_sign_in_task))
        .route("/api/sign-in-tasks/{id}/run", post(run_sign_in_task_once))
        .route(
            "/api/sign-in-tasks/{id}/probe-1-1-1-1",
            post(probe_sign_in_task_1_1_1_1),
        )
        .route(
            "/api/sign-in-probe-1-1-1-1",
            post(probe_sign_in_form_1_1_1_1),
        )
        .route("/api/sign-in-records", get(list_sign_in_records))
        // 下载器管理
        .route(
            "/api/downloaders",
            get(list_downloaders).post(create_downloader),
        )
        .route(
            "/api/downloaders/{id}",
            put(update_downloader).delete(delete_downloader),
        )
        .route("/api/downloaders/{id}/test", post(test_downloader))
        .route(
            "/api/downloaders/{id}/space",
            get(get_downloader_space_stats),
        )
        // 刷流任务
        .route(
            "/api/brush-tasks",
            get(list_brush_tasks).post(create_brush_task),
        )
        .route(
            "/api/brush-tasks/{id}",
            get(get_brush_task)
                .put(update_brush_task)
                .delete(delete_brush_task),
        )
        .route("/api/brush-tasks/{id}/start", post(start_brush_task))
        .route("/api/brush-tasks/{id}/stop", post(stop_brush_task))
        .route("/api/brush-tasks/{id}/run", post(run_brush_task_once))
        .route(
            "/api/brush-tasks/{id}/torrents",
            get(list_brush_task_torrents),
        )
        .route("/api/system/logs/stream", get(stream_logs))
        // 统计
        .route("/api/stats/overview", get(stats_overview))
        .route("/api/stats/trend", get(stats_trend))
        .route(
            "/api/stats/downloader-speed-trend",
            get(downloader_speed_trend),
        )
        .route("/", get(index))
        .route("/{*path}", get(static_asset))
        .with_state(state)
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
                info_span!(
                    "http",
                    method = %request.method(),
                    path = %request.uri().path(),
                )
            }),
        )
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
}

async fn get_settings(State(state): State<AppState>) -> Result<Json<GlobalConfig>, ApiError> {
    Ok(Json(state.db.get_settings().await?))
}

async fn update_settings(
    State(state): State<AppState>,
    Json(settings): Json<GlobalConfig>,
) -> Result<Json<GlobalConfig>, ApiError> {
    validate_settings(&settings)?;
    state.db.update_settings(&settings).await?;
    crate::logging::update_log_filter(settings.log_level.as_deref())?;
    Ok(Json(settings))
}

async fn list_rss(State(state): State<AppState>) -> Result<Json<Vec<RssSubscription>>, ApiError> {
    Ok(Json(state.db.list_rss().await?))
}

async fn create_rss(
    State(state): State<AppState>,
    Json(payload): Json<CreateRssRequest>,
) -> Result<(StatusCode, Json<RssSubscription>), ApiError> {
    if payload.name.trim().is_empty() || payload.url.trim().is_empty() {
        return Err(ApiError::bad_request("name and url are required"));
    }
    let auto_start = payload.auto_start.unwrap_or(true);
    let rss = state
        .db
        .create_rss(
            RssConfig {
                name: payload.name.trim().to_string(),
                url: payload.url.trim().to_string(),
            },
            auto_start,
        )
        .await?;
    if auto_start {
        spawn_task_job(state.clone(), rss.clone()).await?;
    }
    Ok((StatusCode::CREATED, Json(rss)))
}

async fn delete_rss(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let deleted = state.db.delete_rss(id).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("rss subscription not found"))
    }
}

async fn get_task_records(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<RunRecordsQuery>,
) -> Result<Json<TaskRecordsResponse>, ApiError> {
    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(10);
    let task = state
        .db
        .get_rss(id)
        .await?
        .ok_or_else(|| ApiError::not_found("task not found"))?;
    let records = state.db.list_task_records(id, page, page_size).await?;
    let total_records = state.db.count_task_records(id).await?;
    Ok(Json(TaskRecordsResponse {
        task,
        page,
        page_size,
        total_records,
        records,
    }))
}

async fn start_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<JobInfo>, ApiError> {
    let task = state
        .db
        .get_rss(id)
        .await?
        .ok_or_else(|| ApiError::not_found("task not found"))?;
    state.db.update_rss_enabled(&[id], true).await?;
    Ok(Json(spawn_task_job(state, task).await?))
}

async fn pause_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state
        .db
        .get_rss(id)
        .await?
        .ok_or_else(|| ApiError::not_found("task not found"))?;
    state.db.update_rss_enabled(&[id], false).await?;
    state.jobs.stop_tasks(&[id]).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<TaskBatchRequest>,
) -> Result<StatusCode, ApiError> {
    let task = state
        .db
        .get_rss(id)
        .await?
        .ok_or_else(|| ApiError::not_found("task not found"))?;
    delete_tasks_inner(&state, vec![task], payload.delete_files.unwrap_or(false)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_tasks(
    State(state): State<AppState>,
    Json(payload): Json<TaskBatchRequest>,
) -> Result<Json<Vec<JobInfo>>, ApiError> {
    if payload.ids.is_empty() {
        return Err(ApiError::bad_request("ids are required"));
    }
    state.db.update_rss_enabled(&payload.ids, true).await?;
    let mut jobs = Vec::new();
    for id in payload.ids {
        if let Some(task) = state.db.get_rss(id).await? {
            jobs.push(spawn_task_job(state.clone(), task).await?);
        }
    }
    Ok(Json(jobs))
}

async fn pause_tasks(
    State(state): State<AppState>,
    Json(payload): Json<TaskBatchRequest>,
) -> Result<StatusCode, ApiError> {
    if payload.ids.is_empty() {
        return Err(ApiError::bad_request("ids are required"));
    }
    state.db.update_rss_enabled(&payload.ids, false).await?;
    state.jobs.stop_tasks(&payload.ids).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_tasks(
    State(state): State<AppState>,
    Json(payload): Json<TaskBatchRequest>,
) -> Result<StatusCode, ApiError> {
    if payload.ids.is_empty() {
        return Err(ApiError::bad_request("ids are required"));
    }
    let mut tasks = Vec::new();
    for id in payload.ids {
        if let Some(task) = state.db.get_rss(id).await? {
            tasks.push(task);
        }
    }
    delete_tasks_inner(&state, tasks, payload.delete_files.unwrap_or(false)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_all_tasks(State(state): State<AppState>) -> Result<Json<Vec<JobInfo>>, ApiError> {
    let tasks = state.db.list_rss().await?;
    if tasks.is_empty() {
        return Err(ApiError::bad_request("no tasks configured"));
    }
    state.db.set_all_rss_enabled(true).await?;
    let mut jobs = Vec::new();
    for task in tasks {
        jobs.push(spawn_task_job(state.clone(), task).await?);
    }
    Ok(Json(jobs))
}

async fn pause_all_tasks(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    state.db.set_all_rss_enabled(false).await?;
    state.jobs.stop_all().await;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_all_tasks(
    State(state): State<AppState>,
    Json(payload): Json<TaskBatchRequest>,
) -> Result<StatusCode, ApiError> {
    let tasks = state.db.list_rss().await?;
    delete_tasks_inner(&state, tasks, payload.delete_files.unwrap_or(false)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_history(
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<DownloadHistoryRecord>>, ApiError> {
    let limit = query.limit.unwrap_or(200).min(1000);
    Ok(Json(state.db.list_history(limit).await?))
}

async fn get_runs(
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<DownloadRunRecord>>, ApiError> {
    let limit = query.limit.unwrap_or(100).min(500);
    Ok(Json(state.db.list_runs(limit).await?))
}

async fn get_run_records(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<RunRecordsQuery>,
) -> Result<Json<PaginatedRunRecords>, ApiError> {
    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(20);
    let records = state
        .db
        .list_run_records(id, page, page_size)
        .await?
        .ok_or_else(|| ApiError::not_found("run not found"))?;
    Ok(Json(records))
}

async fn run_all(State(state): State<AppState>) -> Result<Json<JobInfo>, ApiError> {
    let config = state.build_config_for_all().await?;
    if config.rss.is_empty() {
        return Err(ApiError::bad_request("no RSS subscriptions configured"));
    }
    let job_id = spawn_job(state.clone(), "all".to_string(), None, config).await;
    let job = state
        .jobs
        .get(job_id)
        .await
        .ok_or_else(|| ApiError::internal("job not found after enqueue"))?;
    Ok(Json(job))
}

async fn run_one(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<JobInfo>, ApiError> {
    let task = state
        .db
        .get_rss(id)
        .await?
        .ok_or_else(|| ApiError::not_found("rss subscription not found"))?;
    Ok(Json(spawn_task_job(state, task).await?))
}

async fn spawn_task_job(state: AppState, task: RssSubscription) -> Result<JobInfo, ApiError> {
    if let Some(job) = state.jobs.active_for_task(task.id).await {
        return Ok(job);
    }

    let config = state.build_config_for_task(&task).await?;
    let job_id = spawn_job(state.clone(), task.name.clone(), Some(task.id), config).await;
    state
        .jobs
        .get(job_id)
        .await
        .ok_or_else(|| ApiError::internal("job not found after enqueue"))
}

async fn spawn_job(state: AppState, scope: String, task_id: Option<i64>, config: AppConfig) -> u64 {
    let shutdown = Arc::new(AtomicBool::new(false));
    let job_id = state.jobs.create(scope, task_id, shutdown.clone()).await;
    tokio::spawn(async move {
        state.jobs.mark_running(job_id).await;
        match state
            .engine
            .run_with_shutdown(config, shutdown.clone())
            .await
        {
            Ok(history) => match state
                .db
                .save_history(
                    &history,
                    task_id,
                    history.rss.first().map(|rss| rss.name.as_str()),
                )
                .await
            {
                Ok(run_id) => {
                    state
                        .jobs
                        .mark_completed(job_id, run_id, history.summary.clone())
                        .await
                }
                Err(error) => state.jobs.mark_failed(job_id, error.to_string()).await,
            },
            Err(error) => state.jobs.mark_failed(job_id, error.to_string()).await,
        }

        if shutdown.load(Ordering::Relaxed) {
            let run_id = state.jobs.get(job_id).await.and_then(|job| job.run_id);
            let summary = state.jobs.get(job_id).await.and_then(|job| job.summary);
            state.jobs.mark_paused(job_id, run_id, summary).await;
        }
    });
    job_id
}

async fn delete_tasks_inner(
    state: &AppState,
    tasks: Vec<RssSubscription>,
    delete_files: bool,
) -> Result<(), ApiError> {
    if tasks.is_empty() {
        return Ok(());
    }

    let ids = tasks.iter().map(|task| task.id).collect::<Vec<_>>();
    state.db.update_rss_enabled(&ids, false).await?;
    state.jobs.stop_tasks(&ids).await;

    if delete_files {
        for task in &tasks {
            let task_dir = state.base_dir.join(sanitize_component(&task.name));
            if tokio::fs::try_exists(&task_dir)
                .await
                .map_err(|source| AppError::ReadDir {
                    path: task_dir.display().to_string(),
                    source,
                })?
            {
                tokio::fs::remove_dir_all(&task_dir)
                    .await
                    .map_err(|source| AppError::RemovePath {
                        path: task_dir.display().to_string(),
                        source,
                    })?;
            }
        }
        state.db.mark_task_records_deleted(&ids).await?;
    }

    state.db.delete_rss_batch(&ids).await?;
    Ok(())
}

async fn index() -> impl IntoResponse {
    serve_asset("index.html")
}

async fn static_asset(Path(path): Path<String>) -> impl IntoResponse {
    if path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve_asset(&path)
}

fn serve_asset(path: &str) -> Response {
    let requested = if path.is_empty() { "index.html" } else { path };
    let asset = FrontendAssets::get(requested).or_else(|| FrontendAssets::get("index.html"));
    let Some(asset) = asset else {
        return Html("<h1>Frontend not built</h1><p>Run the frontend build first.</p>")
            .into_response();
    };
    let mime = mime_guess::from_path(requested).first_or_octet_stream();
    let mut response = asset.data.into_owned().into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response
}

// ========== Sites API ==========

#[derive(Debug, Deserialize)]
struct CreateSiteRequest {
    name: String,
    site_type: String,
    base_url: String,
    auth_config: serde_json::Value,
}

async fn list_sites(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::site::SiteRecord>>, ApiError> {
    Ok(Json(state.db.list_sites().await?))
}

async fn create_site(
    State(state): State<AppState>,
    Json(body): Json<CreateSiteRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.name.is_empty() || body.site_type.is_empty() || body.base_url.is_empty() {
        return Err(ApiError::bad_request("名称、站点类型和基础URL不能为空"));
    }
    let auth_str = serde_json::to_string(&body.auth_config)
        .map_err(|e| ApiError::bad_request(format!("认证配置序列化失败: {}", e)))?;
    let id = state
        .db
        .create_site(&body.name, &body.site_type, &body.base_url, &auth_str)
        .await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_site(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<CreateSiteRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let auth_str = serde_json::to_string(&body.auth_config)
        .map_err(|e| ApiError::bad_request(format!("认证配置序列化失败: {}", e)))?;
    state
        .db
        .update_site(id, &body.name, &body.site_type, &body.base_url, &auth_str)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_site(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.delete_site(id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn test_site(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::site::SiteTestResult>, ApiError> {
    let site = state
        .db
        .get_site(id)
        .await?
        .ok_or_else(|| ApiError::not_found("站点不存在"))?;
    let adapter = site_factory::create_adapter(&site).map_err(ApiError::bad_request)?;
    let result = adapter
        .test_connection()
        .await
        .map_err(|e| ApiError::internal(e))?;
    Ok(Json(result))
}

async fn get_site_stats(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::site::UserStats>, ApiError> {
    let site = state
        .db
        .get_site(id)
        .await?
        .ok_or_else(|| ApiError::not_found("站点不存在"))?;
    let adapter = site_factory::create_adapter(&site).map_err(ApiError::bad_request)?;
    let stats = adapter
        .get_user_stats()
        .await
        .map_err(|e| ApiError::internal(e))?;
    Ok(Json(stats))
}

// ========== Sign-in API ==========

#[derive(Debug, Deserialize)]
struct SignInRecordsQuery {
    task_id: Option<i64>,
    limit: Option<usize>,
}

async fn list_sign_in_tasks(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::sign_in::SignInTaskRecord>>, ApiError> {
    Ok(Json(state.db.list_sign_in_tasks().await?))
}

async fn create_sign_in_task(
    State(state): State<AppState>,
    Json(mut body): Json<crate::sign_in::SignInTaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_sign_in_task(&state, &mut body).await?;
    let id = state.db.create_sign_in_task(&body).await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_sign_in_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(mut body): Json<crate::sign_in::SignInTaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_sign_in_task(&state, &mut body).await?;
    state.db.update_sign_in_task(id, &body).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_sign_in_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.sign_in_scheduler.stop_task(id).await;
    state.db.delete_sign_in_task(id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn start_sign_in_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.set_sign_in_task_enabled(id, true).await?;
    state
        .sign_in_scheduler
        .trigger_task(id)
        .await
        .map_err(map_sign_in_trigger_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn stop_sign_in_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.set_sign_in_task_enabled(id, false).await?;
    state.sign_in_scheduler.stop_task(id).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn run_sign_in_task_once(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .sign_in_scheduler
        .trigger_task(id)
        .await
        .map_err(map_sign_in_trigger_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn probe_sign_in_task_1_1_1_1(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::sign_in::LightpandaProbeResult>, ApiError> {
    let task = state
        .db
        .get_sign_in_task(id)
        .await?
        .ok_or_else(|| ApiError::not_found("签到任务不存在"))?;
    let result = crate::sign_in::probe_lightpanda_1_1_1_1(task)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(result))
}

async fn probe_sign_in_form_1_1_1_1(
    Json(mut body): Json<crate::sign_in::SignInTaskRequest>,
) -> Result<Json<crate::sign_in::LightpandaProbeResult>, ApiError> {
    body.lightpanda_endpoint = body
        .lightpanda_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    body.lightpanda_token = body.lightpanda_token.trim().to_string();
    if body.lightpanda_endpoint.is_none() && body.lightpanda_token.is_empty() {
        return Err(ApiError::bad_request("Lightpanda endpoint 不能为空"));
    }
    let result = crate::sign_in::probe_lightpanda_request_1_1_1_1(body)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(result))
}

async fn list_sign_in_records(
    State(state): State<AppState>,
    Query(query): Query<SignInRecordsQuery>,
) -> Result<Json<Vec<crate::sign_in::SignInRecord>>, ApiError> {
    Ok(Json(
        state
            .db
            .list_sign_in_records(query.task_id, query.limit.unwrap_or(100))
            .await?,
    ))
}

async fn validate_sign_in_task(
    state: &AppState,
    body: &mut crate::sign_in::SignInTaskRequest,
) -> Result<(), ApiError> {
    body.name = body.name.trim().to_string();
    body.cron_expression = normalize_cron(&body.cron_expression);
    body.lightpanda_token = body.lightpanda_token.trim().to_string();
    body.lightpanda_endpoint = body
        .lightpanda_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    body.lightpanda_region = Some(
        body.lightpanda_region
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("euwest")
            .to_string(),
    );
    body.browser = Some(
        body.browser
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("lightpanda")
            .to_string(),
    );
    body.proxy = Some(
        body.proxy
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("fast_dc")
            .to_string(),
    );
    body.country = body
        .country
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    if body.name.is_empty() {
        return Err(ApiError::bad_request("名称不能为空"));
    }
    if body.lightpanda_endpoint.is_none() && body.lightpanda_token.is_empty() {
        return Err(ApiError::bad_request("Lightpanda Token 不能为空"));
    }
    body.cron_expression
        .parse::<cron::Schedule>()
        .map_err(|e| ApiError::bad_request(format!("无效的cron表达式: {}", e)))?;
    let site = state
        .db
        .get_site(body.site_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("所选站点不存在"))?;
    if site.site_type != "nexusphp" && site.site_type != "nexus_php" {
        return Err(ApiError::bad_request("自动签到目前仅支持 NexusPHP 站点"));
    }
    Ok(())
}

// ========== Downloaders API ==========

#[derive(Debug, Deserialize)]
struct CreateDownloaderRequest {
    name: String,
    downloader_type: String,
    url: String,
    username: Option<String>,
    password: Option<String>,
}

async fn list_downloaders(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::downloader::DownloaderRecord>>, ApiError> {
    Ok(Json(state.db.list_downloaders().await?))
}

async fn create_downloader(
    State(state): State<AppState>,
    Json(body): Json<CreateDownloaderRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.name.is_empty() || body.url.is_empty() {
        return Err(ApiError::bad_request("名称和URL不能为空"));
    }
    let id = state
        .db
        .create_downloader(
            &body.name,
            &body.downloader_type,
            &body.url,
            body.username.as_deref().unwrap_or(""),
            body.password.as_deref().unwrap_or(""),
        )
        .await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_downloader(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<CreateDownloaderRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .db
        .update_downloader(
            id,
            &body.name,
            &body.downloader_type,
            &body.url,
            body.username.as_deref().unwrap_or(""),
            body.password.as_deref().unwrap_or(""),
        )
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_downloader(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.delete_downloader(id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn test_downloader(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::downloader::DownloaderTestResult>, ApiError> {
    let dl = state
        .db
        .get_downloader(id)
        .await?
        .ok_or_else(|| ApiError::not_found("下载器不存在"))?;
    let client = downloader_factory::create_client(&dl).map_err(ApiError::bad_request)?;
    let result = client
        .test_connection()
        .await
        .map_err(|e| ApiError::internal(e))?;
    Ok(Json(result))
}

async fn get_downloader_space_stats(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<DownloaderSpaceStats>, ApiError> {
    let dl = state
        .db
        .get_downloader(id)
        .await?
        .ok_or_else(|| ApiError::not_found("下载器不存在"))?;
    let client = downloader_factory::create_client(&dl).map_err(ApiError::bad_request)?;
    let torrents = state
        .collector
        .get_all_torrents(&dl)
        .await
        .map_err(ApiError::internal)?;
    let stats = client
        .get_effective_free_space(None, &torrents)
        .await
        .map_err(ApiError::internal)?;

    Ok(Json(stats))
}

// ========== Brush Tasks API ==========

async fn list_brush_tasks(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::brush::BrushTaskRecord>>, ApiError> {
    Ok(Json(state.db.list_brush_tasks().await?))
}

async fn get_brush_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::brush::BrushTaskRecord>, ApiError> {
    let task = state
        .db
        .get_brush_task(id)
        .await?
        .ok_or_else(|| ApiError::not_found("刷流任务不存在"))?;
    Ok(Json(task))
}

/// 规范化 cron 表达式：标准5字段自动补秒字段
fn normalize_cron(expr: &str) -> String {
    let fields: Vec<&str> = expr.trim().split_whitespace().collect();
    if fields.len() == 5 {
        format!("0 {}", expr.trim())
    } else {
        expr.trim().to_string()
    }
}

async fn create_brush_task(
    State(state): State<AppState>,
    Json(mut body): Json<BrushTaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.name.is_empty()
        || body.cron_expression.is_empty()
        || body.tag.is_empty()
        || body.rss_url.is_empty()
    {
        return Err(ApiError::bad_request(
            "名称、cron表达式、标签和RSS地址不能为空",
        ));
    }
    let site_id = body
        .site_id
        .ok_or_else(|| ApiError::bad_request("必须选择一个具体站点"))?;
    state
        .db
        .get_site(site_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("所选站点不存在"))?;
    body.cron_expression = normalize_cron(&body.cron_expression);
    body.cron_expression
        .parse::<cron::Schedule>()
        .map_err(|e| ApiError::bad_request(format!("无效的cron表达式: {}", e)))?;
    let id = state.db.create_brush_task(&body).await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_brush_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(mut body): Json<BrushTaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site_id = body
        .site_id
        .ok_or_else(|| ApiError::bad_request("必须选择一个具体站点"))?;
    state
        .db
        .get_site(site_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("所选站点不存在"))?;
    body.cron_expression = normalize_cron(&body.cron_expression);
    body.cron_expression
        .parse::<cron::Schedule>()
        .map_err(|e| ApiError::bad_request(format!("无效的cron表达式: {}", e)))?;
    state.db.update_brush_task(id, &body).await?;
    state
        .scheduler
        .refresh_task_config(id)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_brush_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.scheduler.stop_task(id).await;
    state.db.delete_brush_task(id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn start_brush_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.set_brush_task_enabled(id, true).await?;
    state
        .scheduler
        .trigger_task(id)
        .await
        .map_err(map_brush_trigger_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn stop_brush_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.set_brush_task_enabled(id, false).await?;
    state.scheduler.stop_task(id).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn run_brush_task_once(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .scheduler
        .trigger_task(id)
        .await
        .map_err(map_brush_trigger_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn list_brush_task_torrents(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<BrushTorrentsQuery>,
) -> Result<Json<BrushTaskTorrentsResponse>, ApiError> {
    let task = state
        .db
        .get_brush_task(id)
        .await?
        .ok_or_else(|| ApiError::not_found("刷流任务不存在"))?;
    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(20);
    let mut torrents = state
        .db
        .list_brush_task_torrents(id, page, page_size, query.keyword.as_deref())
        .await?;

    if let Some(downloader) = state.db.get_downloader(task.downloader_id).await? {
        if let Ok(live_torrents) = state
            .collector
            .get_tagged_torrents(&downloader, &task.tag)
            .await
        {
            for record in &mut torrents.records {
                if let Some(live) = find_live_brush_torrent(record, &live_torrents) {
                    apply_live_torrent(record, live);
                }
            }
        }
    }

    for record in &mut torrents.records {
        if record.torrent_id.is_none() && !looks_like_info_hash(&record.torrent_hash) {
            record.torrent_id = Some(record.torrent_hash.clone());
            record.torrent_hash.clear();
        }
    }

    Ok(Json(BrushTaskTorrentsResponse {
        task,
        page: torrents.page,
        page_size: torrents.page_size,
        total_records: torrents.total_records,
        records: torrents.records,
    }))
}

async fn stream_logs() -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>
{
    let receiver = crate::logging::subscribe_logs();
    let stream = stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(line) => {
                    let payload = serde_json::json!({
                        "encoded_line": urlencoding::encode(&line).into_owned()
                    })
                    .to_string();
                    let event = Event::default().event("log").data(payload);
                    return Some((Ok(event), receiver));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return None;
                }
            }
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ========== Stats API ==========

#[derive(Debug, Deserialize)]
struct StatsQuery {
    task_id: Option<i64>,
    since: Option<String>,
    until: Option<String>,
    hours: Option<i64>,
}

#[derive(Debug, Serialize)]
struct StatsOverview {
    tasks: Vec<TaskOverview>,
}

#[derive(Debug, Serialize)]
struct TaskOverview {
    task_id: i64,
    task_name: String,
    total_uploaded: i64,
    total_downloaded: i64,
    torrent_count: i64,
    enabled: bool,
}

async fn stats_overview(State(state): State<AppState>) -> Result<Json<StatsOverview>, ApiError> {
    let tasks = state.db.list_brush_tasks().await?;
    let mut overviews = Vec::new();
    for task in &tasks {
        // 获取最新的统计快照
        let now = Utc::now().to_rfc3339();
        let since = (Utc::now() - chrono::Duration::seconds(120)).to_rfc3339();
        let snapshots = state
            .db
            .get_task_stats_snapshots(Some(task.id), &since, &now)
            .await?;
        let latest = snapshots.last();
        let (total_uploaded, total_downloaded, historical_torrent_count) =
            state.db.get_brush_task_transfer_totals(task.id).await?;
        overviews.push(TaskOverview {
            task_id: task.id,
            task_name: task.name.clone(),
            total_uploaded,
            total_downloaded,
            torrent_count: latest
                .map(|s| s.torrent_count)
                .unwrap_or(historical_torrent_count),
            enabled: task.enabled,
        });
    }
    Ok(Json(StatsOverview { tasks: overviews }))
}

async fn stats_trend(
    State(state): State<AppState>,
    Query(q): Query<StatsQuery>,
) -> Result<Json<Vec<crate::stats::TaskStatsSnapshot>>, ApiError> {
    let hours = q.hours.unwrap_or(24);
    let until = q.until.unwrap_or_else(|| Utc::now().to_rfc3339());
    let since = q.since.unwrap_or_else(|| {
        // Stats are sampled periodically, so add a small grace window to avoid
        // dropping the latest bucket on exact boundary cuts like "last 1h".
        (Utc::now() - chrono::Duration::hours(hours) - chrono::Duration::minutes(2)).to_rfc3339()
    });
    let data = state
        .db
        .get_task_stats_snapshots(q.task_id, &since, &until)
        .await?;
    Ok(Json(data))
}

#[derive(Debug, Deserialize)]
struct DownloaderStatsQuery {
    downloader_id: Option<i64>,
    since: Option<String>,
    until: Option<String>,
    hours: Option<i64>,
}

async fn downloader_speed_trend(
    State(state): State<AppState>,
    Query(q): Query<DownloaderStatsQuery>,
) -> Result<Json<Vec<crate::stats::DownloaderSpeedSnapshot>>, ApiError> {
    let hours = q.hours.unwrap_or(24);
    let until = q.until.unwrap_or_else(|| Utc::now().to_rfc3339());
    let since = q.since.unwrap_or_else(|| {
        (Utc::now() - chrono::Duration::hours(hours) - chrono::Duration::minutes(2)).to_rfc3339()
    });
    let data = state
        .db
        .get_downloader_speed_snapshots(q.downloader_id, &since, &until)
        .await?;
    Ok(Json(data))
}

fn validate_settings(settings: &GlobalConfig) -> Result<(), ApiError> {
    const ALLOWED_LOG_LEVELS: &[&str] = &["trace", "debug", "info", "warn", "error"];

    if settings.download_rate_limit.requests == 0 {
        return Err(ApiError::bad_request(
            "download_rate_limit.requests must be >= 1",
        ));
    }
    if settings.download_rate_limit.interval == 0 {
        return Err(ApiError::bad_request(
            "download_rate_limit.interval must be >= 1",
        ));
    }
    if settings.retry_interval_secs == 0 {
        return Err(ApiError::bad_request("retry_interval_secs must be >= 1"));
    }
    if let Some(log_level) = settings.log_level.as_deref() {
        if !ALLOWED_LOG_LEVELS.contains(&log_level) {
            return Err(ApiError::bad_request(
                "log_level must be one of: trace, debug, info, warn, error",
            ));
        }
    }
    Ok(())
}

fn find_live_brush_torrent<'a>(
    record: &crate::brush::BrushTorrentRecord,
    live_torrents: &'a [crate::downloader::TorrentInfo],
) -> Option<&'a crate::downloader::TorrentInfo> {
    live_torrents
        .iter()
        .find(|torrent| torrent.hash.eq_ignore_ascii_case(&record.torrent_hash))
        .or_else(|| {
            live_torrents
                .iter()
                .find(|torrent| torrent.name == record.torrent_name)
        })
}

fn apply_live_torrent(
    record: &mut crate::brush::BrushTorrentRecord,
    live: &crate::downloader::TorrentInfo,
) {
    record.status = live.state.clone();
    record.remove_reason = None;
    record.removed_at = None;
    record.torrent_hash = live.hash.clone();
    record.uploaded_bytes = live.uploaded;
    record.downloaded_bytes = live.downloaded;
    record.download_duration_secs = live.time_active.max(0);
    record.avg_upload_speed = average_upload_speed(live.uploaded, live.time_active);
    record.ratio = calculate_ratio(live.uploaded, live.downloaded, live.ratio);
}

fn average_upload_speed(uploaded_bytes: i64, duration_secs: i64) -> f64 {
    if duration_secs <= 0 {
        0.0
    } else {
        uploaded_bytes as f64 / duration_secs as f64
    }
}

fn calculate_ratio(uploaded_bytes: i64, downloaded_bytes: i64, fallback: f64) -> f64 {
    if downloaded_bytes > 0 {
        uploaded_bytes as f64 / downloaded_bytes as f64
    } else if uploaded_bytes > 0 {
        fallback.max(0.0)
    } else {
        0.0
    }
}

fn looks_like_info_hash(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn map_brush_trigger_error(message: String) -> ApiError {
    match message.as_str() {
        "任务不存在" => ApiError::not_found(message),
        "任务正在运行中" => ApiError::conflict(message),
        _ => ApiError::internal(message),
    }
}

fn map_sign_in_trigger_error(message: String) -> ApiError {
    match message.as_str() {
        "签到任务不存在" => ApiError::not_found(message),
        "签到任务正在运行中" => ApiError::conflict(message),
        _ => ApiError::internal(message),
    }
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }
}

impl From<AppError> for ApiError {
    fn from(value: AppError) -> Self {
        match value {
            AppError::InvalidConfig { message } | AppError::Database { message } => Self {
                status: StatusCode::BAD_REQUEST,
                message,
            },
            other => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: other.to_string(),
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}
