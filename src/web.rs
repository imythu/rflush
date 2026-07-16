use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
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
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{info, info_span, warn};

use crate::brush::BrushTaskRequest;
use crate::brush::scheduler::BrushScheduler;
use crate::collector::DownloaderSnapshotCollector;
use crate::config::{AppConfig, GlobalConfig, RssConfig, RssSubscription};
use crate::db::{Database, DownloadHistoryRecord, DownloadRunRecord, PaginatedRunRecords};
use crate::downloader::DownloaderClientPool;
use crate::downloader::DownloaderSpaceStats;
use crate::engine::DownloadEngine;
use crate::error::AppError;
use crate::history::RunSummary;
use crate::indexer::IndexerError;
use crate::media::domain::{MediaTarget, ReleaseInfo, ReleaseParser};
use crate::media::models::{
    MediaDownloadRecord, MediaSettings, QualityProfileRecord, QualityProfileRequest,
    SubscriptionRecord, UpdateSubscription,
};
use crate::media::scheduler::MediaScheduler;
use crate::media::service::{
    CreateSubscriptionRequest, MediaService, MediaServiceError, QueueDownloadRequest,
    ResourceSearchRequest, ResourceSearchResponse, SubscriptionRunResult, SubscriptionRunSnapshot,
};
use crate::media::tmdb::{TmdbDetails, TmdbError, TmdbMedia, TmdbMediaType, TmdbSeason};
use crate::monitor::{SystemMonitor, SystemSnapshot, SystemSnapshotRecord};
use crate::net::client_factory;
use crate::sign_in::scheduler::SignInScheduler;
use crate::site::factory as site_factory;
use crate::site::{SiteAuth, SiteStatsRecord, SiteType, SiteWithStats};
use crate::site_stats::SiteStatsRefresher;
use crate::tag_rule::scheduler::TagRuleScheduler;

#[derive(Clone)]
pub struct AppState {
    db: Database,
    engine: DownloadEngine,
    jobs: Arc<JobRegistry>,
    scheduler: Arc<BrushScheduler>,
    sign_in_scheduler: Arc<SignInScheduler>,
    site_stats_refresher: Arc<SiteStatsRefresher>,
    collector: Arc<DownloaderSnapshotCollector>,
    pool: Arc<DownloaderClientPool>,
    media: Arc<MediaService>,
    media_scheduler: Arc<MediaScheduler>,
    monitor: Arc<SystemMonitor>,
    tag_rule_scheduler: Arc<TagRuleScheduler>,
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
    downloader_id: Option<i64>,
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
        db: Database,
        engine: DownloadEngine,
        scheduler: Arc<BrushScheduler>,
        sign_in_scheduler: Arc<SignInScheduler>,
        site_stats_refresher: Arc<SiteStatsRefresher>,
        collector: Arc<DownloaderSnapshotCollector>,
        pool: Arc<DownloaderClientPool>,
        media: Arc<MediaService>,
        media_scheduler: Arc<MediaScheduler>,
        monitor: Arc<SystemMonitor>,
        tag_rule_scheduler: Arc<TagRuleScheduler>,
    ) -> Self {
        Self {
            db,
            engine,
            jobs: Arc::new(JobRegistry::default()),
            scheduler,
            sign_in_scheduler,
            site_stats_refresher,
            collector,
            pool,
            media,
            media_scheduler,
            monitor,
            tag_rule_scheduler,
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
                downloader_id: item.downloader_id,
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
                downloader_id: task.downloader_id,
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
    listener: TcpListener,
    db: Database,
    scheduler: Arc<BrushScheduler>,
    sign_in_scheduler: Arc<SignInScheduler>,
    site_stats_refresher: Arc<SiteStatsRefresher>,
    collector: Arc<DownloaderSnapshotCollector>,
    pool: Arc<DownloaderClientPool>,
    media: Arc<MediaService>,
    media_scheduler: Arc<MediaScheduler>,
    rate_limiter: Arc<crate::net::rate_limiter::SharedRateLimiter>,
    monitor: Arc<SystemMonitor>,
    tag_rule_scheduler: Arc<TagRuleScheduler>,
) -> Result<(), AppError> {
    let addr = listener.local_addr().map_err(|error| AppError::Server {
        message: format!("failed to read bound web server address: {error}"),
    })?;
    let engine = DownloadEngine::new(rate_limiter);
    let state = AppState::new(
        db,
        engine,
        scheduler,
        sign_in_scheduler,
        site_stats_refresher,
        collector,
        pool,
        media,
        media_scheduler,
        monitor,
        tag_rule_scheduler,
    );
    let app = app_router(state);
    if !addr.ip().is_loopback() {
        warn!(
            "web server is listening on a non-loopback address; place rflush behind an authenticated reverse proxy and restrict network access"
        );
    }
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
    let media = Arc::clone(&state.media);
    let media_scheduler = Arc::clone(&state.media_scheduler);
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
        .route("/api/sites/stats-overview", get(get_sites_stats_overview))
        .route("/api/sites/{id}", put(update_site).delete(delete_site))
        .route("/api/sites/{id}/test", post(test_site))
        .route("/api/sites/{id}/stats", get(get_site_stats))
        .route("/api/proxy/test", post(test_proxy))
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
        .route(
            "/api/downloaders/{id}/default-path",
            get(get_downloader_default_path),
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
        .route("/api/stats/daily-transfer", get(daily_transfer))
        // 系统监控
        .route("/api/system/stats", get(get_system_stats))
        .route("/api/system/stats/history", get(get_system_stats_history))
        // 标签规则
        .route("/api/tag-rules", get(list_tag_rules).post(create_tag_rule))
        .route(
            "/api/tag-rules/{id}",
            get(get_tag_rule)
                .put(update_tag_rule)
                .delete(delete_tag_rule),
        )
        .route("/api/tag-rules/scan", post(scan_tag_rules))
        .route("/", get(index))
        .route("/{*path}", get(static_asset))
        .with_state(state)
        .nest("/api/media", media_router(media, media_scheduler))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
                info_span!(
                    "http",
                    method = %request.method(),
                    path = %request.uri().path(),
                )
            }),
        )
        .layer(cors_layer())
}

const VITE_DEV_ORIGINS: [&str; 3] = [
    "http://localhost:5173",
    "http://127.0.0.1:5173",
    "http://[::1]:5173",
];

fn cors_layer() -> CorsLayer {
    let origins = VITE_DEV_ORIGINS.map(HeaderValue::from_static);
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::ACCEPT, header::CONTENT_TYPE])
}

#[derive(Clone)]
struct MediaApiState {
    service: Arc<MediaService>,
    scheduler: Arc<MediaScheduler>,
}

fn media_router(service: Arc<MediaService>, scheduler: Arc<MediaScheduler>) -> Router {
    Router::new()
        .route(
            "/settings",
            get(get_media_settings).put(update_media_settings),
        )
        .route("/tmdb/search", get(search_tmdb_media))
        .route("/tmdb/details", get(get_tmdb_details_query))
        .route("/tmdb/season", get(get_tmdb_season_query))
        .route("/tmdb/{media_type}/{id}", get(get_tmdb_details_path))
        .route("/tmdb/tv/{id}/season/{season}", get(get_tmdb_season_path))
        .route(
            "/quality-profiles",
            get(list_quality_profiles).post(create_quality_profile),
        )
        .route(
            "/quality-profiles/{id}",
            get(get_quality_profile)
                .put(update_quality_profile)
                .delete(delete_quality_profile),
        )
        .route(
            "/subscriptions",
            get(list_media_subscriptions).post(create_media_subscription),
        )
        .route(
            "/subscriptions/{id}",
            get(get_media_subscription)
                .put(update_media_subscription)
                .delete(delete_media_subscription),
        )
        .route("/subscriptions/{id}/run", post(run_media_subscription))
        .route(
            "/subscriptions/{id}/last-run",
            get(get_media_subscription_last_run),
        )
        .route("/subscriptions/{id}/pause", post(pause_media_subscription))
        .route(
            "/subscriptions/{id}/resume",
            post(resume_media_subscription),
        )
        .route(
            "/subscriptions/{id}/downloads",
            get(list_subscription_downloads),
        )
        .route("/resources/search", post(search_media_resources))
        .route(
            "/downloads",
            get(list_media_downloads).post(queue_media_download),
        )
        .route("/downloads/{id}", get(get_media_download))
        .with_state(MediaApiState { service, scheduler })
}

#[derive(Debug, Serialize)]
struct MediaSettingsResponse {
    // Never return the configured token. `tmdb_token_configured` is sufficient for the UI.
    tmdb_token: Option<String>,
    tmdb_token_configured: bool,
    tmdb_language: String,
    scan_interval_mins: u64,
    max_search_queries: usize,
    search_concurrency: usize,
    updated_at: String,
}

impl MediaSettingsResponse {
    fn from_settings(settings: MediaSettings) -> Self {
        let tmdb_token_configured = settings
            .tmdb_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty());
        Self {
            tmdb_token: None,
            tmdb_token_configured,
            tmdb_language: settings.tmdb_language,
            scan_interval_mins: settings.scan_interval_mins,
            max_search_queries: settings.max_search_queries,
            search_concurrency: settings.search_concurrency,
            updated_at: settings.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
struct UpdateMediaSettingsRequest {
    tmdb_token: Option<String>,
    #[serde(default)]
    clear_tmdb_token: bool,
    tmdb_language: String,
    scan_interval_mins: u64,
    max_search_queries: usize,
    search_concurrency: usize,
}

#[derive(Debug, Deserialize)]
struct TmdbSearchQuery {
    query: String,
    media_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TmdbDetailsQuery {
    tmdb_id: i64,
    media_type: String,
}

#[derive(Debug, Deserialize)]
struct TmdbSeasonQuery {
    tmdb_id: i64,
    season: u32,
}

#[derive(Debug, Default, Deserialize)]
struct MediaDownloadsQuery {
    subscription_id: Option<i64>,
    status: Option<String>,
    page: Option<usize>,
    page_size: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
struct SubscriptionDownloadsQuery {
    status: Option<String>,
    page: Option<usize>,
    page_size: Option<usize>,
}

async fn get_media_settings(
    State(state): State<MediaApiState>,
) -> Result<Json<MediaSettingsResponse>, ApiError> {
    let settings = state
        .service
        .database()
        .get_media_settings()
        .await
        .map_err(media_app_error)?;
    Ok(Json(MediaSettingsResponse::from_settings(settings)))
}

async fn update_media_settings(
    State(state): State<MediaApiState>,
    Json(payload): Json<UpdateMediaSettingsRequest>,
) -> Result<Json<MediaSettingsResponse>, ApiError> {
    validate_media_settings(&payload)?;
    let current = state
        .service
        .database()
        .get_media_settings()
        .await
        .map_err(media_app_error)?;
    let supplied_token = payload
        .tmdb_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string);
    let tmdb_token = if payload.clear_tmdb_token {
        None
    } else {
        supplied_token.or(current.tmdb_token)
    };
    let updated = state
        .service
        .database()
        .update_media_settings(&MediaSettings {
            tmdb_token,
            tmdb_language: payload.tmdb_language.trim().to_string(),
            scan_interval_mins: payload.scan_interval_mins,
            max_search_queries: payload.max_search_queries,
            search_concurrency: payload.search_concurrency,
            updated_at: current.updated_at,
        })
        .await
        .map_err(media_app_error)?;
    Ok(Json(MediaSettingsResponse::from_settings(updated)))
}

async fn search_tmdb_media(
    State(state): State<MediaApiState>,
    Query(query): Query<TmdbSearchQuery>,
) -> Result<Json<Vec<TmdbMedia>>, ApiError> {
    let text = query.query.trim();
    if text.is_empty() {
        return Err(ApiError::bad_request("query is required"));
    }
    if text.len() > 200 {
        return Err(ApiError::bad_request("query must not exceed 200 bytes"));
    }
    let media_type = normalize_tmdb_search_type(query.media_type.as_deref())?;
    Ok(Json(
        state
            .service
            .tmdb_search(text, media_type)
            .await
            .map_err(ApiError::from)?,
    ))
}

async fn get_tmdb_details_query(
    State(state): State<MediaApiState>,
    Query(query): Query<TmdbDetailsQuery>,
) -> Result<Json<TmdbDetails>, ApiError> {
    tmdb_details(&state, query.tmdb_id, &query.media_type).await
}

async fn get_tmdb_details_path(
    State(state): State<MediaApiState>,
    Path((media_type, id)): Path<(String, i64)>,
) -> Result<Json<TmdbDetails>, ApiError> {
    tmdb_details(&state, id, &media_type).await
}

async fn tmdb_details(
    state: &MediaApiState,
    tmdb_id: i64,
    media_type: &str,
) -> Result<Json<TmdbDetails>, ApiError> {
    validate_positive_id(tmdb_id, "tmdb_id")?;
    let media_type = parse_tmdb_media_type(media_type)?;
    Ok(Json(
        state
            .service
            .tmdb_details(tmdb_id, media_type)
            .await
            .map_err(ApiError::from)?,
    ))
}

async fn get_tmdb_season_query(
    State(state): State<MediaApiState>,
    Query(query): Query<TmdbSeasonQuery>,
) -> Result<Json<TmdbSeason>, ApiError> {
    tmdb_season(&state, query.tmdb_id, query.season).await
}

async fn get_tmdb_season_path(
    State(state): State<MediaApiState>,
    Path((id, season)): Path<(i64, u32)>,
) -> Result<Json<TmdbSeason>, ApiError> {
    tmdb_season(&state, id, season).await
}

async fn tmdb_season(
    state: &MediaApiState,
    tmdb_id: i64,
    season: u32,
) -> Result<Json<TmdbSeason>, ApiError> {
    validate_positive_id(tmdb_id, "tmdb_id")?;
    if season > 1_000 {
        return Err(ApiError::bad_request("season must not exceed 1000"));
    }
    Ok(Json(
        state
            .service
            .tmdb_season(tmdb_id, season)
            .await
            .map_err(ApiError::from)?,
    ))
}

async fn list_quality_profiles(
    State(state): State<MediaApiState>,
) -> Result<Json<Vec<QualityProfileRecord>>, ApiError> {
    Ok(Json(
        state
            .service
            .database()
            .list_quality_profiles()
            .await
            .map_err(media_app_error)?,
    ))
}

async fn get_quality_profile(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<QualityProfileRecord>, ApiError> {
    validate_positive_id(id, "quality profile id")?;
    let profile = state
        .service
        .database()
        .get_quality_profile(id)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::not_found("quality profile not found"))?;
    Ok(Json(profile))
}

async fn create_quality_profile(
    State(state): State<MediaApiState>,
    Json(mut payload): Json<QualityProfileRequest>,
) -> Result<(StatusCode, Json<QualityProfileRecord>), ApiError> {
    normalize_and_validate_quality_profile(&mut payload)?;
    let profile = state
        .service
        .database()
        .create_quality_profile(&payload)
        .await
        .map_err(media_app_error)?;
    Ok((StatusCode::CREATED, Json(profile)))
}

async fn update_quality_profile(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
    Json(mut payload): Json<QualityProfileRequest>,
) -> Result<Json<QualityProfileRecord>, ApiError> {
    validate_positive_id(id, "quality profile id")?;
    normalize_and_validate_quality_profile(&mut payload)?;
    let profile = state
        .service
        .database()
        .update_quality_profile(id, &payload)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::not_found("quality profile not found"))?;
    Ok(Json(profile))
}

async fn delete_quality_profile(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    validate_positive_id(id, "quality profile id")?;
    if state
        .service
        .database()
        .delete_quality_profile(id)
        .await
        .map_err(media_app_error)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("quality profile not found"))
    }
}

async fn list_media_subscriptions(
    State(state): State<MediaApiState>,
) -> Result<Json<Vec<SubscriptionRecord>>, ApiError> {
    Ok(Json(
        state
            .service
            .database()
            .list_subscriptions()
            .await
            .map_err(media_app_error)?,
    ))
}

async fn get_media_subscription(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<SubscriptionRecord>, ApiError> {
    Ok(Json(load_media_subscription(&state, id).await?))
}

async fn create_media_subscription(
    State(state): State<MediaApiState>,
    Json(mut payload): Json<CreateSubscriptionRequest>,
) -> Result<(StatusCode, Json<SubscriptionRecord>), ApiError> {
    normalize_and_validate_new_subscription(&mut payload)?;
    let subscription = state
        .service
        .create_subscription(&payload)
        .await
        .map_err(ApiError::from)?;
    if subscription.enabled {
        state.scheduler.wake();
    }
    Ok((StatusCode::CREATED, Json(subscription)))
}

async fn update_media_subscription(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
    Json(mut payload): Json<UpdateSubscription>,
) -> Result<Json<SubscriptionRecord>, ApiError> {
    let current = load_media_subscription(&state, id).await?;
    if subscription_is_leased(&current) {
        return Err(ApiError::conflict(
            "subscription is currently being scanned",
        ));
    }
    normalize_and_validate_subscription_update(&current, &mut payload)?;
    ensure_media_references(
        &state,
        payload.quality_profile_id,
        payload.downloader_id,
        &payload.site_ids,
    )
    .await?;
    let updated = state
        .service
        .database()
        .update_subscription(id, current.version, &payload)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::conflict("subscription changed or is currently being scanned"))?;
    if updated.enabled {
        state.scheduler.wake();
    }
    Ok(Json(updated))
}

async fn delete_media_subscription(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let current = load_media_subscription(&state, id).await?;
    if subscription_is_leased(&current) {
        return Err(ApiError::conflict(
            "subscription is currently being scanned",
        ));
    }
    if state
        .service
        .database()
        .delete_subscription(id, current.version)
        .await
        .map_err(media_app_error)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::conflict(
            "subscription changed or is currently being scanned",
        ))
    }
}

async fn run_media_subscription(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<SubscriptionRunResult>, ApiError> {
    let subscription = load_media_subscription(&state, id).await?;
    reject_completed_subscription(&subscription, "run")?;
    let result = state
        .service
        .run_subscription(id)
        .await
        .map_err(ApiError::from)?;
    info!(
        subscription_id = result.subscription_id,
        target_key = %result.target_key,
        queries = result.query_count,
        candidates = result.candidate_count,
        accepted = result.accepted_count,
        queued = result.download.is_some(),
        site_errors = result.site_errors.len(),
        "manual media subscription scan completed"
    );
    state.scheduler.wake();
    Ok(Json(result))
}

async fn get_media_subscription_last_run(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<SubscriptionRunSnapshot>, ApiError> {
    load_media_subscription(&state, id).await?;
    state
        .service
        .get_subscription_last_run(id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("subscription has no recorded run details"))
}

async fn pause_media_subscription(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<SubscriptionRecord>, ApiError> {
    set_media_subscription_enabled(&state, id, false).await
}

async fn resume_media_subscription(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<SubscriptionRecord>, ApiError> {
    set_media_subscription_enabled(&state, id, true).await
}

async fn set_media_subscription_enabled(
    state: &MediaApiState,
    id: i64,
    enabled: bool,
) -> Result<Json<SubscriptionRecord>, ApiError> {
    let current = load_media_subscription(state, id).await?;
    if enabled {
        reject_completed_subscription(&current, "resume")?;
    }
    if subscription_is_leased(&current) {
        return Err(ApiError::conflict(
            "subscription is currently being scanned",
        ));
    }
    if !state
        .service
        .database()
        .set_subscription_enabled(id, current.version, enabled)
        .await
        .map_err(media_app_error)?
    {
        return Err(ApiError::conflict(
            "subscription changed or is currently being scanned",
        ));
    }
    let subscription = load_media_subscription(state, id).await?;
    if enabled {
        state.scheduler.wake();
    }
    Ok(Json(subscription))
}

async fn search_media_resources(
    State(state): State<MediaApiState>,
    Json(payload): Json<ResourceSearchRequest>,
) -> Result<Json<ResourceSearchResponse>, ApiError> {
    validate_resource_search(&payload)?;
    Ok(Json(
        state
            .service
            .search_resources(&payload)
            .await
            .map_err(ApiError::from)?,
    ))
}

async fn queue_media_download(
    State(state): State<MediaApiState>,
    Json(mut payload): Json<QueueDownloadRequest>,
) -> Result<(StatusCode, Json<MediaDownloadResponse>), ApiError> {
    normalize_and_validate_download(&state, &mut payload).await?;
    let download = state
        .service
        .queue_download(&payload)
        .await
        .map_err(ApiError::from)?;
    state.scheduler.wake();
    Ok((
        StatusCode::CREATED,
        Json(MediaDownloadResponse::from(download)),
    ))
}

async fn list_media_downloads(
    State(state): State<MediaApiState>,
    Query(query): Query<MediaDownloadsQuery>,
) -> Result<Json<Vec<MediaDownloadResponse>>, ApiError> {
    let (status, limit, offset) =
        validate_download_query(query.status.as_deref(), query.page, query.page_size)?;
    if let Some(subscription_id) = query.subscription_id {
        load_media_subscription(&state, subscription_id).await?;
    }
    let downloads = state
        .service
        .database()
        .list_media_downloads(query.subscription_id, status.as_deref(), limit, offset)
        .await
        .map_err(media_app_error)?;
    Ok(Json(
        downloads
            .into_iter()
            .map(MediaDownloadResponse::from)
            .collect(),
    ))
}

async fn list_subscription_downloads(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
    Query(query): Query<SubscriptionDownloadsQuery>,
) -> Result<Json<Vec<MediaDownloadResponse>>, ApiError> {
    load_media_subscription(&state, id).await?;
    let (status, limit, offset) =
        validate_download_query(query.status.as_deref(), query.page, query.page_size)?;
    let downloads = state
        .service
        .database()
        .list_media_downloads(Some(id), status.as_deref(), limit, offset)
        .await
        .map_err(media_app_error)?;
    Ok(Json(
        downloads
            .into_iter()
            .map(MediaDownloadResponse::from)
            .collect(),
    ))
}

async fn get_media_download(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<MediaDownloadResponse>, ApiError> {
    validate_positive_id(id, "download id")?;
    let download = state
        .service
        .database()
        .get_media_download(id)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::not_found("media download not found"))?;
    Ok(Json(MediaDownloadResponse::from(download)))
}

#[derive(Debug, Serialize)]
struct MediaDownloadResponse {
    #[serde(flatten)]
    download: MediaDownloadRecord,
    parsed_release: Option<ReleaseInfo>,
}

impl From<MediaDownloadRecord> for MediaDownloadResponse {
    fn from(download: MediaDownloadRecord) -> Self {
        let parsed_release = ReleaseParser::default().parse(&download.title).ok();
        Self {
            download,
            parsed_release,
        }
    }
}

async fn load_media_subscription(
    state: &MediaApiState,
    id: i64,
) -> Result<SubscriptionRecord, ApiError> {
    validate_positive_id(id, "subscription id")?;
    state
        .service
        .database()
        .get_subscription(id)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::not_found("subscription not found"))
}

fn validate_media_settings(payload: &UpdateMediaSettingsRequest) -> Result<(), ApiError> {
    let language = payload.tmdb_language.trim();
    if language.is_empty() || language.len() > 32 {
        return Err(ApiError::bad_request(
            "tmdb_language must contain between 1 and 32 bytes",
        ));
    }
    if payload.scan_interval_mins == 0 || payload.scan_interval_mins > 10_080 {
        return Err(ApiError::bad_request(
            "scan_interval_mins must be between 1 and 10080",
        ));
    }
    if !(1..=32).contains(&payload.max_search_queries) {
        return Err(ApiError::bad_request(
            "max_search_queries must be between 1 and 32",
        ));
    }
    if !(1..=16).contains(&payload.search_concurrency) {
        return Err(ApiError::bad_request(
            "search_concurrency must be between 1 and 16",
        ));
    }
    if payload
        .tmdb_token
        .as_deref()
        .is_some_and(|token| token.trim().len() > 4_096)
    {
        return Err(ApiError::bad_request(
            "tmdb_token must not exceed 4096 bytes",
        ));
    }
    if payload.clear_tmdb_token
        && payload
            .tmdb_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
    {
        return Err(ApiError::bad_request(
            "tmdb_token and clear_tmdb_token cannot both be set",
        ));
    }
    Ok(())
}

fn normalize_tmdb_search_type(media_type: Option<&str>) -> Result<Option<&str>, ApiError> {
    match media_type
        .unwrap_or("multi")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "multi" | "" => Ok(None),
        "tv" => Ok(Some("tv")),
        "movie" => Ok(Some("movie")),
        _ => Err(ApiError::bad_request(
            "media_type must be multi, tv, or movie",
        )),
    }
}

fn parse_tmdb_media_type(value: &str) -> Result<TmdbMediaType, ApiError> {
    TmdbMediaType::parse(value).map_err(|_| ApiError::bad_request("media_type must be tv or movie"))
}

fn validate_positive_id(id: i64, field: &str) -> Result<(), ApiError> {
    if id <= 0 {
        Err(ApiError::bad_request(format!("{field} must be positive")))
    } else {
        Ok(())
    }
}

fn normalize_and_validate_quality_profile(
    payload: &mut QualityProfileRequest,
) -> Result<(), ApiError> {
    payload.name = payload.name.trim().to_string();
    if payload.name.is_empty() || payload.name.len() > 100 {
        return Err(ApiError::bad_request(
            "quality profile name must contain between 1 and 100 bytes",
        ));
    }
    if !(0..=100).contains(&payload.minimum_score) {
        return Err(ApiError::bad_request(
            "minimum_score must be between 0 and 100",
        ));
    }
    if payload.min_seeders > 1_000_000 {
        return Err(ApiError::bad_request("min_seeders must not exceed 1000000"));
    }
    for (field, values) in [
        ("resolution_order", &mut payload.resolution_order),
        ("allowed_resolutions", &mut payload.allowed_resolutions),
        ("blocked_resolutions", &mut payload.blocked_resolutions),
        ("source_order", &mut payload.source_order),
        ("allowed_sources", &mut payload.allowed_sources),
        ("codec_order", &mut payload.codec_order),
        ("blocked_codecs", &mut payload.blocked_codecs),
    ] {
        normalize_string_list(field, values)?;
    }
    Ok(())
}

fn normalize_string_list(field: &str, values: &mut Vec<String>) -> Result<(), ApiError> {
    if values.len() > 64 {
        return Err(ApiError::bad_request(format!(
            "{field} must not contain more than 64 values"
        )));
    }
    let mut seen = std::collections::HashSet::new();
    for value in values.iter_mut() {
        *value = value.trim().to_string();
        if value.is_empty() || value.len() > 64 {
            return Err(ApiError::bad_request(format!(
                "{field} values must contain between 1 and 64 bytes"
            )));
        }
        if !seen.insert(value.to_ascii_lowercase()) {
            return Err(ApiError::bad_request(format!(
                "{field} must not contain duplicate values"
            )));
        }
    }
    Ok(())
}

fn normalize_and_validate_new_subscription(
    payload: &mut CreateSubscriptionRequest,
) -> Result<(), ApiError> {
    validate_positive_id(payload.tmdb_id, "tmdb_id")?;
    validate_positive_id(payload.quality_profile_id, "quality_profile_id")?;
    validate_positive_id(payload.downloader_id, "downloader_id")?;
    payload.media_type = payload.media_type.trim().to_ascii_lowercase();
    if !matches!(payload.media_type.as_str(), "tv" | "movie") {
        return Err(ApiError::bad_request("media_type must be tv or movie"));
    }
    validate_site_ids(&payload.site_ids)?;
    normalize_save_path(&mut payload.save_path)?;
    if payload.start_episode == Some(0) || payload.absolute_episode == Some(0) {
        return Err(ApiError::bad_request(
            "episode numbers must be greater than zero",
        ));
    }
    if payload.media_type == "tv" && payload.season.is_none() {
        return Err(ApiError::bad_request(
            "season is required for a TV subscription",
        ));
    }
    if payload.media_type == "movie"
        && (payload.season.is_some()
            || payload.start_episode.is_some()
            || payload.absolute_episode.is_some())
    {
        return Err(ApiError::bad_request(
            "movie subscriptions cannot contain episode fields",
        ));
    }
    Ok(())
}

fn normalize_and_validate_subscription_update(
    current: &SubscriptionRecord,
    payload: &mut UpdateSubscription,
) -> Result<(), ApiError> {
    validate_positive_id(payload.quality_profile_id, "quality_profile_id")?;
    validate_positive_id(payload.downloader_id, "downloader_id")?;
    validate_site_ids(&payload.site_ids)?;
    normalize_save_path(&mut payload.save_path)?;
    if payload.next_episode == Some(0) || payload.absolute_episode == Some(0) {
        return Err(ApiError::bad_request(
            "episode numbers must be greater than zero",
        ));
    }
    if current.media_type == "movie" {
        if payload.season.is_some()
            || payload.next_episode.is_some()
            || payload.absolute_episode.is_some()
        {
            return Err(ApiError::bad_request(
                "movie subscriptions cannot contain episode fields",
            ));
        }
    } else {
        if payload.season.is_none() {
            return Err(ApiError::bad_request(
                "season is required for a TV subscription",
            ));
        }
        if payload.next_episode.is_none() && payload.absolute_episode.is_some() {
            payload.next_episode = current.next_episode.or(current.start_episode);
        }
        if payload.next_episode.is_none() {
            return Err(ApiError::bad_request(
                "next_episode is required for a TV subscription",
            ));
        }
    }
    Ok(())
}

fn validate_site_ids(site_ids: &[i64]) -> Result<(), ApiError> {
    if site_ids.is_empty() {
        return Err(ApiError::bad_request("at least one site is required"));
    }
    if site_ids.len() > 100 {
        return Err(ApiError::bad_request(
            "no more than 100 sites may be selected",
        ));
    }
    let mut unique = std::collections::HashSet::new();
    for site_id in site_ids {
        validate_positive_id(*site_id, "site_id")?;
        if !unique.insert(*site_id) {
            return Err(ApiError::bad_request(
                "site_ids must not contain duplicates",
            ));
        }
    }
    Ok(())
}

fn normalize_save_path(save_path: &mut Option<String>) -> Result<(), ApiError> {
    let normalized = save_path
        .take()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty());
    if normalized.as_deref().is_some_and(|path| path.len() > 1_024) {
        return Err(ApiError::bad_request(
            "save_path must not exceed 1024 bytes",
        ));
    }
    *save_path = normalized;
    Ok(())
}

async fn ensure_media_references(
    state: &MediaApiState,
    quality_profile_id: i64,
    downloader_id: i64,
    site_ids: &[i64],
) -> Result<(), ApiError> {
    let db = state.service.database();
    if db
        .get_quality_profile(quality_profile_id)
        .await
        .map_err(media_app_error)?
        .is_none()
    {
        return Err(ApiError::not_found("quality profile not found"));
    }
    if db
        .get_downloader(downloader_id)
        .await
        .map_err(media_app_error)?
        .is_none()
    {
        return Err(ApiError::not_found("downloader not found"));
    }
    let configured: std::collections::HashSet<_> = db
        .list_sites()
        .await
        .map_err(media_app_error)?
        .into_iter()
        .map(|site| site.id)
        .collect();
    if site_ids.iter().any(|site_id| !configured.contains(site_id)) {
        return Err(ApiError::not_found(
            "one or more selected PT sites do not exist",
        ));
    }
    Ok(())
}

fn subscription_is_leased(subscription: &SubscriptionRecord) -> bool {
    subscription.lease_owner.is_some()
        && subscription
            .lease_until
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .is_some_and(|until| until > Utc::now())
}

fn validate_resource_search(payload: &ResourceSearchRequest) -> Result<(), ApiError> {
    if payload
        .query
        .as_deref()
        .is_none_or(|query| query.trim().is_empty())
        && payload.target.is_none()
    {
        return Err(ApiError::bad_request("query or target is required"));
    }
    if payload
        .query
        .as_deref()
        .is_some_and(|query| query.trim().len() > 512)
    {
        return Err(ApiError::bad_request("query must not exceed 512 bytes"));
    }
    if payload
        .page_size
        .is_some_and(|size| !(1..=100).contains(&size))
    {
        return Err(ApiError::bad_request("page_size must be between 1 and 100"));
    }
    if let Some(profile_id) = payload.quality_profile_id {
        validate_positive_id(profile_id, "quality_profile_id")?;
    }
    if !payload.site_ids.is_empty() {
        validate_site_ids(&payload.site_ids)?;
    }
    if let Some(target) = &payload.target {
        validate_media_target(target)?;
    }
    Ok(())
}

fn validate_media_target(target: &MediaTarget) -> Result<(), ApiError> {
    validate_positive_id(target.tmdb_id(), "target.tmdb_id")?;
    if target.titles().is_empty() || target.titles().len() > 32 {
        return Err(ApiError::bad_request(
            "target.titles must contain between 1 and 32 values",
        ));
    }
    if target
        .titles()
        .iter()
        .any(|title| title.trim().is_empty() || title.len() > 512)
    {
        return Err(ApiError::bad_request(
            "target titles must contain between 1 and 512 bytes",
        ));
    }
    match target {
        MediaTarget::Episode { episode: 0, .. }
        | MediaTarget::Anime {
            absolute_episode: 0,
            ..
        } => Err(ApiError::bad_request(
            "target episode numbers must be greater than zero",
        )),
        _ => Ok(()),
    }
}

async fn normalize_and_validate_download(
    _state: &MediaApiState,
    payload: &mut QueueDownloadRequest,
) -> Result<(), ApiError> {
    validate_positive_id(payload.quality_profile_id, "quality_profile_id")?;
    validate_positive_id(payload.downloader_id, "downloader_id")?;
    payload.candidate_id = payload.candidate_id.trim().to_string();
    let token = payload
        .candidate_id
        .strip_prefix("cand_")
        .ok_or_else(|| ApiError::bad_request("candidate_id is invalid"))?;
    if token.len() != 48 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request("candidate_id is invalid"));
    }

    if let Some(reason) = payload.override_reason.take() {
        let reason = reason.trim().to_string();
        if reason.len() > 1_000 {
            return Err(ApiError::bad_request(
                "override_reason must not exceed 1000 bytes",
            ));
        }
        payload.override_reason = (!reason.is_empty()).then_some(reason);
    }

    Ok(())
}

fn reject_completed_subscription(
    subscription: &SubscriptionRecord,
    action: &str,
) -> Result<(), ApiError> {
    if subscription.last_status.as_deref() == Some("completed") {
        Err(ApiError::conflict(format!(
            "completed subscription cannot {action}; edit it with a new cursor first"
        )))
    } else {
        Ok(())
    }
}

fn validate_download_query(
    status: Option<&str>,
    page: Option<usize>,
    page_size: Option<usize>,
) -> Result<(Option<String>, usize, usize), ApiError> {
    let page = page.unwrap_or(1);
    let page_size = page_size.unwrap_or(100);
    if page == 0 {
        return Err(ApiError::bad_request("page must be greater than zero"));
    }
    if !(1..=200).contains(&page_size) {
        return Err(ApiError::bad_request("page_size must be between 1 and 200"));
    }
    let offset = page
        .checked_sub(1)
        .and_then(|value| value.checked_mul(page_size))
        .ok_or_else(|| ApiError::bad_request("pagination values are too large"))?;
    i64::try_from(offset).map_err(|_| ApiError::bad_request("pagination values are too large"))?;
    let status = status
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    if status.as_deref().is_some_and(|value| {
        !matches!(
            value,
            "queued"
                | "fetching"
                | "submitting"
                | "reconciling"
                | "retry_wait"
                | "submitted"
                | "failed"
                | "cancelled"
        )
    }) {
        return Err(ApiError::bad_request("invalid download status"));
    }
    Ok((status, page_size, offset))
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
                downloader_id: payload.downloader_id,
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
            .run_with_shutdown(
                config,
                shutdown.clone(),
                Some(state.pool.clone()),
                Some(state.db.clone()),
            )
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
    _delete_files: bool,
) -> Result<(), ApiError> {
    if tasks.is_empty() {
        return Ok(());
    }

    let ids = tasks.iter().map(|task| task.id).collect::<Vec<_>>();
    state.db.update_rss_enabled(&ids, false).await?;
    state.jobs.stop_tasks(&ids).await;

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
    #[serde(default = "default_true")]
    use_proxy: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateSiteRequest {
    name: String,
    site_type: String,
    base_url: String,
    auth_config: Option<serde_json::Value>,
    #[serde(default)]
    clear_auth_config: bool,
    #[serde(default = "default_true")]
    use_proxy: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteAuthInput {
    auth_type: String,
    cookie: Option<String>,
    passkey: Option<String>,
    api_key: Option<String>,
}

#[derive(Debug, Serialize)]
struct SiteResponse {
    id: i64,
    name: String,
    site_type: String,
    base_url: String,
    auth_type: Option<&'static str>,
    auth_configured: bool,
    use_proxy: bool,
    created_at: String,
    updated_at: String,
    stats: Option<SiteStatsRecord>,
}

fn default_true() -> bool {
    true
}

impl From<SiteWithStats> for SiteResponse {
    fn from(site: SiteWithStats) -> Self {
        let auth = serde_json::from_str::<SiteAuth>(&site.auth_config).ok();
        Self {
            id: site.id,
            name: site.name,
            site_type: site.site_type,
            base_url: site.base_url,
            auth_type: auth.as_ref().map(site_auth_type),
            auth_configured: auth.as_ref().is_some_and(site_auth_is_configured),
            use_proxy: site.use_proxy,
            created_at: site.created_at,
            updated_at: site.updated_at,
            stats: site.stats,
        }
    }
}

fn site_auth_type(auth: &SiteAuth) -> &'static str {
    match auth {
        SiteAuth::Cookie { .. } => "cookie",
        SiteAuth::Passkey { .. } => "passkey",
        SiteAuth::CookiePasskey { .. } => "cookie_passkey",
        SiteAuth::ApiKey { .. } => "api_key",
    }
}

fn site_auth_is_configured(auth: &SiteAuth) -> bool {
    match auth {
        SiteAuth::Cookie { cookie } => !cookie.trim().is_empty(),
        SiteAuth::Passkey { passkey } => !passkey.trim().is_empty(),
        SiteAuth::CookiePasskey { cookie, passkey } => {
            !cookie.trim().is_empty() && !passkey.trim().is_empty()
        }
        SiteAuth::ApiKey { api_key } => !api_key.trim().is_empty(),
    }
}

fn site_auth_has_secret(auth: &SiteAuth) -> bool {
    match auth {
        SiteAuth::Cookie { cookie } => !cookie.is_empty(),
        SiteAuth::Passkey { passkey } => !passkey.is_empty(),
        SiteAuth::CookiePasskey { cookie, passkey } => !cookie.is_empty() || !passkey.is_empty(),
        SiteAuth::ApiKey { api_key } => !api_key.is_empty(),
    }
}

fn parse_site_auth_input(value: serde_json::Value) -> Result<SiteAuth, ApiError> {
    let input: SiteAuthInput =
        serde_json::from_value(value).map_err(|_| ApiError::bad_request("认证配置格式无效"))?;
    match input.auth_type.as_str() {
        "cookie" => Ok(SiteAuth::Cookie {
            cookie: input.cookie.unwrap_or_default(),
        }),
        "passkey" => Ok(SiteAuth::Passkey {
            passkey: input.passkey.unwrap_or_default(),
        }),
        "cookie_passkey" => Ok(SiteAuth::CookiePasskey {
            cookie: input.cookie.unwrap_or_default(),
            passkey: input.passkey.unwrap_or_default(),
        }),
        "api_key" => Ok(SiteAuth::ApiKey {
            api_key: input.api_key.unwrap_or_default(),
        }),
        _ => Err(ApiError::bad_request("不支持的认证类型")),
    }
}

fn parse_site_type(value: &str) -> Result<SiteType, ApiError> {
    SiteType::from_str(value.trim()).ok_or_else(|| ApiError::bad_request("不支持的站点类型"))
}

fn validate_site_auth_type(site_type: SiteType, auth: &SiteAuth) -> Result<(), ApiError> {
    if site_type == SiteType::MTeam && !matches!(auth, SiteAuth::ApiKey { .. }) {
        return Err(ApiError::bad_request("M-Team 站点必须使用 API Key 认证"));
    }
    Ok(())
}

fn require_configured_site_auth(auth: &SiteAuth) -> Result<(), ApiError> {
    if site_auth_is_configured(auth) {
        Ok(())
    } else {
        Err(ApiError::bad_request("认证凭据不能为空"))
    }
}

fn empty_site_auth(auth: &SiteAuth) -> SiteAuth {
    match auth {
        SiteAuth::Cookie { .. } => SiteAuth::Cookie {
            cookie: String::new(),
        },
        SiteAuth::Passkey { .. } => SiteAuth::Passkey {
            passkey: String::new(),
        },
        SiteAuth::CookiePasskey { .. } => SiteAuth::CookiePasskey {
            cookie: String::new(),
            passkey: String::new(),
        },
        SiteAuth::ApiKey { .. } => SiteAuth::ApiKey {
            api_key: String::new(),
        },
    }
}

fn merge_site_auth(existing: SiteAuth, incoming: SiteAuth) -> Result<SiteAuth, ApiError> {
    match (existing, incoming) {
        (SiteAuth::Cookie { cookie }, SiteAuth::Cookie { cookie: next }) => Ok(SiteAuth::Cookie {
            cookie: if next.is_empty() { cookie } else { next },
        }),
        (SiteAuth::Passkey { passkey }, SiteAuth::Passkey { passkey: next }) => {
            Ok(SiteAuth::Passkey {
                passkey: if next.is_empty() { passkey } else { next },
            })
        }
        (
            SiteAuth::CookiePasskey { cookie, passkey },
            SiteAuth::CookiePasskey {
                cookie: next_cookie,
                passkey: next_passkey,
            },
        ) => Ok(SiteAuth::CookiePasskey {
            cookie: if next_cookie.is_empty() {
                cookie
            } else {
                next_cookie
            },
            passkey: if next_passkey.is_empty() {
                passkey
            } else {
                next_passkey
            },
        }),
        (SiteAuth::ApiKey { api_key }, SiteAuth::ApiKey { api_key: next }) => {
            Ok(SiteAuth::ApiKey {
                api_key: if next.is_empty() { api_key } else { next },
            })
        }
        _ => Err(ApiError::bad_request("切换认证类型时必须提交完整的新凭据")),
    }
}

fn resolve_site_auth_update(
    existing: &crate::site::SiteRecord,
    site_type: &str,
    incoming: Option<serde_json::Value>,
    clear_auth_config: bool,
) -> Result<SiteAuth, ApiError> {
    let old_site_type = SiteType::from_str(existing.site_type.trim())
        .ok_or_else(|| ApiError::internal("现有站点类型无效"))?;
    let new_site_type = parse_site_type(site_type)?;
    let existing_auth: SiteAuth = serde_json::from_str(&existing.auth_config)
        .map_err(|_| ApiError::internal("现有认证配置无效"))?;
    let incoming = incoming.map(parse_site_auth_input).transpose()?;

    if let Some(auth) = incoming.as_ref() {
        validate_site_auth_type(new_site_type, auth)?;
    }

    let site_type_changed = old_site_type != new_site_type;
    let auth_type_changed = incoming
        .as_ref()
        .is_some_and(|auth| site_auth_type(auth) != site_auth_type(&existing_auth));

    if site_type_changed || auth_type_changed {
        if clear_auth_config {
            return Err(ApiError::bad_request(
                "切换站点或认证类型时不能同时清除凭据",
            ));
        }
        let auth = incoming
            .ok_or_else(|| ApiError::bad_request("切换站点或认证类型时必须提交完整的新凭据"))?;
        require_configured_site_auth(&auth)?;
        return Ok(auth);
    }

    validate_site_auth_type(new_site_type, &existing_auth)?;
    if clear_auth_config {
        if incoming.as_ref().is_some_and(site_auth_has_secret) {
            return Err(ApiError::bad_request(
                "认证凭据和 clear_auth_config 不能同时提交",
            ));
        }
        return Ok(empty_site_auth(&existing_auth));
    }

    match incoming {
        Some(auth) => merge_site_auth(existing_auth, auth),
        None => Ok(existing_auth),
    }
}

async fn list_sites(State(state): State<AppState>) -> Result<Json<Vec<SiteResponse>>, ApiError> {
    Ok(Json(
        state
            .db
            .list_sites_with_stats()
            .await?
            .into_iter()
            .map(SiteResponse::from)
            .collect(),
    ))
}

async fn create_site(
    State(state): State<AppState>,
    Json(body): Json<CreateSiteRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.name.is_empty() || body.site_type.is_empty() || body.base_url.is_empty() {
        return Err(ApiError::bad_request("名称、站点类型和基础URL不能为空"));
    }
    let site_type = parse_site_type(&body.site_type)?;
    let auth = parse_site_auth_input(body.auth_config)?;
    validate_site_auth_type(site_type, &auth)?;
    require_configured_site_auth(&auth)?;
    let auth_str = serde_json::to_string(&auth)
        .map_err(|e| ApiError::bad_request(format!("认证配置序列化失败: {}", e)))?;
    let id = state
        .db
        .create_site(
            &body.name,
            &body.site_type,
            &body.base_url,
            &auth_str,
            body.use_proxy,
        )
        .await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_site(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateSiteRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.name.is_empty() || body.site_type.is_empty() || body.base_url.is_empty() {
        return Err(ApiError::bad_request("名称、站点类型和基础URL不能为空"));
    }
    let existing = state
        .db
        .get_site(id)
        .await?
        .ok_or_else(|| ApiError::not_found("站点不存在"))?;
    let auth = resolve_site_auth_update(
        &existing,
        &body.site_type,
        body.auth_config,
        body.clear_auth_config,
    )?;
    let auth_str = serde_json::to_string(&auth)
        .map_err(|e| ApiError::bad_request(format!("认证配置序列化失败: {}", e)))?;
    state
        .db
        .update_site(
            id,
            &body.name,
            &body.site_type,
            &body.base_url,
            &auth_str,
            body.use_proxy,
        )
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
    let settings = state.db.get_settings().await?;
    let client = client_factory::resolve_client(settings.proxy.as_deref(), site.use_proxy)
        .map_err(|e| ApiError::internal(format!("创建 HTTP 客户端失败: {}", e)))?;
    let adapter = site_factory::create_adapter(&site, client).map_err(ApiError::bad_request)?;
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
    let settings = state.db.get_settings().await?;
    let client = client_factory::resolve_client(settings.proxy.as_deref(), site.use_proxy)
        .map_err(|e| ApiError::internal(format!("创建 HTTP 客户端失败: {}", e)))?;
    let adapter = site_factory::create_adapter(&site, client).map_err(ApiError::bad_request)?;
    let stats = adapter
        .get_user_stats()
        .await
        .map_err(|e| ApiError::internal(e))?;
    Ok(Json(stats))
}

async fn get_sites_stats_overview(
    State(state): State<AppState>,
) -> Result<Json<Vec<SiteResponse>>, ApiError> {
    Ok(Json(
        state
            .site_stats_refresher
            .refresh_all()
            .await?
            .into_iter()
            .map(SiteResponse::from)
            .collect(),
    ))
}

// ========== Proxy Test API ==========

#[derive(Debug, Deserialize)]
struct ProxyTestRequest {
    proxy: String,
    test_url: String,
}

async fn test_proxy(
    Json(body): Json<ProxyTestRequest>,
) -> Result<Json<client_factory::ProxyTestResult>, ApiError> {
    if body.proxy.trim().is_empty() {
        return Err(ApiError::bad_request("代理地址不能为空"));
    }
    if body.test_url.trim().is_empty() {
        return Err(ApiError::bad_request("测试URL不能为空"));
    }
    let result = client_factory::test_proxy(&body.proxy, &body.test_url).await;
    Ok(Json(result))
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
    let settings = state.db.get_settings().await?;
    let result = crate::sign_in::probe_lightpanda_1_1_1_1(task, settings.use_proxy_for_lightpanda)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(result))
}

async fn probe_sign_in_form_1_1_1_1(
    State(state): State<AppState>,
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
    let settings = state.db.get_settings().await?;
    let result =
        crate::sign_in::probe_lightpanda_request_1_1_1_1(body, settings.use_proxy_for_lightpanda)
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
    body.sign_in_method = Some(crate::sign_in::normalize_sign_in_method(
        body.sign_in_method
            .as_deref()
            .unwrap_or(crate::sign_in::SIGN_IN_METHOD_OPEN_PAGE),
    ));

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

#[derive(Debug, Deserialize)]
struct UpdateDownloaderRequest {
    name: String,
    downloader_type: String,
    url: String,
    username: Option<String>,
    password: Option<String>,
    #[serde(default)]
    clear_password: bool,
}

#[derive(Debug, Serialize)]
struct DownloaderResponse {
    id: i64,
    name: String,
    downloader_type: String,
    url: String,
    username: String,
    password_configured: bool,
    created_at: String,
    updated_at: String,
}

impl From<crate::downloader::DownloaderRecord> for DownloaderResponse {
    fn from(downloader: crate::downloader::DownloaderRecord) -> Self {
        Self {
            id: downloader.id,
            name: downloader.name,
            downloader_type: downloader.downloader_type,
            url: downloader.url,
            username: downloader.username,
            password_configured: !downloader.password.is_empty(),
            created_at: downloader.created_at,
            updated_at: downloader.updated_at,
        }
    }
}

fn resolve_downloader_password(
    existing: &str,
    incoming: Option<String>,
    clear_password: bool,
) -> Result<String, ApiError> {
    if clear_password {
        if incoming
            .as_ref()
            .is_some_and(|password| !password.is_empty())
        {
            return Err(ApiError::bad_request(
                "password and clear_password cannot both be set",
            ));
        }
        return Ok(String::new());
    }

    Ok(incoming
        .filter(|password| !password.is_empty())
        .unwrap_or_else(|| existing.to_string()))
}

async fn list_downloaders(
    State(state): State<AppState>,
) -> Result<Json<Vec<DownloaderResponse>>, ApiError> {
    Ok(Json(
        state
            .db
            .list_downloaders()
            .await?
            .into_iter()
            .map(DownloaderResponse::from)
            .collect(),
    ))
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
    Json(body): Json<UpdateDownloaderRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.name.is_empty() || body.url.is_empty() {
        return Err(ApiError::bad_request("名称和URL不能为空"));
    }
    let existing = state
        .db
        .get_downloader(id)
        .await?
        .ok_or_else(|| ApiError::not_found("下载器不存在"))?;
    let password =
        resolve_downloader_password(&existing.password, body.password, body.clear_password)?;
    state
        .db
        .update_downloader(
            id,
            &body.name,
            &body.downloader_type,
            &body.url,
            body.username.as_deref().unwrap_or(""),
            &password,
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
    let client = state.pool.get(&dl).await.map_err(ApiError::bad_request)?;
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
    let client = state.pool.get(&dl).await.map_err(ApiError::bad_request)?;
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

async fn get_downloader_default_path(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dl = state
        .db
        .get_downloader(id)
        .await?
        .ok_or_else(|| ApiError::not_found("下载器不存在"))?;
    let client = state.pool.get(&dl).await.map_err(ApiError::bad_request)?;
    let path = client
        .get_default_save_path()
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({ "path": path })))
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

    // 按 downloader_id 分组，分别从对应下载器拉取实时种子信息
    let mut dl_live_cache: std::collections::HashMap<i64, Vec<crate::downloader::TorrentInfo>> =
        std::collections::HashMap::new();
    for record in &torrents.records {
        let dl_id = match record.downloader_id {
            Some(id) => id,
            None => continue,
        };
        if dl_live_cache.contains_key(&dl_id) {
            continue;
        }
        if let Ok(Some(downloader)) = state.db.get_downloader(dl_id).await {
            if let Ok(live_torrents) = state
                .collector
                .get_tagged_torrents(&downloader, &task.tag)
                .await
            {
                dl_live_cache.insert(dl_id, live_torrents);
            }
        }
    }

    for record in &mut torrents.records {
        if let Some(dl_id) = record.downloader_id {
            if let Some(live_torrents) = dl_live_cache.get(&dl_id) {
                if let Some(live) = find_live_brush_torrent(record, live_torrents) {
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
    // Choose aggregation bucket based on time range to avoid returning excessive raw data:
    //   ≤ 1h  → raw 10s data (max ~360 pts/downloader)
    //   ≤ 6h  → 1-minute buckets (max ~360 pts)
    //   ≤ 24h → 5-minute buckets (max ~288 pts)
    //   > 24h → 1-hour buckets   (max ~168 pts for 7d)
    let bucket_secs = if hours <= 1 {
        None
    } else if hours <= 6 {
        Some(60)
    } else if hours <= 24 {
        Some(300)
    } else {
        Some(3600)
    };
    let data = state
        .db
        .get_downloader_speed_snapshots(q.downloader_id, &since, &until, bucket_secs)
        .await?;
    Ok(Json(data))
}

#[derive(Debug, Serialize)]
struct DailyTransferItem {
    date: String,
    uploaded: i64,
    downloaded: i64,
}

async fn daily_transfer(
    State(state): State<AppState>,
    Query(q): Query<StatsQuery>,
) -> Result<Json<Vec<DailyTransferItem>>, ApiError> {
    let until = q.until.unwrap_or_else(|| Utc::now().to_rfc3339());
    let since = q
        .since
        .unwrap_or_else(|| (Utc::now() - chrono::Duration::days(30)).to_rfc3339());
    let data = state
        .db
        .get_daily_transfer_totals(q.task_id, &since, &until)
        .await?;
    Ok(Json(
        data.into_iter()
            .map(|(date, uploaded, downloaded)| DailyTransferItem {
                date,
                uploaded,
                downloaded,
            })
            .collect(),
    ))
}

async fn get_system_stats(
    State(state): State<AppState>,
) -> Result<Json<SystemSnapshot>, StatusCode> {
    match state.monitor.latest().await {
        Some(snapshot) => Ok(Json(snapshot)),
        None => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[derive(Debug, Deserialize)]
struct SystemStatsQuery {
    since: Option<String>,
    until: Option<String>,
    hours: Option<i64>,
}

async fn get_system_stats_history(
    State(state): State<AppState>,
    Query(q): Query<SystemStatsQuery>,
) -> Result<Json<Vec<SystemSnapshotRecord>>, ApiError> {
    let hours = q.hours.unwrap_or(24);
    let until = q.until.unwrap_or_else(|| Utc::now().to_rfc3339());
    let since = q.since.unwrap_or_else(|| {
        (Utc::now() - chrono::Duration::hours(hours) - chrono::Duration::minutes(2)).to_rfc3339()
    });
    let bucket_secs = if hours <= 1 {
        None
    } else if hours <= 6 {
        Some(60)
    } else if hours <= 24 {
        Some(300)
    } else {
        Some(3600)
    };
    let data = state
        .db
        .get_system_snapshots(&since, &until, bucket_secs)
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
    if let Some(proxy) = settings.proxy.as_deref() {
        let proxy = proxy.trim();
        if !proxy.is_empty()
            && !proxy.starts_with("http://")
            && !proxy.starts_with("https://")
            && !proxy.starts_with("socks5://")
            && !proxy.starts_with("socks5h://")
        {
            return Err(ApiError::bad_request(
                "proxy must start with http://, https://, socks5://, or socks5h://",
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
    record.avg_upload_speed = crate::brush::average_upload_speed(live.uploaded, live.time_active);
    record.ratio = crate::brush::calculate_ratio(live.uploaded, live.downloaded, live.ratio);
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

// ========== Tag Rules API ==========

async fn list_tag_rules(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::tag_rule::TagRuleRecord>>, ApiError> {
    Ok(Json(state.db.list_tag_rules().await?))
}

async fn get_tag_rule(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::tag_rule::TagRuleRecord>, ApiError> {
    state
        .db
        .get_tag_rule(id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("标签规则不存在"))
}

async fn create_tag_rule(
    State(state): State<AppState>,
    Json(body): Json<crate::tag_rule::TagRuleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_tag_rule(&body)?;
    let id = state.db.create_tag_rule(&body).await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_tag_rule(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<crate::tag_rule::TagRuleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_tag_rule(&body)?;
    state.db.update_tag_rule(id, &body).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_tag_rule(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.delete_tag_rule(id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn scan_tag_rules(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .tag_rule_scheduler
        .run_once()
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn validate_tag_rule(req: &crate::tag_rule::TagRuleRequest) -> Result<(), ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::bad_request("名称不能为空"));
    }
    if req.tag_name.trim().is_empty() {
        return Err(ApiError::bad_request("标签名不能为空"));
    }
    if req.match_rules.is_empty() {
        return Err(ApiError::bad_request("匹配规则不能为空"));
    }
    for (i, rule) in req.match_rules.iter().enumerate() {
        match rule.match_type.as_str() {
            "prefix" | "suffix" | "contains" | "exact" | "regex" => {}
            other => {
                return Err(ApiError::bad_request(format!(
                    "第{}条规则的匹配类型无效: {}，支持: prefix, suffix, contains, exact, regex",
                    i + 1,
                    other
                )));
            }
        }
        if rule.pattern.is_empty() {
            return Err(ApiError::bad_request(format!(
                "第{}条规则的匹配模式不能为空",
                i + 1
            )));
        }
    }
    Ok(())
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

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
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

impl From<MediaServiceError> for ApiError {
    fn from(value: MediaServiceError) -> Self {
        match value {
            MediaServiceError::App(error) => media_app_error(error),
            MediaServiceError::Tmdb(error) => match error {
                TmdbError::MissingToken => Self::bad_request("TMDB token is not configured"),
                TmdbError::InvalidMediaType(_) => {
                    Self::bad_request("media_type must be multi, tv, or movie")
                }
                TmdbError::Http { status, .. } if status.as_u16() == 404 => {
                    Self::not_found("TMDB media not found")
                }
                TmdbError::Http { status, .. } if status.as_u16() == 400 => {
                    Self::bad_request("TMDB rejected the request")
                }
                TmdbError::Http { status, .. } if matches!(status.as_u16(), 401 | 403) => {
                    Self::bad_gateway("TMDB authentication failed")
                }
                TmdbError::Http { status, .. } => {
                    Self::bad_gateway(format!("TMDB request failed with HTTP {}", status.as_u16()))
                }
                TmdbError::Transport(_) | TmdbError::Parse(_) => {
                    Self::bad_gateway("TMDB request failed")
                }
            },
            MediaServiceError::Progression(error) => Self::bad_request(error.to_string()),
            MediaServiceError::Indexer(error) => match error {
                IndexerError::Configuration(_) => {
                    Self::bad_gateway("PT indexer configuration is invalid")
                }
                IndexerError::AuthenticationExpired(_) => {
                    Self::bad_gateway("PT site authentication failed")
                }
                IndexerError::RateLimited(_) => Self::bad_gateway("PT site rate limit reached"),
                IndexerError::Http(_)
                | IndexerError::Api(_)
                | IndexerError::Parse(_)
                | IndexerError::UnsafeUrl(_)
                | IndexerError::InvalidTorrent(_) => Self::bad_gateway("PT site request failed"),
            },
            MediaServiceError::Torrent(_) => {
                Self::bad_gateway("PT site returned an invalid torrent")
            }
            MediaServiceError::NotFound(message) => Self::not_found(message),
            MediaServiceError::Conflict(message) => Self::conflict(message),
            MediaServiceError::Invalid(message) => {
                if message.starts_with("failed to create HTTP client") {
                    Self::bad_request("HTTP client configuration is invalid")
                } else {
                    Self::bad_request(message)
                }
            }
            MediaServiceError::Serialization(_) => Self::internal("failed to serialize media data"),
            MediaServiceError::Downloader(_) => Self::bad_gateway("downloader request failed"),
        }
    }
}

fn media_app_error(error: AppError) -> ApiError {
    match error {
        AppError::InvalidConfig { message } => ApiError::bad_request(message),
        AppError::Database { message } if message.contains("UNIQUE constraint failed") => {
            ApiError::conflict("resource already exists")
        }
        AppError::Database { message } if message.contains("FOREIGN KEY constraint failed") => {
            ApiError::conflict("resource is still in use or references missing data")
        }
        AppError::Database { message }
            if message.contains("CHECK constraint failed")
                || message.contains("NOT NULL constraint failed") =>
        {
            ApiError::bad_request("request violates a data constraint")
        }
        AppError::Database { message }
            if message.contains("database is locked") || message.contains("database is busy") =>
        {
            ApiError::conflict("database is busy; retry the request")
        }
        AppError::Database { .. } => ApiError::internal("media database operation failed"),
        _ => ApiError::internal("media operation failed"),
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

#[cfg(test)]
mod media_api_tests {
    use super::*;

    fn subscription_with_status(status: Option<&str>) -> SubscriptionRecord {
        SubscriptionRecord {
            id: 1,
            tmdb_id: 42,
            media_type: "tv".to_string(),
            title: "Example Show".to_string(),
            original_title: None,
            aliases: Vec::new(),
            year: Some(2026),
            poster_path: None,
            season: Some(1),
            next_episode: Some(3),
            start_episode: Some(3),
            absolute_episode: None,
            quality_profile_id: 1,
            downloader_id: 1,
            site_ids: vec![1],
            save_path: None,
            enabled: true,
            next_run_at: String::new(),
            lease_owner: None,
            lease_until: None,
            version: 0,
            last_status: status.map(str::to_string),
            last_error: None,
            last_run_at: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    async fn test_media_state() -> (tempfile::TempDir, MediaApiState) {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(temp.path()).await.unwrap();
        let downloaders = DownloaderClientPool::new(db.clone());
        let service = MediaService::new(db, downloaders);
        let scheduler = MediaScheduler::new(service.clone());
        (temp, MediaApiState { service, scheduler })
    }

    #[tokio::test]
    async fn settings_handler_never_returns_or_accidentally_clears_token() {
        let (_temp, state) = test_media_state().await;
        let current = state.service.database().get_media_settings().await.unwrap();
        state
            .service
            .database()
            .update_media_settings(&MediaSettings {
                tmdb_token: Some("super-secret-token".to_string()),
                ..current
            })
            .await
            .unwrap();

        let Json(response) = get_media_settings(State(state.clone())).await.unwrap();
        assert!(response.tmdb_token.is_none());
        assert!(response.tmdb_token_configured);
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains("super-secret-token")
        );

        let Json(saved) = update_media_settings(
            State(state.clone()),
            Json(UpdateMediaSettingsRequest {
                tmdb_token: None,
                clear_tmdb_token: false,
                tmdb_language: "zh-CN".to_string(),
                scan_interval_mins: 45,
                max_search_queries: 6,
                search_concurrency: 3,
            }),
        )
        .await
        .unwrap();
        assert!(saved.tmdb_token.is_none());
        assert!(saved.tmdb_token_configured);
        assert_eq!(
            state
                .service
                .database()
                .get_media_settings()
                .await
                .unwrap()
                .tmdb_token
                .as_deref(),
            Some("super-secret-token")
        );
    }

    #[test]
    fn media_service_errors_map_to_stable_http_classes_without_secrets() {
        let not_found = ApiError::from(MediaServiceError::NotFound("download 9".to_string()));
        assert_eq!(not_found.status, StatusCode::NOT_FOUND);

        let conflict = ApiError::from(MediaServiceError::Conflict("leased".to_string()));
        assert_eq!(conflict.status, StatusCode::CONFLICT);

        let upstream = ApiError::from(MediaServiceError::Indexer(
            IndexerError::AuthenticationExpired("cookie=secret".to_string()),
        ));
        assert_eq!(upstream.status, StatusCode::BAD_GATEWAY);
        assert!(!upstream.message.contains("secret"));

        let client = ApiError::from(MediaServiceError::Invalid(
            "failed to create HTTP client: proxy http://user:secret@example.test".to_string(),
        ));
        assert_eq!(client.status, StatusCode::BAD_REQUEST);
        assert!(!client.message.contains("secret"));

        let database = media_app_error(AppError::Database {
            message: "unexpected database detail at /private/path".to_string(),
        });
        assert_eq!(database.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!database.message.contains("private"));
    }

    #[test]
    fn download_filters_validate_status_and_pagination_bounds() {
        assert_eq!(
            validate_download_query(Some(" CANCELLED "), Some(2), Some(50)).unwrap(),
            (Some("cancelled".to_string()), 50, 50)
        );
        assert!(validate_download_query(Some("unknown"), None, None).is_err());
        assert!(validate_download_query(None, Some(0), Some(10)).is_err());
        assert!(validate_download_query(None, Some(1), Some(201)).is_err());
        assert!(validate_download_query(None, Some(usize::MAX), Some(200)).is_err());
    }

    #[test]
    fn completed_subscriptions_reject_run_and_resume() {
        let completed = subscription_with_status(Some("completed"));
        let run = reject_completed_subscription(&completed, "run").unwrap_err();
        let resume = reject_completed_subscription(&completed, "resume").unwrap_err();
        assert_eq!(run.status, StatusCode::CONFLICT);
        assert_eq!(resume.status, StatusCode::CONFLICT);
        assert!(run.message.contains("new cursor"));

        assert!(
            reject_completed_subscription(&subscription_with_status(Some("waiting")), "run")
                .is_ok()
        );
    }

    #[tokio::test]
    async fn download_queue_accepts_only_well_formed_candidate_ids() {
        let (_temp, state) = test_media_state().await;
        let mut valid = QueueDownloadRequest {
            candidate_id: format!("cand_{}", "a".repeat(48)),
            quality_profile_id: 1,
            downloader_id: 1,
            subscription_id: None,
            override_reason: Some(" operator checked ".to_string()),
        };
        normalize_and_validate_download(&state, &mut valid)
            .await
            .unwrap();
        assert_eq!(valid.override_reason.as_deref(), Some("operator checked"));

        valid.candidate_id = "cand_predictable".to_string();
        assert!(
            normalize_and_validate_download(&state, &mut valid)
                .await
                .is_err()
        );
    }

    #[test]
    fn media_router_builds_with_all_route_groups() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (_temp, state) = runtime.block_on(test_media_state());
        let _router = media_router(state.service, state.scheduler);
    }
}

#[cfg(test)]
mod security_boundary_tests {
    use super::*;

    #[test]
    fn site_and_downloader_responses_never_serialize_credentials() {
        let site = SiteResponse::from(SiteWithStats {
            id: 1,
            name: "example".to_string(),
            site_type: "nexusphp".to_string(),
            base_url: "https://tracker.example".to_string(),
            auth_config: r#"{"auth_type":"cookie","cookie":"dummy-site-secret"}"#.to_string(),
            use_proxy: true,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            stats: None,
        });
        let site_json = serde_json::to_value(site).unwrap();
        assert_eq!(site_json["auth_type"], "cookie");
        assert_eq!(site_json["auth_configured"], true);
        assert!(site_json.get("auth_config").is_none());
        assert!(!site_json.to_string().contains("dummy-site-secret"));

        let downloader = DownloaderResponse::from(crate::downloader::DownloaderRecord {
            id: 2,
            name: "qBittorrent".to_string(),
            downloader_type: "qbittorrent".to_string(),
            url: "http://127.0.0.1:8080".to_string(),
            username: "operator".to_string(),
            password: "dummy-downloader-secret".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        });
        let downloader_json = serde_json::to_value(downloader).unwrap();
        assert_eq!(downloader_json["password_configured"], true);
        assert!(downloader_json.get("password").is_none());
        assert!(
            !downloader_json
                .to_string()
                .contains("dummy-downloader-secret")
        );
    }

    #[test]
    fn site_update_preserves_blank_credentials_and_requires_explicit_clear() {
        let existing = crate::site::SiteRecord {
            id: 1,
            name: "example".to_string(),
            site_type: "nexusphp".to_string(),
            base_url: "https://tracker.example".to_string(),
            auth_config: r#"{"auth_type":"cookie","cookie":"dummy-site-secret"}"#.to_string(),
            use_proxy: true,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };

        let preserved = resolve_site_auth_update(
            &existing,
            "nexusphp",
            Some(serde_json::json!({ "auth_type": "cookie", "cookie": "" })),
            false,
        )
        .unwrap();
        assert!(matches!(
            preserved,
            SiteAuth::Cookie { ref cookie } if cookie == "dummy-site-secret"
        ));

        let cleared = resolve_site_auth_update(
            &existing,
            "nexusphp",
            Some(serde_json::json!({ "auth_type": "cookie", "cookie": "" })),
            true,
        )
        .unwrap();
        assert!(matches!(cleared, SiteAuth::Cookie { ref cookie } if cookie.is_empty()));

        assert!(
            resolve_site_auth_update(
                &existing,
                "nexusphp",
                Some(serde_json::json!({ "auth_type": "api_key", "api_key": "" })),
                false,
            )
            .is_err()
        );
        assert!(
            resolve_site_auth_update(
                &existing,
                "mteam",
                Some(serde_json::json!({
                    "auth_type": "api_key",
                    "api_key": "dummy-new-secret"
                })),
                false,
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn downloader_update_keeps_password_until_clear_is_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(temp.path()).await.unwrap();
        let id = db
            .create_downloader(
                "qBittorrent",
                "qbittorrent",
                "http://127.0.0.1:8080",
                "operator",
                "dummy-downloader-secret",
            )
            .await
            .unwrap();
        let existing = db.get_downloader(id).await.unwrap().unwrap();

        let preserved =
            resolve_downloader_password(&existing.password, Some(String::new()), false).unwrap();
        db.update_downloader(
            id,
            &existing.name,
            &existing.downloader_type,
            &existing.url,
            &existing.username,
            &preserved,
        )
        .await
        .unwrap();
        assert_eq!(
            db.get_downloader(id).await.unwrap().unwrap().password,
            "dummy-downloader-secret"
        );

        let cleared = resolve_downloader_password(&preserved, None, true).unwrap();
        db.update_downloader(
            id,
            &existing.name,
            &existing.downloader_type,
            &existing.url,
            &existing.username,
            &cleared,
        )
        .await
        .unwrap();
        assert!(
            db.get_downloader(id)
                .await
                .unwrap()
                .unwrap()
                .password
                .is_empty()
        );
    }

    #[test]
    fn cors_is_limited_to_local_vite_origins() {
        assert_eq!(
            VITE_DEV_ORIGINS,
            [
                "http://localhost:5173",
                "http://127.0.0.1:5173",
                "http://[::1]:5173",
            ]
        );
        assert!(!VITE_DEV_ORIGINS.contains(&"https://third-party.example"));
        let _layer = cors_layer();
    }
}
