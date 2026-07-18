use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use tracing::{Instrument, warn};

use crate::db::Database;
use crate::downloader::{AddTorrentOptions, DownloaderClientPool};
use crate::error::AppError;
use crate::indexer::{
    IndexerAggregator, IndexerError, IndexerPool, SearchRequest, SearchResult, SiteSearchError,
};
use crate::net::client_factory;

use super::domain::{
    DecisionEngine, MatchDecision, MediaTarget, QualityProfile, QueryGenerator, RejectCode,
    ReleaseInfo, ReleaseParser, SearchCriteria, SeasonEpisode, SortKey, stable_release_key,
};
use super::lease::process_owner_id;
use super::models::{
    MEDIA_DOWNLOAD_RECONCILIATION_GRACE_SECONDS, MediaDownloadRecord, NewMediaDownload,
    NewSubscription, QualityProfileRecord, SubscriptionRecord, SubscriptionTargetRecord,
    media_download_category,
};
use super::progression::{
    ProgressionError, SubscriptionTargetSeedStatus, TargetReadiness, air_date_eligible_at,
    next_run_at, plan_tv_targets, target_readiness,
};
use super::tmdb::{TmdbClient, TmdbDetails, TmdbError, TmdbMedia, TmdbMediaType, TmdbSeason};
use super::torrent::{TorrentMetadataError, torrent_infohash};

const SUBSCRIPTION_LEASE_SECONDS: i64 = 10 * 60;
const DOWNLOAD_LEASE_SECONDS: i64 = 5 * 60;
const CANDIDATE_CACHE_TTL: StdDuration = StdDuration::from_secs(10 * 60);
const CANDIDATE_CACHE_CAPACITY: usize = 2_048;
const RESOURCE_SEARCH_RESULT_LIMIT: usize = 512;
const _: () = assert!(RESOURCE_SEARCH_RESULT_LIMIT <= CANDIDATE_CACHE_CAPACITY);

#[derive(Debug, thiserror::Error)]
pub enum MediaServiceError {
    #[error(transparent)]
    App(#[from] AppError),
    #[error(transparent)]
    Tmdb(#[from] TmdbError),
    #[error(transparent)]
    Progression(#[from] ProgressionError),
    #[error(transparent)]
    Indexer(#[from] IndexerError),
    #[error(transparent)]
    Torrent(#[from] TorrentMetadataError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("downloader error: {0}")]
    Downloader(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSearchRequest {
    pub query: Option<String>,
    #[serde(default)]
    pub site_ids: Vec<i64>,
    pub target: Option<MediaTarget>,
    pub quality_profile_id: Option<i64>,
    pub page_size: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceCandidate {
    pub candidate_id: String,
    pub result: SearchResult,
    pub release: Option<ReleaseInfo>,
    pub parse_error: Option<String>,
    pub decision: Option<MatchDecision>,
    pub sort_key: Option<SortKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSearchResponse {
    pub queries: Vec<String>,
    pub candidates: Vec<ResourceCandidate>,
    pub errors: Vec<SiteSearchError>,
    pub total_sites: usize,
    pub successful_sites: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSubscriptionRequest {
    pub tmdb_id: i64,
    pub media_type: String,
    pub season: Option<u32>,
    pub start_episode: Option<u32>,
    pub absolute_episode: Option<u32>,
    pub quality_profile_id: i64,
    pub downloader_id: i64,
    #[serde(default)]
    pub site_ids: Vec<i64>,
    pub save_path: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueDownloadRequest {
    pub candidate_id: String,
    pub quality_profile_id: i64,
    pub downloader_id: i64,
    pub subscription_id: Option<i64>,
    pub override_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionRunResult {
    pub subscription_id: i64,
    pub target_key: String,
    pub query_count: usize,
    pub candidate_count: usize,
    pub accepted_count: usize,
    pub download: Option<MediaDownloadRecord>,
    pub site_errors: Vec<SiteSearchError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionRunSnapshot {
    pub started_at: String,
    pub finished_at: String,
    pub target_key: String,
    pub queries: Vec<String>,
    pub candidates: Vec<ResourceCandidate>,
    pub site_errors: Vec<SiteSearchError>,
    pub total_sites: usize,
    pub successful_sites: usize,
    pub best_candidate_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct MediaService {
    db: Database,
    indexers: Arc<IndexerPool>,
    downloaders: Arc<DownloaderClientPool>,
    parser: ReleaseParser,
    candidate_cache: Arc<tokio::sync::Mutex<CandidateCache>>,
}

#[derive(Clone)]
struct CachedCandidate {
    result: SearchResult,
    target: Option<MediaTarget>,
    quality_profile_id: Option<i64>,
    expires_at: Instant,
}

#[derive(Default)]
struct CandidateCache {
    entries: HashMap<String, CachedCandidate>,
    insertion_order: VecDeque<String>,
}

impl CandidateCache {
    fn insert(
        &mut self,
        candidate_id: String,
        result: SearchResult,
        target: Option<MediaTarget>,
        quality_profile_id: Option<i64>,
    ) {
        self.prune_expired();
        while self.entries.len() >= CANDIDATE_CACHE_CAPACITY {
            let Some(oldest) = self.insertion_order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.insertion_order.push_back(candidate_id.clone());
        self.entries.insert(
            candidate_id,
            CachedCandidate {
                result,
                target,
                quality_profile_id,
                expires_at: Instant::now() + CANDIDATE_CACHE_TTL,
            },
        );
    }

    fn get(&mut self, candidate_id: &str) -> Option<CachedCandidate> {
        self.prune_expired();
        self.entries.get(candidate_id).cloned()
    }

    fn prune_expired(&mut self) {
        let now = Instant::now();
        self.entries.retain(|_, entry| entry.expires_at > now);
        self.insertion_order
            .retain(|candidate_id| self.entries.contains_key(candidate_id));
    }
}

enum PreparedSubscriptionTarget {
    Due(MediaTarget),
    Deferred {
        target_key: String,
        next_run_at: String,
        status: &'static str,
    },
    Completed {
        target_key: String,
    },
}

impl MediaService {
    pub fn new(db: Database, downloaders: Arc<DownloaderClientPool>) -> Arc<Self> {
        Arc::new(Self {
            db,
            indexers: IndexerPool::new(),
            downloaders,
            parser: ReleaseParser::default(),
            candidate_cache: Arc::new(tokio::sync::Mutex::new(CandidateCache::default())),
        })
    }

    pub fn database(&self) -> &Database {
        &self.db
    }

    pub async fn tmdb_search(
        &self,
        query: &str,
        media_type: Option<&str>,
    ) -> Result<Vec<TmdbMedia>, MediaServiceError> {
        self.tmdb_client()
            .await?
            .search(query, media_type)
            .await
            .map_err(Into::into)
    }

    pub async fn tmdb_details(
        &self,
        tmdb_id: i64,
        media_type: TmdbMediaType,
    ) -> Result<TmdbDetails, MediaServiceError> {
        self.tmdb_client()
            .await?
            .details(tmdb_id, media_type)
            .await
            .map_err(Into::into)
    }

    pub async fn tmdb_season(
        &self,
        tmdb_id: i64,
        season: u32,
    ) -> Result<TmdbSeason, MediaServiceError> {
        self.tmdb_client()
            .await?
            .season(tmdb_id, season)
            .await
            .map_err(Into::into)
    }

    pub async fn create_subscription(
        &self,
        request: &CreateSubscriptionRequest,
    ) -> Result<SubscriptionRecord, MediaServiceError> {
        if request.site_ids.is_empty() {
            return Err(MediaServiceError::Invalid(
                "at least one PT site must be selected".to_string(),
            ));
        }
        let media_type = TmdbMediaType::parse(&request.media_type)?;
        if media_type == TmdbMediaType::Tv && request.season.is_none() {
            return Err(MediaServiceError::Invalid(
                "season is required for a TV subscription".to_string(),
            ));
        }
        self.ensure_references(
            request.quality_profile_id,
            request.downloader_id,
            &request.site_ids,
        )
        .await?;
        let (details, target_plan) = if media_type == TmdbMediaType::Tv {
            let season_number = request.season.expect("TV season was validated");
            let client = self.tmdb_client().await?;
            let (details, season) = tokio::try_join!(
                client.details(request.tmdb_id, media_type.clone()),
                client.season(request.tmdb_id, season_number),
            )?;
            let plan = plan_tv_targets(
                &details,
                &season,
                request.start_episode.unwrap_or(1),
                request.absolute_episode,
            )?;
            (details, Some(plan))
        } else {
            (
                self.tmdb_details(request.tmdb_id, media_type.clone())
                    .await?,
                None,
            )
        };
        let mut aliases = details.aliases.clone();
        if let Some(original) = &details.media.original_title {
            push_case_insensitive(&mut aliases, original);
        }
        let subscription = NewSubscription {
            tmdb_id: details.media.tmdb_id,
            media_type: media_type.as_str().to_string(),
            tmdb_is_animation: details.media.is_animation,
            title: details.media.title,
            original_title: details.media.original_title,
            aliases,
            year: details.media.year,
            poster_path: details.media.poster_path,
            season: request.season,
            start_episode: request
                .start_episode
                .or((media_type == TmdbMediaType::Tv).then_some(1)),
            absolute_episode: request.absolute_episode,
            quality_profile_id: request.quality_profile_id,
            downloader_id: request.downloader_id,
            site_ids: request.site_ids.clone(),
            save_path: request.save_path.clone(),
            enabled: request.enabled,
        };
        if let Some(plan) = target_plan {
            let settings = self.db.get_media_settings().await?;
            let now = Utc::now();
            let current = plan
                .targets
                .iter()
                .find(|target| target.status != SubscriptionTargetSeedStatus::Skipped)
                .ok_or_else(|| {
                    MediaServiceError::Invalid(
                        "TMDB season has no episode at or after the requested start".to_string(),
                    )
                })?;
            let readiness = target_readiness(
                current.status,
                current.air_date.as_deref(),
                plan.terminal,
                now,
            );
            let scheduled_at = next_run_at(
                &readiness,
                now,
                Duration::minutes(settings.scan_interval_mins as i64),
            )?
            .to_rfc3339();
            let initial_status = readiness_status(&readiness);
            self.db
                .create_subscription_with_targets(
                    &subscription,
                    &plan.targets,
                    Some(&scheduled_at),
                    Some(initial_status),
                )
                .await
                .map_err(Into::into)
        } else {
            self.db
                .create_subscription(&subscription)
                .await
                .map_err(Into::into)
        }
    }

    pub async fn search_resources(
        &self,
        request: &ResourceSearchRequest,
    ) -> Result<ResourceSearchResponse, MediaServiceError> {
        self.search_resources_internal(request, true).await
    }

    async fn search_resources_uncached(
        &self,
        request: &ResourceSearchRequest,
    ) -> Result<ResourceSearchResponse, MediaServiceError> {
        self.search_resources_internal(request, false).await
    }

    async fn search_resources_internal(
        &self,
        request: &ResourceSearchRequest,
        cache_candidates: bool,
    ) -> Result<ResourceSearchResponse, MediaServiceError> {
        let media_settings = self.db.get_media_settings().await?;
        let profile = match (request.target.as_ref(), request.quality_profile_id) {
            (Some(_), id) => Some(
                self.db
                    .get_quality_profile(id.unwrap_or(1))
                    .await?
                    .ok_or_else(|| {
                        MediaServiceError::NotFound(format!("quality profile {}", id.unwrap_or(1)))
                    })?,
            ),
            (None, _) => None,
        };
        let queries = if let Some(query) = request
            .query
            .as_deref()
            .map(str::trim)
            .filter(|query| !query.is_empty())
        {
            vec![query.to_string()]
        } else if let Some(target) = &request.target {
            QueryGenerator::new(media_settings.max_search_queries)
                .generate(&SearchCriteria::from(target))
                .into_iter()
                .map(|query| query.query)
                .collect()
        } else {
            return Err(MediaServiceError::Invalid(
                "query or target is required".to_string(),
            ));
        };
        let page_size = request.page_size.unwrap_or(30).clamp(1, 100);
        let search_requests: Vec<_> = queries
            .iter()
            .map(|query| SearchRequest {
                query: query.clone(),
                page: 1,
                page_size,
            })
            .collect();
        let (indexers, mut setup_errors, priorities) =
            self.resolve_indexers(&request.site_ids).await?;
        let aggregate = IndexerAggregator::new(media_settings.search_concurrency)
            .search(&indexers, &search_requests)
            .await;
        setup_errors.extend(aggregate.errors.clone());
        let candidates = self
            .finalize_resource_candidates(
                aggregate.results,
                request,
                profile.as_ref(),
                &priorities,
                cache_candidates,
            )
            .await?;
        Ok(ResourceSearchResponse {
            queries,
            candidates,
            errors: setup_errors,
            total_sites: aggregate.total_sites,
            successful_sites: aggregate.successful_sites,
        })
    }

    async fn finalize_resource_candidates(
        &self,
        results: Vec<SearchResult>,
        request: &ResourceSearchRequest,
        profile: Option<&QualityProfileRecord>,
        priorities: &HashMap<i64, u32>,
        cache_candidates: bool,
    ) -> Result<Vec<ResourceCandidate>, MediaServiceError> {
        let domain_profile = profile.map(profile_to_domain);
        let mut candidates = Vec::with_capacity(results.len().min(RESOURCE_SEARCH_RESULT_LIMIT));
        for result in results {
            match self.parser.parse(&result.title) {
                Ok(release) => {
                    let decision = request.target.as_ref().zip(domain_profile.as_ref()).map(
                        |(target, profile)| {
                            DecisionEngine::evaluate(target, &release, profile, result.seeders)
                        },
                    );
                    let sort_key = decision.as_ref().map(|decision| {
                        SortKey::from_decision(
                            decision,
                            result.seeders,
                            result.publish_time,
                            priorities.get(&result.site_id).copied().unwrap_or(u32::MAX),
                            stable_release_key(
                                &result.source_site,
                                &result.torrent_id,
                                &result.title,
                            ),
                        )
                    });
                    candidates.push(ResourceCandidate {
                        candidate_id: String::new(),
                        result,
                        release: Some(release),
                        parse_error: None,
                        decision,
                        sort_key,
                    });
                }
                Err(error) => candidates.push(ResourceCandidate {
                    candidate_id: String::new(),
                    result,
                    release: None,
                    parse_error: Some(error.to_string()),
                    decision: None,
                    sort_key: None,
                }),
            }
        }

        candidates.sort_by(compare_candidates);
        candidates.truncate(RESOURCE_SEARCH_RESULT_LIMIT);
        for candidate in &mut candidates {
            candidate.candidate_id = new_candidate_id()?;
        }

        if cache_candidates {
            let mut cache = self.candidate_cache.lock().await;
            for candidate in &candidates {
                cache.insert(
                    candidate.candidate_id.clone(),
                    candidate.result.clone(),
                    request.target.clone(),
                    profile.map(|profile| profile.id),
                );
            }
        }
        Ok(candidates)
    }

    pub async fn run_subscription(
        &self,
        subscription_id: i64,
    ) -> Result<SubscriptionRunResult, MediaServiceError> {
        let owner = process_owner_id("manual-subscription");
        let record = self
            .db
            .claim_subscription(subscription_id, &owner, SUBSCRIPTION_LEASE_SECONDS)
            .await?
            .ok_or_else(|| {
                MediaServiceError::Conflict(
                    "subscription is already running or does not exist".to_string(),
                )
            })?;
        let service = self.clone();
        let task_owner = owner.clone();
        let task = tokio::spawn(
            async move {
                service
                    .run_claimed_subscription(record, &task_owner, SUBSCRIPTION_LEASE_SECONDS)
                    .await
            }
            .in_current_span(),
        );
        match task.await {
            Ok(result) => result,
            Err(error) => {
                let _ = self
                    .db
                    .recover_subscription_leases_for_owners(&[owner])
                    .await;
                Err(MediaServiceError::App(AppError::Server {
                    message: format!("manual subscription scan task terminated: {error}"),
                }))
            }
        }
    }

    pub async fn get_subscription_last_run(
        &self,
        subscription_id: i64,
    ) -> Result<Option<SubscriptionRunSnapshot>, MediaServiceError> {
        self.db
            .get_subscription_last_run_info(subscription_id)
            .await?
            .map(|info| {
                serde_json::from_str(&info).map_err(|error| {
                    MediaServiceError::Serialization(format!(
                        "invalid subscription run snapshot: {error}"
                    ))
                })
            })
            .transpose()
    }

    pub async fn run_claimed_subscription(
        &self,
        subscription: SubscriptionRecord,
        owner: &str,
        lease_seconds: i64,
    ) -> Result<SubscriptionRunResult, MediaServiceError> {
        let subscription_id = subscription.id;
        let claimed_target_key = subscription_target(&subscription)?.target_key();
        let started_at = Utc::now().to_rfc3339();
        let result = self
            .run_claimed_subscription_inner(subscription, owner, lease_seconds, &started_at)
            .await;
        if let Err(error) = &result {
            let current_snapshot = self
                .get_subscription_last_run(subscription_id)
                .await
                .ok()
                .flatten();
            let snapshot = match current_snapshot {
                Some(mut snapshot) if snapshot.started_at == started_at => {
                    snapshot.finished_at = Utc::now().to_rfc3339();
                    snapshot.error = Some(error.to_string());
                    snapshot
                }
                _ => failed_run_snapshot(&started_at, &claimed_target_key, error.to_string()),
            };
            if let Ok(info) = to_json(&snapshot) {
                let _ = self
                    .db
                    .save_claimed_subscription_last_run_info(subscription_id, owner, &info)
                    .await;
            }
            let retry_minutes = self
                .db
                .get_media_settings()
                .await
                .map(|settings| settings.scan_interval_mins as i64)
                .unwrap_or(30)
                .max(1);
            let next_run_at = (Utc::now() + Duration::minutes(retry_minutes)).to_rfc3339();
            let _ = self
                .db
                .release_claimed_subscription_after_error(
                    subscription_id,
                    owner,
                    &claimed_target_key,
                    &next_run_at,
                    &error.to_string(),
                )
                .await;
        }
        result
    }

    async fn run_claimed_subscription_inner(
        &self,
        mut subscription: SubscriptionRecord,
        owner: &str,
        lease_seconds: i64,
        started_at: &str,
    ) -> Result<SubscriptionRunResult, MediaServiceError> {
        let settings = self.db.get_media_settings().await?;
        let scan_interval = Duration::minutes(settings.scan_interval_mins as i64);
        let fallback_next_run_at = (Utc::now() + scan_interval).to_rfc3339();
        let prepared = match self
            .prepare_claimed_subscription_target(
                &mut subscription,
                owner,
                scan_interval,
                started_at,
            )
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let snapshot = failed_run_snapshot(
                    started_at,
                    &subscription_target(&subscription)?.target_key(),
                    error.to_string(),
                );
                self.save_claimed_run_snapshot(subscription.id, owner, &snapshot)
                    .await?;
                let _ = self
                    .db
                    .finish_subscription_scan(
                        subscription.id,
                        subscription.version,
                        owner,
                        &fallback_next_run_at,
                        "error",
                        Some(&error.to_string()),
                    )
                    .await;
                return Err(error);
            }
        };
        let mut target = match prepared {
            PreparedSubscriptionTarget::Due(target) => target,
            PreparedSubscriptionTarget::Deferred {
                target_key,
                next_run_at,
                status,
            } => {
                let snapshot = empty_run_snapshot(started_at, &target_key);
                self.save_claimed_run_snapshot(subscription.id, owner, &snapshot)
                    .await?;
                if !self
                    .db
                    .finish_subscription_scan(
                        subscription.id,
                        subscription.version,
                        owner,
                        &next_run_at,
                        status,
                        None,
                    )
                    .await?
                {
                    return Err(MediaServiceError::Conflict(
                        "subscription lease expired while deferring the scan".to_string(),
                    ));
                }
                return Ok(empty_subscription_run(subscription.id, target_key));
            }
            PreparedSubscriptionTarget::Completed { target_key } => {
                return Ok(empty_subscription_run(subscription.id, target_key));
            }
        };
        let profile = self
            .db
            .get_quality_profile(subscription.quality_profile_id)
            .await?
            .ok_or_else(|| {
                MediaServiceError::NotFound(format!(
                    "quality profile {}",
                    subscription.quality_profile_id
                ))
            })?;
        let (search, mut lease_version) = self
            .search_resources_with_subscription_heartbeat(
                &subscription,
                owner,
                lease_seconds,
                ResourceSearchRequest {
                    query: None,
                    site_ids: subscription.site_ids.clone(),
                    target: Some(target.clone()),
                    quality_profile_id: Some(profile.id),
                    page_size: Some(50),
                },
            )
            .await?;
        let next_run_at = (Utc::now() + scan_interval).to_rfc3339();

        let mut search = match search {
            Ok(search) => search,
            Err(error) => {
                let snapshot =
                    failed_run_snapshot(started_at, &target.target_key(), error.to_string());
                self.save_claimed_run_snapshot(subscription.id, owner, &snapshot)
                    .await?;
                let _ = self
                    .db
                    .finish_subscription_scan(
                        subscription.id,
                        lease_version,
                        owner,
                        &next_run_at,
                        "error",
                        Some(&error.to_string()),
                    )
                    .await;
                return Err(error);
            }
        };
        lease_version = self
            .refresh_aliases_after_title_only_rejection(
                &mut subscription,
                owner,
                lease_version,
                &mut target,
                &mut search,
                &profile,
            )
            .await?;
        let accepted_count = search
            .candidates
            .iter()
            .filter(|candidate| candidate.decision.as_ref().is_some_and(|d| d.accepted))
            .count();
        let best = search.candidates.iter().find(|candidate| {
            candidate
                .decision
                .as_ref()
                .is_some_and(|decision| decision.accepted)
        });
        let snapshot = SubscriptionRunSnapshot {
            started_at: started_at.to_string(),
            finished_at: Utc::now().to_rfc3339(),
            target_key: target.target_key(),
            queries: search.queries.clone(),
            candidates: search.candidates.clone(),
            site_errors: search.errors.clone(),
            total_sites: search.total_sites,
            successful_sites: search.successful_sites,
            best_candidate_id: best.map(|candidate| candidate.candidate_id.clone()),
            error: None,
        };
        self.save_claimed_run_snapshot(subscription.id, owner, &snapshot)
            .await?;
        let download = if let Some(best) = best {
            let downloader = self
                .db
                .get_downloader(subscription.downloader_id)
                .await?
                .ok_or_else(|| {
                    MediaServiceError::NotFound(format!(
                        "downloader {}",
                        subscription.downloader_id
                    ))
                })?;
            let decision = best
                .decision
                .as_ref()
                .expect("accepted candidate has decision");
            let target_key = target.target_key();
            let request = NewMediaDownload {
                subscription_id: Some(subscription.id),
                target_key: target_key.clone(),
                dedupe_key: format!("subscription:{}:{target_key}", subscription.id),
                site_id: Some(best.result.site_id),
                downloader_id: Some(subscription.downloader_id),
                source_site: best.result.source_site.clone(),
                downloader_name: downloader.name,
                torrent_id: best.result.torrent_id.clone(),
                title: best.result.title.clone(),
                size: best.result.size,
                release_json: to_json(&best.result)?,
                decision_json: to_json(decision)?,
                profile_snapshot_json: to_json(&profile)?,
            };
            let download = self
                .db
                .enqueue_claimed_subscription_media_download(lease_version, owner, &request)
                .await?
                .ok_or_else(|| {
                    MediaServiceError::Conflict(
                        "subscription lease or target changed before queueing".to_string(),
                    )
                })?;
            Some(download)
        } else {
            None
        };
        let status = if download.is_some() {
            "queued"
        } else {
            "waiting"
        };
        let error_summary = if download.is_none() && !search.errors.is_empty() {
            Some(
                search
                    .errors
                    .iter()
                    .map(|error| format!("{}: {}", error.source_site, error.message))
                    .take(3)
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        } else {
            None
        };
        if !self
            .db
            .finish_subscription_scan(
                subscription.id,
                lease_version,
                owner,
                &next_run_at,
                status,
                error_summary.as_deref(),
            )
            .await?
        {
            return Err(MediaServiceError::Conflict(
                "subscription lease expired while finishing the scan".to_string(),
            ));
        }
        Ok(SubscriptionRunResult {
            subscription_id: subscription.id,
            target_key: target.target_key(),
            query_count: search.queries.len(),
            candidate_count: search.candidates.len(),
            accepted_count,
            download,
            site_errors: search.errors,
        })
    }

    async fn save_claimed_run_snapshot(
        &self,
        subscription_id: i64,
        owner: &str,
        snapshot: &SubscriptionRunSnapshot,
    ) -> Result<(), MediaServiceError> {
        let info = to_json(snapshot)?;
        if self
            .db
            .save_claimed_subscription_last_run_info(subscription_id, owner, &info)
            .await?
        {
            Ok(())
        } else {
            Err(MediaServiceError::Conflict(
                "subscription lease expired while saving run details".to_string(),
            ))
        }
    }

    async fn refresh_aliases_after_title_only_rejection(
        &self,
        subscription: &mut SubscriptionRecord,
        owner: &str,
        lease_version: i64,
        target: &mut MediaTarget,
        search: &mut ResourceSearchResponse,
        profile: &QualityProfileRecord,
    ) -> Result<i64, MediaServiceError> {
        let title_only_rejection = search.candidates.iter().any(|candidate| {
            candidate.decision.as_ref().is_some_and(|decision| {
                !decision.accepted
                    && decision.rejections.len() == 1
                    && decision.rejections[0].code == RejectCode::WrongTitle
            })
        });
        if !title_only_rejection {
            return Ok(lease_version);
        }

        let media_type = TmdbMediaType::parse(&subscription.media_type)?;
        let details = match self.tmdb_details(subscription.tmdb_id, media_type).await {
            Ok(details) => details,
            Err(error) => {
                warn!(
                    subscription_id = subscription.id,
                    %error,
                    "could not refresh TMDB aliases after title-only rejection"
                );
                return Ok(lease_version);
            }
        };
        let mut aliases = subscription.aliases.clone();
        push_case_insensitive(&mut aliases, &details.media.title);
        if let Some(original_title) = &details.media.original_title {
            push_case_insensitive(&mut aliases, original_title);
        }
        for alias in &details.aliases {
            push_case_insensitive(&mut aliases, alias);
        }
        if aliases.len() == subscription.aliases.len() {
            return Ok(lease_version);
        }

        subscription.aliases = aliases;
        let refreshed_target = subscription_target(subscription)?;
        reevaluate_resource_candidates(&refreshed_target, &mut search.candidates, profile);
        *target = refreshed_target;

        let refreshed_version = self
            .db
            .refresh_claimed_subscription_aliases(
                subscription.id,
                lease_version,
                owner,
                &subscription.aliases,
            )
            .await?
            .ok_or_else(|| {
                MediaServiceError::Conflict(
                    "subscription lease changed while refreshing TMDB aliases".to_string(),
                )
            })?;
        subscription.version = refreshed_version;
        Ok(refreshed_version)
    }

    async fn prepare_claimed_subscription_target(
        &self,
        subscription: &mut SubscriptionRecord,
        owner: &str,
        scan_interval: Duration,
        started_at: &str,
    ) -> Result<PreparedSubscriptionTarget, MediaServiceError> {
        let target = subscription_target(subscription)?;
        let target_key = target.target_key();
        let now = Utc::now();
        let mut target_record = self
            .db
            .get_subscription_target(subscription.id, &target_key)
            .await?;
        let mut terminal = subscription.media_type == "movie"
            || target_record
                .as_ref()
                .is_some_and(|record| record.status == "skipped");

        if subscription.media_type == "tv"
            && target_needs_tmdb_refresh(
                target_record.as_ref(),
                subscription.last_status.as_deref(),
                now,
            )
        {
            let season_number = subscription.season.ok_or_else(|| {
                MediaServiceError::Invalid("TV subscription has no season".to_string())
            })?;
            let start_episode = subscription.start_episode.unwrap_or(1);
            let absolute_anchor = subscription_absolute_anchor(subscription)?;
            let client = self.tmdb_client().await?;
            let (details, season) = tokio::try_join!(
                client.details(subscription.tmdb_id, TmdbMediaType::Tv),
                client.season(subscription.tmdb_id, season_number),
            )?;
            let plan = plan_tv_targets(&details, &season, start_episode, absolute_anchor)?;
            terminal = plan.terminal;
            let synced = self
                .db
                .sync_claimed_subscription_targets(
                    subscription.id,
                    subscription.version,
                    owner,
                    &plan.targets,
                    details.media.is_animation,
                )
                .await?
                .ok_or_else(|| {
                    MediaServiceError::Conflict(
                        "subscription changed while refreshing TMDB targets".to_string(),
                    )
                })?;
            subscription.version = synced.version;
            subscription.tmdb_is_animation = details.media.is_animation;
            target_record = synced.current;
        }

        let target_record = target_record.ok_or_else(|| {
            MediaServiceError::Conflict(format!(
                "subscription target {target_key} is missing after metadata refresh"
            ))
        })?;
        match target_record.status.as_str() {
            "skipped" => {
                if !terminal {
                    return Err(MediaServiceError::Conflict(
                        "a non-terminal season has a skipped current target".to_string(),
                    ));
                }
                let snapshot = empty_run_snapshot(started_at, &target_key);
                self.save_claimed_run_snapshot(subscription.id, owner, &snapshot)
                    .await?;
                if !self
                    .db
                    .complete_claimed_subscription(
                        subscription.id,
                        subscription.version,
                        owner,
                        Some(&target_key),
                    )
                    .await?
                {
                    return Err(MediaServiceError::Conflict(
                        "subscription changed while completing the season".to_string(),
                    ));
                }
                Ok(PreparedSubscriptionTarget::Completed { target_key })
            }
            "queued" => Ok(PreparedSubscriptionTarget::Deferred {
                target_key,
                next_run_at: (now + scan_interval).to_rfc3339(),
                status: "queued",
            }),
            "metadata_pending" => Ok(PreparedSubscriptionTarget::Deferred {
                target_key,
                next_run_at: next_run_at(&TargetReadiness::AwaitingMetadata, now, scan_interval)?
                    .to_rfc3339(),
                status: "awaiting_metadata",
            }),
            "pending" => {
                let readiness = target_readiness(
                    SubscriptionTargetSeedStatus::Pending,
                    target_record.air_date.as_deref(),
                    terminal,
                    now,
                );
                match readiness {
                    TargetReadiness::Due => Ok(PreparedSubscriptionTarget::Due(target)),
                    TargetReadiness::Future(_) => Ok(PreparedSubscriptionTarget::Deferred {
                        target_key,
                        next_run_at: next_run_at(&readiness, now, scan_interval)?.to_rfc3339(),
                        status: "waiting_air_date",
                    }),
                    TargetReadiness::AwaitingMetadata => Ok(PreparedSubscriptionTarget::Deferred {
                        target_key,
                        next_run_at: next_run_at(&readiness, now, scan_interval)?.to_rfc3339(),
                        status: "awaiting_metadata",
                    }),
                }
            }
            "submitted" => Err(MediaServiceError::Conflict(
                "subscription cursor still points at a submitted target".to_string(),
            )),
            status => Err(MediaServiceError::Invalid(format!(
                "unsupported subscription target status: {status}"
            ))),
        }
    }

    async fn search_resources_with_subscription_heartbeat(
        &self,
        subscription: &SubscriptionRecord,
        owner: &str,
        lease_seconds: i64,
        request: ResourceSearchRequest,
    ) -> Result<(Result<ResourceSearchResponse, MediaServiceError>, i64), MediaServiceError> {
        let lease_seconds = lease_seconds.max(10);
        let heartbeat_seconds = (lease_seconds / 3).max(1) as u64;
        let mut version = subscription.version;
        let search = self.search_resources_uncached(&request);
        tokio::pin!(search);

        loop {
            tokio::select! {
                result = &mut search => return Ok((result, version)),
                _ = tokio::time::sleep(std::time::Duration::from_secs(heartbeat_seconds)) => {
                    version = self.db
                        .renew_subscription_lease(
                            subscription.id,
                            version,
                            owner,
                            lease_seconds,
                        )
                        .await?
                        .ok_or_else(|| MediaServiceError::Conflict(
                            "subscription lease changed during the scan".to_string()
                        ))?;
                }
            }
        }
    }

    pub async fn queue_download(
        &self,
        request: &QueueDownloadRequest,
    ) -> Result<MediaDownloadRecord, MediaServiceError> {
        let candidate_id = request.candidate_id.trim();
        if candidate_id.is_empty() {
            return Err(MediaServiceError::Invalid(
                "candidate_id is required".to_string(),
            ));
        }
        let cached = self
            .candidate_cache
            .lock()
            .await
            .get(candidate_id)
            .ok_or_else(|| {
                MediaServiceError::Invalid(
                    "candidate_id is unknown or expired; search again".to_string(),
                )
            })?;
        let override_reason = request
            .override_reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty());
        let mut result = cached.result;
        let (
            quality_profile_id,
            downloader_id,
            target_key,
            dedupe_key,
            decision,
            subscription_version,
        ) = if let Some(subscription_id) = request.subscription_id {
            let subscription = self
                .db
                .get_subscription(subscription_id)
                .await?
                .ok_or_else(|| {
                    MediaServiceError::NotFound(format!("subscription {subscription_id}"))
                })?;
            if request.quality_profile_id != subscription.quality_profile_id
                || request.downloader_id != subscription.downloader_id
            {
                return Err(MediaServiceError::Invalid(
                    "quality profile and downloader must match the subscription".to_string(),
                ));
            }
            if !subscription.site_ids.contains(&result.site_id) {
                return Err(MediaServiceError::Invalid(
                    "download site is not configured for the subscription".to_string(),
                ));
            }
            let target = subscription_target(&subscription)?;
            if cached
                .target
                .as_ref()
                .map(MediaTarget::target_key)
                .as_deref()
                != Some(target.target_key().as_str())
                || cached.quality_profile_id != Some(subscription.quality_profile_id)
            {
                return Err(MediaServiceError::Invalid(
                        "candidate was not issued for the subscription's current target and quality profile"
                            .to_string(),
                    ));
            }
            let profile = self
                .db
                .get_quality_profile(subscription.quality_profile_id)
                .await?
                .ok_or_else(|| {
                    MediaServiceError::NotFound(format!(
                        "quality profile {}",
                        subscription.quality_profile_id
                    ))
                })?;
            let release = self.parser.parse(&result.title).map_err(|error| {
                MediaServiceError::Invalid(format!(
                    "release title could not be matched to the subscription: {error}"
                ))
            })?;
            let decision = DecisionEngine::evaluate(
                &target,
                &release,
                &profile_to_domain(&profile),
                result.seeders,
            );
            if !decision.accepted && override_reason.is_none() {
                return Err(MediaServiceError::Invalid(
                    "override_reason is required for a rejected candidate".to_string(),
                ));
            }
            let target_key = target.target_key();
            (
                subscription.quality_profile_id,
                subscription.downloader_id,
                target_key.clone(),
                format!("subscription:{subscription_id}:{target_key}"),
                Some(decision),
                Some(subscription.version),
            )
        } else {
            if cached
                .quality_profile_id
                .is_some_and(|profile_id| profile_id != request.quality_profile_id)
            {
                return Err(MediaServiceError::Invalid(
                    "quality profile does not match the candidate search context".to_string(),
                ));
            }
            let decision = if let Some(target) = cached.target.as_ref() {
                let profile = self
                    .db
                    .get_quality_profile(request.quality_profile_id)
                    .await?
                    .ok_or_else(|| {
                        MediaServiceError::NotFound(format!(
                            "quality profile {}",
                            request.quality_profile_id
                        ))
                    })?;
                let release = self.parser.parse(&result.title).map_err(|error| {
                    MediaServiceError::Invalid(format!(
                        "release title could not be matched to the search target: {error}"
                    ))
                })?;
                Some(DecisionEngine::evaluate(
                    target,
                    &release,
                    &profile_to_domain(&profile),
                    result.seeders,
                ))
            } else {
                None
            };
            if decision.as_ref().is_some_and(|decision| !decision.accepted)
                && override_reason.is_none()
            {
                return Err(MediaServiceError::Invalid(
                    "override_reason is required for a rejected candidate".to_string(),
                ));
            }
            let target_key = cached.target.as_ref().map_or_else(
                || format!("manual:{}:{}", result.site_id, result.torrent_id),
                MediaTarget::target_key,
            );
            (
                request.quality_profile_id,
                request.downloader_id,
                target_key,
                format!(
                    "manual:{}:{}:{}",
                    result.site_id, result.torrent_id, request.downloader_id
                ),
                decision,
                None,
            )
        };

        self.ensure_references(quality_profile_id, downloader_id, &[result.site_id])
            .await?;
        let profile = self
            .db
            .get_quality_profile(quality_profile_id)
            .await?
            .ok_or_else(|| {
                MediaServiceError::NotFound(format!("quality profile {quality_profile_id}"))
            })?;
        let downloader = self
            .db
            .get_downloader(downloader_id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("downloader {downloader_id}")))?;
        let site = self
            .db
            .get_site(result.site_id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("site {}", result.site_id)))?;
        result.source_site = site.name;
        let decision_json = serde_json::json!({
            "decision": decision,
            "override_reason": override_reason,
        });
        let new_download = NewMediaDownload {
            subscription_id: request.subscription_id,
            target_key,
            dedupe_key,
            site_id: Some(result.site_id),
            downloader_id: Some(downloader_id),
            source_site: result.source_site.clone(),
            downloader_name: downloader.name.clone(),
            torrent_id: result.torrent_id.clone(),
            title: result.title.clone(),
            size: result.size,
            release_json: to_json(&result)?,
            decision_json: decision_json.to_string(),
            profile_snapshot_json: to_json(&profile)?,
        };
        if let Some(expected_version) = subscription_version {
            return self
                .db
                .enqueue_subscription_media_download(expected_version, &new_download)
                .await?
                .ok_or_else(|| {
                    MediaServiceError::Conflict(
                        "subscription target is no longer ready to queue".to_string(),
                    )
                });
        }
        let queued = self
            .db
            .enqueue_media_download(&new_download)
            .await
            .map_err(MediaServiceError::from)?;
        if queued.status == "submitted"
            && let Some(infohash) = queued.infohash.as_deref()
        {
            let client = self
                .downloaders
                .get(&downloader)
                .await
                .map_err(MediaServiceError::Downloader)?;
            let exists = !client
                .list_torrents_by_hashes(&[infohash.to_string()])
                .await
                .map_err(MediaServiceError::Downloader)?
                .is_empty();
            if !exists
                && self
                    .db
                    .requeue_missing_manual_media_download(queued.id, queued.version)
                    .await?
            {
                return self
                    .db
                    .get_media_download(queued.id)
                    .await?
                    .ok_or_else(|| MediaServiceError::NotFound(format!("download {}", queued.id)));
            }
        }
        Ok(queued)
    }

    pub async fn process_download(
        &self,
        download: MediaDownloadRecord,
        owner: &str,
    ) -> Result<(), MediaServiceError> {
        let download_id = download.id;
        let result = self.process_download_attempt(download, owner).await;
        if let Err(error) = &result {
            self.db
                .release_media_download_after_error(download_id, owner, &error.to_string(), false)
                .await?;
        }
        result
    }

    async fn process_download_attempt(
        &self,
        download: MediaDownloadRecord,
        owner: &str,
    ) -> Result<(), MediaServiceError> {
        if download.status == "reconciling" {
            return self.reconcile_download(download, owner).await;
        }
        if download.status != "fetching" {
            return Err(MediaServiceError::Conflict(format!(
                "download {} is in unexpected state {}",
                download.id, download.status
            )));
        }
        let result: SearchResult = serde_json::from_str(&download.release_json)
            .map_err(|error| MediaServiceError::Serialization(error.to_string()))?;
        let site_id = download
            .site_id
            .ok_or_else(|| MediaServiceError::NotFound("download site was deleted".to_string()))?;
        let downloader_id = download.downloader_id.ok_or_else(|| {
            MediaServiceError::NotFound("download client was deleted".to_string())
        })?;
        let site = self
            .db
            .get_site(site_id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("site {site_id}")))?;
        let downloader = self
            .db
            .get_downloader(downloader_id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("downloader {downloader_id}")))?;
        let proxy = self.db.get_settings().await?.proxy;
        let adapter = self.indexers.get_or_create(&site, proxy.as_deref()).await?;
        let torrent = match adapter.fetch_torrent(&result).await {
            Ok(torrent) => torrent,
            Err(error) => return Err(error.into()),
        };
        let infohash = torrent_infohash(&torrent)?;
        if let Some(existing) = self
            .db
            .get_media_download_by_infohash(downloader_id, &infohash)
            .await?
        {
            if existing.id != download.id && existing.status == "submitted" {
                if !self
                    .db
                    .mark_media_download_duplicate_submitted(
                        download.id,
                        download.version,
                        owner,
                        existing.id,
                    )
                    .await?
                {
                    return Err(MediaServiceError::Conflict(
                        "download lease changed while marking a duplicate".to_string(),
                    ));
                }
                return Ok(());
            }
        }
        if !self
            .db
            .mark_media_download_submitting(
                download.id,
                download.version,
                owner,
                &infohash,
                DOWNLOAD_LEASE_SECONDS,
            )
            .await?
        {
            return Err(MediaServiceError::Conflict(
                "download lease changed before qBittorrent submission".to_string(),
            ));
        }
        let submitting = self
            .db
            .get_media_download(download.id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("download {}", download.id)))?;
        let client = self
            .downloaders
            .get(&downloader)
            .await
            .map_err(MediaServiceError::Downloader)?;
        let subscription = match submitting.subscription_id {
            Some(subscription_id) => self.db.get_subscription(subscription_id).await?,
            None => None,
        };
        let save_path = subscription
            .as_ref()
            .and_then(|subscription| subscription.save_path.clone());
        let options = AddTorrentOptions {
            save_path,
            tags: Some("云母".to_string()),
            category: Some(
                media_download_category(
                    &submitting.target_key,
                    subscription
                        .as_ref()
                        .is_some_and(|subscription| subscription.tmdb_is_animation),
                )
                .to_string(),
            ),
            ..Default::default()
        };
        let filename = format!(
            "rflush-media-{}-{}.torrent",
            result.site_id, result.torrent_id
        );
        let submission = client.add_torrent(torrent, &filename, &options).await;
        let submission_error = submission.as_ref().err().cloned();
        let submitted: Result<bool, String> = match submission {
            Ok(()) => Ok(true),
            Err(_) => client
                .list_torrents_by_hashes(std::slice::from_ref(&infohash))
                .await
                .map(|torrents| !torrents.is_empty()),
        };
        match submitted {
            Ok(true) => {
                if !self
                    .db
                    .mark_media_download_submitted(
                        submitting.id,
                        submitting.version,
                        owner,
                        &infohash,
                    )
                    .await?
                {
                    return Err(MediaServiceError::Conflict(
                        "qBittorrent accepted the torrent but the outbox state changed".to_string(),
                    ));
                }
                Ok(())
            }
            Ok(false) => {
                let message = submission_error.unwrap_or_else(|| {
                    "qBittorrent did not contain the submitted torrent".to_string()
                });
                self.db
                    .release_media_download_after_error(submitting.id, owner, &message, true)
                    .await?;
                Err(MediaServiceError::Downloader(message))
            }
            Err(error) => {
                let next = (Utc::now()
                    + Duration::seconds(MEDIA_DOWNLOAD_RECONCILIATION_GRACE_SECONDS))
                .to_rfc3339();
                if !self
                    .db
                    .transition_media_download(
                        submitting.id,
                        submitting.version,
                        owner,
                        "submitting",
                        "reconciling",
                        Some(&infohash),
                        Some(&error),
                        Some(&next),
                    )
                    .await?
                {
                    return Err(MediaServiceError::Conflict(
                        "download lease changed while recording an unknown submission".to_string(),
                    ));
                }
                Err(MediaServiceError::Downloader(format!(
                    "submission result is unknown: {error}"
                )))
            }
        }
    }

    async fn reconcile_download(
        &self,
        download: MediaDownloadRecord,
        owner: &str,
    ) -> Result<(), MediaServiceError> {
        let infohash = download.infohash.clone().ok_or_else(|| {
            MediaServiceError::Invalid("reconciling download has no infohash".to_string())
        })?;
        let downloader_id = download.downloader_id.ok_or_else(|| {
            MediaServiceError::NotFound("download client was deleted".to_string())
        })?;
        let downloader = self
            .db
            .get_downloader(downloader_id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("downloader {downloader_id}")))?;
        let client = self
            .downloaders
            .get(&downloader)
            .await
            .map_err(MediaServiceError::Downloader)?;
        match client
            .list_torrents_by_hashes(std::slice::from_ref(&infohash))
            .await
        {
            Ok(torrents) if !torrents.is_empty() => {
                if !self
                    .db
                    .mark_media_download_submitted(download.id, download.version, owner, &infohash)
                    .await?
                {
                    return Err(MediaServiceError::Conflict(
                        "download lease changed during reconciliation".to_string(),
                    ));
                }
                Ok(())
            }
            Ok(_) => {
                self.db
                    .release_media_download_after_error(
                        download.id,
                        owner,
                        "torrent not found during reconciliation",
                        true,
                    )
                    .await?;
                Ok(())
            }
            Err(error) => Err(MediaServiceError::Downloader(error)),
        }
    }

    async fn resolve_indexers(
        &self,
        requested_site_ids: &[i64],
    ) -> Result<
        (
            Vec<Arc<dyn crate::indexer::IndexerAdapter>>,
            Vec<SiteSearchError>,
            HashMap<i64, u32>,
        ),
        MediaServiceError,
    > {
        let all_sites = self.db.list_sites().await?;
        let selected: Vec<_> =
            if requested_site_ids.is_empty() {
                all_sites
            } else {
                let mut by_id: HashMap<_, _> =
                    all_sites.into_iter().map(|site| (site.id, site)).collect();
                let mut seen = HashSet::new();
                let mut selected = Vec::with_capacity(requested_site_ids.len());
                for site_id in requested_site_ids {
                    if !seen.insert(*site_id) {
                        return Err(MediaServiceError::Invalid(
                            "selected PT sites must not contain duplicates".to_string(),
                        ));
                    }
                    selected.push(by_id.remove(site_id).ok_or_else(|| {
                        MediaServiceError::NotFound(format!("PT site {site_id}"))
                    })?);
                }
                selected
            };
        if selected.is_empty() {
            return Err(MediaServiceError::Invalid(
                "no PT sites are configured".to_string(),
            ));
        }
        let proxy = self.db.get_settings().await?.proxy;
        let priorities: HashMap<_, _> = selected
            .iter()
            .enumerate()
            .map(|(priority, site)| (site.id, priority as u32))
            .collect();
        let mut adapters = Vec::new();
        let mut errors = Vec::new();
        for site in selected {
            match self.indexers.get_or_create(&site, proxy.as_deref()).await {
                Ok(adapter) => adapters.push(adapter),
                Err(error) => errors.push(SiteSearchError {
                    site_id: site.id,
                    source_site: site.name,
                    query: String::new(),
                    code: error.code().to_string(),
                    message: error.to_string(),
                }),
            }
        }
        Ok((adapters, errors, priorities))
    }

    async fn ensure_references(
        &self,
        quality_profile_id: i64,
        downloader_id: i64,
        site_ids: &[i64],
    ) -> Result<(), MediaServiceError> {
        if self
            .db
            .get_quality_profile(quality_profile_id)
            .await?
            .is_none()
        {
            return Err(MediaServiceError::NotFound(format!(
                "quality profile {quality_profile_id}"
            )));
        }
        if self.db.get_downloader(downloader_id).await?.is_none() {
            return Err(MediaServiceError::NotFound(format!(
                "downloader {downloader_id}"
            )));
        }
        let configured: HashSet<_> = self
            .db
            .list_sites()
            .await?
            .into_iter()
            .map(|site| site.id)
            .collect();
        if site_ids.is_empty() || site_ids.iter().any(|id| !configured.contains(id)) {
            return Err(MediaServiceError::NotFound(
                "one or more selected PT sites do not exist".to_string(),
            ));
        }
        Ok(())
    }

    async fn tmdb_client(&self) -> Result<TmdbClient, MediaServiceError> {
        let media = self.db.get_media_settings().await?;
        let token = media.tmdb_token.ok_or(TmdbError::MissingToken)?;
        let proxy = self.db.get_settings().await?.proxy;
        let client = client_factory::build_client(proxy.as_deref())?;
        TmdbClient::new(client, token, media.tmdb_language).map_err(Into::into)
    }
}

fn readiness_status(readiness: &TargetReadiness) -> &'static str {
    match readiness {
        TargetReadiness::Due => "waiting",
        TargetReadiness::Future(_) => "waiting_air_date",
        TargetReadiness::AwaitingMetadata => "awaiting_metadata",
    }
}

fn target_needs_tmdb_refresh(
    target: Option<&SubscriptionTargetRecord>,
    last_status: Option<&str>,
    now: chrono::DateTime<Utc>,
) -> bool {
    let Some(target) = target else {
        return true;
    };
    match target.status.as_str() {
        "metadata_pending" => true,
        "pending" => match target.air_date.as_deref().and_then(air_date_eligible_at) {
            None => true,
            Some(eligible_at) => {
                matches!(last_status, Some("waiting_air_date" | "submitted")) && eligible_at <= now
            }
        },
        _ => false,
    }
}

fn subscription_absolute_anchor(
    subscription: &SubscriptionRecord,
) -> Result<Option<u32>, MediaServiceError> {
    let Some(current_absolute) = subscription.absolute_episode else {
        return Ok(None);
    };
    let start_episode = subscription.start_episode.unwrap_or(1);
    let current_episode = subscription.next_episode.ok_or_else(|| {
        MediaServiceError::Invalid(
            "absolute-numbered subscription has no episode cursor".to_string(),
        )
    })?;
    let offset = current_episode.checked_sub(start_episode).ok_or_else(|| {
        MediaServiceError::Invalid(
            "subscription episode cursor precedes its configured start".to_string(),
        )
    })?;
    current_absolute
        .checked_sub(offset)
        .map(Some)
        .ok_or_else(|| {
            MediaServiceError::Invalid(
                "subscription absolute episode cursor precedes its configured anchor".to_string(),
            )
        })
}

fn empty_subscription_run(subscription_id: i64, target_key: String) -> SubscriptionRunResult {
    SubscriptionRunResult {
        subscription_id,
        target_key,
        query_count: 0,
        candidate_count: 0,
        accepted_count: 0,
        download: None,
        site_errors: Vec::new(),
    }
}

fn empty_run_snapshot(started_at: &str, target_key: &str) -> SubscriptionRunSnapshot {
    SubscriptionRunSnapshot {
        started_at: started_at.to_string(),
        finished_at: Utc::now().to_rfc3339(),
        target_key: target_key.to_string(),
        queries: Vec::new(),
        candidates: Vec::new(),
        site_errors: Vec::new(),
        total_sites: 0,
        successful_sites: 0,
        best_candidate_id: None,
        error: None,
    }
}

fn failed_run_snapshot(
    started_at: &str,
    target_key: &str,
    error: String,
) -> SubscriptionRunSnapshot {
    SubscriptionRunSnapshot {
        error: Some(error),
        ..empty_run_snapshot(started_at, target_key)
    }
}

fn subscription_target(
    subscription: &SubscriptionRecord,
) -> Result<MediaTarget, MediaServiceError> {
    let mut titles = vec![subscription.title.clone()];
    if let Some(original) = &subscription.original_title {
        push_case_insensitive(&mut titles, original);
    }
    for alias in &subscription.aliases {
        push_case_insensitive(&mut titles, alias);
    }
    if subscription.media_type == "movie" {
        return Ok(MediaTarget::Movie {
            tmdb_id: subscription.tmdb_id,
            titles,
            year: subscription.year,
        });
    }
    if let Some(absolute_episode) = subscription.absolute_episode {
        return Ok(MediaTarget::Anime {
            tmdb_id: subscription.tmdb_id,
            titles,
            year: subscription.year,
            absolute_episode,
            season_episode: subscription
                .season
                .zip(subscription.next_episode)
                .map(|(season, episode)| SeasonEpisode { season, episode }),
        });
    }
    Ok(MediaTarget::Episode {
        tmdb_id: subscription.tmdb_id,
        titles,
        year: subscription.year,
        season: subscription.season.ok_or_else(|| {
            MediaServiceError::Invalid("TV subscription has no season".to_string())
        })?,
        episode: subscription.next_episode.ok_or_else(|| {
            MediaServiceError::Invalid("TV subscription has no next episode".to_string())
        })?,
        allow_season_pack: false,
    })
}

fn profile_to_domain(profile: &QualityProfileRecord) -> QualityProfile {
    QualityProfile {
        id: Some(profile.id),
        name: profile.name.clone(),
        resolution_order: profile.resolution_order.clone(),
        allowed_resolutions: profile.allowed_resolutions.clone(),
        blocked_resolutions: profile.blocked_resolutions.clone(),
        source_order: profile.source_order.clone(),
        allowed_sources: profile.allowed_sources.clone(),
        codec_order: profile.codec_order.clone(),
        blocked_codecs: profile.blocked_codecs.clone(),
        allow_unknown_quality: profile.allow_unknown_quality,
        minimum_score: profile.minimum_score.clamp(0, 100) as u32,
        min_seeders: profile.min_seeders,
    }
}

fn compare_candidates(left: &ResourceCandidate, right: &ResourceCandidate) -> std::cmp::Ordering {
    match (&left.sort_key, &right.sort_key) {
        (Some(left), Some(right)) => SortKey::compare_best_first(left, right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => right
            .result
            .seeders
            .cmp(&left.result.seeders)
            .then_with(|| right.result.publish_time.cmp(&left.result.publish_time))
            .then_with(|| left.result.site_id.cmp(&right.result.site_id))
            .then_with(|| left.result.torrent_id.cmp(&right.result.torrent_id)),
    }
}

fn reevaluate_resource_candidates(
    target: &MediaTarget,
    candidates: &mut [ResourceCandidate],
    profile: &QualityProfileRecord,
) {
    let domain_profile = profile_to_domain(profile);
    for candidate in candidates.iter_mut() {
        let Some(release) = candidate.release.as_ref() else {
            continue;
        };
        let decision =
            DecisionEngine::evaluate(target, release, &domain_profile, candidate.result.seeders);
        let site_priority = candidate
            .sort_key
            .as_ref()
            .map(|sort_key| sort_key.site_priority)
            .unwrap_or(u32::MAX);
        candidate.sort_key = Some(SortKey::from_decision(
            &decision,
            candidate.result.seeders,
            candidate.result.publish_time,
            site_priority,
            stable_release_key(
                &candidate.result.source_site,
                &candidate.result.torrent_id,
                &candidate.result.title,
            ),
        ));
        candidate.decision = Some(decision);
    }
    candidates.sort_by(compare_candidates);
}

fn push_case_insensitive(values: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty()
        && !values
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(value))
    {
        values.push(value.to_string());
    }
}

fn to_json<T: Serialize>(value: &T) -> Result<String, MediaServiceError> {
    serde_json::to_string(value)
        .map_err(|error| MediaServiceError::Serialization(error.to_string()))
}

fn new_candidate_id() -> Result<String, MediaServiceError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut random = [0_u8; 24];
    getrandom::fill(&mut random).map_err(|error| {
        MediaServiceError::Serialization(format!("failed to generate candidate id: {error}"))
    })?;
    let mut candidate_id = String::with_capacity(5 + random.len() * 2);
    candidate_id.push_str("cand_");
    for byte in random {
        candidate_id.push(HEX[(byte >> 4) as usize] as char);
        candidate_id.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(candidate_id)
}

fn default_true() -> bool {
    true
}

impl From<reqwest::Error> for MediaServiceError {
    fn from(error: reqwest::Error) -> Self {
        Self::Invalid(format!("failed to create HTTP client: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use crate::downloader::DownloaderClientPool;
    use crate::indexer::{IndexerAdapter, IndexerCapabilities, IndexerFuture};
    use tempfile::tempdir;

    use super::*;

    struct StaticIndexer {
        site_id: i64,
        site_name: String,
        results: Vec<SearchResult>,
    }

    impl IndexerAdapter for StaticIndexer {
        fn site_id(&self) -> i64 {
            self.site_id
        }

        fn site_name(&self) -> &str {
            &self.site_name
        }

        fn capabilities(&self) -> IndexerCapabilities {
            IndexerCapabilities {
                search: true,
                fetch_torrent: false,
                api_search: true,
                html_search: false,
            }
        }

        fn search<'a>(
            &'a self,
            _request: &'a SearchRequest,
        ) -> IndexerFuture<'a, Vec<SearchResult>> {
            Box::pin(async move { Ok(self.results.clone()) })
        }

        fn fetch_torrent<'a>(&'a self, _result: &'a SearchResult) -> IndexerFuture<'a, Vec<u8>> {
            Box::pin(async move {
                Err(IndexerError::Configuration(
                    "static test indexer cannot fetch torrents".to_string(),
                ))
            })
        }
    }

    async fn test_service() -> (
        tempfile::TempDir,
        Arc<MediaService>,
        Database,
        i64,
        i64,
        i64,
    ) {
        let dir = tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let first_site = db
            .create_site(
                "service-site-a",
                "nexusphp",
                "https://a.example",
                r#"{"auth_type":"cookie","cookie":"test=1"}"#,
                false,
            )
            .await
            .unwrap();
        let second_site = db
            .create_site(
                "service-site-b",
                "nexusphp",
                "https://b.example",
                r#"{"auth_type":"cookie","cookie":"test=1"}"#,
                false,
            )
            .await
            .unwrap();
        let downloader_id = db
            .create_downloader(
                "service-downloader",
                "qbittorrent",
                "http://127.0.0.1:8080",
                "",
                "",
            )
            .await
            .unwrap();
        let pool = DownloaderClientPool::new(db.clone());
        let service = MediaService::new(db.clone(), pool);
        (dir, service, db, first_site, second_site, downloader_id)
    }

    fn resource_search_result(index: usize) -> SearchResult {
        SearchResult {
            site_id: 1,
            source_site: "test-site".to_string(),
            torrent_id: format!("torrent-{index}"),
            title: format!("Example.Show.S01E01.1080p.WEB-DL-{index}"),
            detail_url: None,
            download_locator: Some(format!("torrent-{index}")),
            magnet: None,
            size: 1_024,
            seeders: u32::try_from(index).unwrap_or(u32::MAX),
            leechers: 0,
            publish_time: None,
        }
    }

    fn resource_search_request(site_id: i64) -> ResourceSearchRequest {
        ResourceSearchRequest {
            query: Some("Example Show".to_string()),
            site_ids: vec![site_id],
            target: None,
            quality_profile_id: None,
            page_size: Some(100),
        }
    }

    async fn install_static_indexer(
        service: &MediaService,
        db: &Database,
        site_id: i64,
        results: Vec<SearchResult>,
    ) {
        let site = db.get_site(site_id).await.unwrap().unwrap();
        service
            .indexers
            .insert_for_test(
                &site,
                None,
                Arc::new(StaticIndexer {
                    site_id,
                    site_name: site.name.clone(),
                    results,
                }),
            )
            .await;
    }

    #[test]
    fn persistent_profile_maps_to_domain_policy() {
        let record = QualityProfileRecord {
            id: 9,
            name: "Test".to_string(),
            resolution_order: vec!["1080p".to_string()],
            allowed_resolutions: vec!["1080p".to_string()],
            blocked_resolutions: vec!["480p".to_string()],
            source_order: vec!["web-dl".to_string()],
            allowed_sources: Vec::new(),
            codec_order: vec!["h265".to_string()],
            blocked_codecs: Vec::new(),
            allow_unknown_quality: false,
            minimum_score: 80,
            min_seeders: 2,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let domain = profile_to_domain(&record);
        assert_eq!(domain.id, Some(9));
        assert_eq!(domain.minimum_score, 80);
        assert_eq!(domain.min_seeders, 2);
    }

    #[test]
    fn translated_alias_turns_title_only_rejection_into_an_accepted_candidate() {
        let profile = QualityProfileRecord {
            id: 1,
            name: "Test".to_string(),
            resolution_order: vec!["1080p".to_string()],
            allowed_resolutions: vec!["1080p".to_string()],
            blocked_resolutions: Vec::new(),
            source_order: vec!["web-dl".to_string()],
            allowed_sources: vec!["web-dl".to_string()],
            codec_order: vec!["h264".to_string()],
            blocked_codecs: Vec::new(),
            allow_unknown_quality: false,
            minimum_score: 80,
            min_seeders: 1,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let release = ReleaseParser::default()
            .parse("Crowned in a Hundred Days 2026 S01E06 1080p WEB-DL H.264 AAC-UBWEB")
            .unwrap();
        let original_target = MediaTarget::Episode {
            tmdb_id: 326844,
            titles: vec!["百日成王".to_string(), "Bai Ri Cheng Wang".to_string()],
            year: Some(2026),
            season: 1,
            episode: 6,
            allow_season_pack: false,
        };
        let original_decision =
            DecisionEngine::evaluate(&original_target, &release, &profile_to_domain(&profile), 31);
        assert_eq!(original_decision.rejections.len(), 1);
        assert_eq!(original_decision.rejections[0].code, RejectCode::WrongTitle);

        let translated_target = MediaTarget::Episode {
            tmdb_id: 326844,
            titles: vec![
                "百日成王".to_string(),
                "Bai Ri Cheng Wang".to_string(),
                "Crowned in a Hundred Days".to_string(),
            ],
            year: Some(2026),
            season: 1,
            episode: 6,
            allow_season_pack: false,
        };
        let mut candidates = vec![ResourceCandidate {
            candidate_id: "candidate".to_string(),
            result: SearchResult {
                site_id: 1,
                source_site: "site".to_string(),
                torrent_id: "1207293".to_string(),
                title: release.raw_title.clone(),
                detail_url: None,
                download_locator: Some("1207293".to_string()),
                magnet: None,
                size: 146_374_137,
                seeders: 31,
                leechers: 0,
                publish_time: None,
            },
            release: Some(release),
            parse_error: None,
            decision: Some(original_decision),
            sort_key: None,
        }];

        reevaluate_resource_candidates(&translated_target, &mut candidates, &profile);

        assert!(candidates[0].decision.as_ref().unwrap().accepted);
        assert_eq!(candidates[0].decision.as_ref().unwrap().score, 100);
    }

    #[test]
    fn subscription_target_uses_absolute_episode_when_configured() {
        let mut subscription = SubscriptionRecord {
            id: 1,
            tmdb_id: 2,
            media_type: "tv".to_string(),
            tmdb_is_animation: true,
            title: "Anime".to_string(),
            original_title: None,
            aliases: Vec::new(),
            year: None,
            poster_path: None,
            season: Some(1),
            next_episode: Some(3),
            start_episode: Some(3),
            absolute_episode: Some(123),
            quality_profile_id: 1,
            downloader_id: 1,
            site_ids: vec![1],
            save_path: None,
            enabled: true,
            next_run_at: String::new(),
            lease_owner: None,
            lease_until: None,
            version: 0,
            last_status: None,
            last_error: None,
            last_run_at: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let target = subscription_target(&subscription).unwrap();
        assert_eq!(target.target_key(), "tv:2:abs0123");

        subscription.next_episode = Some(5);
        subscription.absolute_episode = Some(125);
        assert_eq!(
            subscription_absolute_anchor(&subscription).unwrap(),
            Some(123)
        );
    }

    #[test]
    fn tmdb_refresh_is_required_only_at_metadata_or_air_date_checkpoints() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-15T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut target = SubscriptionTargetRecord {
            id: 1,
            subscription_id: 1,
            target_key: "tv:2:s01e03".to_string(),
            season: Some(1),
            episode: Some(3),
            absolute_episode: None,
            air_date: None,
            status: "metadata_pending".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(target_needs_tmdb_refresh(Some(&target), None, now));

        target.status = "pending".to_string();
        target.air_date = Some("2026-07-20".to_string());
        assert!(!target_needs_tmdb_refresh(
            Some(&target),
            Some("waiting_air_date"),
            now,
        ));

        target.air_date = Some("2026-07-15".to_string());
        assert!(target_needs_tmdb_refresh(
            Some(&target),
            Some("waiting_air_date"),
            now,
        ));
        assert!(target_needs_tmdb_refresh(
            Some(&target),
            Some("submitted"),
            now,
        ));

        target.status = "queued".to_string();
        assert!(!target_needs_tmdb_refresh(Some(&target), None, now));
    }

    #[tokio::test]
    async fn subscription_queue_uses_the_cached_title_instead_of_forged_client_fields() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let subscription = db
            .create_subscription(&NewSubscription {
                tmdb_id: 42,
                media_type: "tv".to_string(),
                tmdb_is_animation: false,
                title: "Example Show".to_string(),
                original_title: None,
                aliases: Vec::new(),
                year: Some(2026),
                poster_path: None,
                season: Some(1),
                start_episode: Some(3),
                absolute_episode: None,
                quality_profile_id: 1,
                downloader_id,
                site_ids: vec![site_id],
                save_path: None,
                enabled: true,
            })
            .await
            .unwrap();
        let target = subscription_target(&subscription).unwrap();
        let candidate_id = new_candidate_id().unwrap();
        service.candidate_cache.lock().await.insert(
            candidate_id.clone(),
            SearchResult {
                site_id,
                source_site: "service-site-a".to_string(),
                torrent_id: "authoritative-unrelated-3".to_string(),
                title: "Other.Show.S01E03.1080p.WEB-DL".to_string(),
                detail_url: None,
                download_locator: Some("authoritative-unrelated-3".to_string()),
                magnet: None,
                size: 1024,
                seeders: 100,
                leechers: 0,
                publish_time: None,
            },
            Some(target),
            Some(1),
        );
        let request = QueueDownloadRequest {
            candidate_id: candidate_id.clone(),
            quality_profile_id: 1,
            downloader_id,
            subscription_id: Some(subscription.id),
            override_reason: None,
        };

        let error = service.queue_download(&request).await.unwrap_err();
        assert!(matches!(error, MediaServiceError::Invalid(_)));
        assert!(error.to_string().contains("override_reason is required"));
        assert!(
            db.list_media_downloads(None, None, 100, 0)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            db.get_subscription(subscription.id)
                .await
                .unwrap()
                .unwrap()
                .next_episode,
            Some(3)
        );

        let forged = serde_json::json!({
            "candidate_id": candidate_id,
            "quality_profile_id": 1,
            "downloader_id": downloader_id,
            "subscription_id": subscription.id,
            "result": {
                "site_id": site_id,
                "torrent_id": "forged-torrent",
                "title": "Example.Show.S01E03.1080p.WEB-DL"
            }
        });
        assert!(serde_json::from_value::<QueueDownloadRequest>(forged).is_err());
    }

    #[tokio::test]
    async fn subscription_queue_rejects_candidates_from_another_target_or_indexer() {
        let (_dir, service, db, site_id, other_site, downloader_id) = test_service().await;
        let subscription = db
            .create_subscription(&NewSubscription {
                tmdb_id: 42,
                media_type: "tv".to_string(),
                tmdb_is_animation: false,
                title: "Example Show".to_string(),
                original_title: None,
                aliases: Vec::new(),
                year: Some(2026),
                poster_path: None,
                season: Some(1),
                start_episode: Some(3),
                absolute_episode: None,
                quality_profile_id: 1,
                downloader_id,
                site_ids: vec![site_id],
                save_path: None,
                enabled: true,
            })
            .await
            .unwrap();
        let other_target = MediaTarget::Episode {
            tmdb_id: 42,
            titles: vec!["Example Show".to_string()],
            year: Some(2026),
            season: 1,
            episode: 4,
            allow_season_pack: false,
        };
        let candidate_id = new_candidate_id().unwrap();
        service.candidate_cache.lock().await.insert(
            candidate_id.clone(),
            SearchResult {
                site_id: other_site,
                source_site: "service-site-b".to_string(),
                torrent_id: "other-context".to_string(),
                title: "Example.Show.S01E03.1080p.WEB-DL".to_string(),
                detail_url: None,
                download_locator: Some("other-context".to_string()),
                magnet: None,
                size: 1024,
                seeders: 100,
                leechers: 0,
                publish_time: None,
            },
            Some(other_target),
            Some(1),
        );

        let error = service
            .queue_download(&QueueDownloadRequest {
                candidate_id,
                quality_profile_id: 1,
                downloader_id,
                subscription_id: Some(subscription.id),
                override_reason: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(error, MediaServiceError::Invalid(_)));
        assert!(
            error.to_string().contains("site is not configured")
                || error.to_string().contains("current target")
        );
    }

    #[tokio::test]
    async fn manual_queue_persists_only_the_cached_search_result() {
        let (_dir, service, _db, site_id, _other_site, downloader_id) = test_service().await;
        let candidate_id = new_candidate_id().unwrap();
        service.candidate_cache.lock().await.insert(
            candidate_id.clone(),
            SearchResult {
                site_id,
                source_site: "stale-client-name".to_string(),
                torrent_id: "authoritative-torrent".to_string(),
                title: "Authoritative.Release.1080p.WEB-DL".to_string(),
                detail_url: None,
                download_locator: Some("authoritative-torrent".to_string()),
                magnet: None,
                size: 2048,
                seeders: 20,
                leechers: 1,
                publish_time: None,
            },
            None,
            None,
        );

        let queued = service
            .queue_download(&QueueDownloadRequest {
                candidate_id,
                quality_profile_id: 1,
                downloader_id,
                subscription_id: None,
                override_reason: None,
            })
            .await
            .unwrap();
        assert_eq!(queued.torrent_id, "authoritative-torrent");
        assert_eq!(queued.title, "Authoritative.Release.1080p.WEB-DL");
        assert_eq!(queued.source_site, "service-site-a");
        let persisted: SearchResult = serde_json::from_str(&queued.release_json).unwrap();
        assert_eq!(persisted.torrent_id, "authoritative-torrent");
        assert_eq!(persisted.seeders, 20);
    }

    #[test]
    fn candidate_ids_are_random_opaque_tokens() {
        let first = new_candidate_id().unwrap();
        let second = new_candidate_id().unwrap();
        assert_ne!(first, second);
        assert_eq!(first.len(), 53);
        assert!(
            first
                .strip_prefix("cand_")
                .unwrap()
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
    }

    #[test]
    fn candidate_cache_discards_expired_and_oldest_entries() {
        let result = SearchResult {
            site_id: 1,
            source_site: "site".to_string(),
            torrent_id: "torrent".to_string(),
            title: "Title".to_string(),
            detail_url: None,
            download_locator: Some("torrent".to_string()),
            magnet: None,
            size: 1,
            seeders: 1,
            leechers: 0,
            publish_time: None,
        };
        let mut cache = CandidateCache::default();
        cache.insert("expired".to_string(), result.clone(), None, None);
        cache.entries.get_mut("expired").unwrap().expires_at = Instant::now();
        assert!(cache.get("expired").is_none());

        for index in 0..=CANDIDATE_CACHE_CAPACITY {
            cache.insert(format!("candidate-{index}"), result.clone(), None, None);
        }
        assert_eq!(cache.entries.len(), CANDIDATE_CACHE_CAPACITY);
        assert!(cache.get("candidate-0").is_none());
        assert!(
            cache
                .get(&format!("candidate-{CANDIDATE_CACHE_CAPACITY}"))
                .is_some()
        );
    }

    #[tokio::test]
    async fn oversized_public_search_returns_only_immediately_resolvable_candidate_ids() {
        let (_dir, service, db, first_site, _second_site, _downloader_id) = test_service().await;
        let results = (0..RESOURCE_SEARCH_RESULT_LIMIT + 37)
            .map(resource_search_result)
            .collect();
        install_static_indexer(&service, &db, first_site, results).await;

        let response = service
            .search_resources(&resource_search_request(first_site))
            .await
            .unwrap();

        assert_eq!(response.candidates.len(), RESOURCE_SEARCH_RESULT_LIMIT);
        let mut ids = HashSet::new();
        let mut cache = service.candidate_cache.lock().await;
        for candidate in response.candidates {
            assert!(ids.insert(candidate.candidate_id.clone()));
            let cached = cache
                .get(&candidate.candidate_id)
                .expect("every returned candidate must still be cached");
            assert_eq!(cached.result.torrent_id, candidate.result.torrent_id);
        }
    }

    #[tokio::test]
    async fn subscription_heartbeat_search_does_not_evict_manual_candidates() {
        let (_dir, service, db, first_site, _second_site, downloader_id) = test_service().await;
        let subscription = db
            .create_subscription(&NewSubscription {
                tmdb_id: 42,
                media_type: "movie".to_string(),
                tmdb_is_animation: false,
                title: "Example Show".to_string(),
                original_title: None,
                aliases: Vec::new(),
                year: Some(2026),
                poster_path: None,
                season: None,
                start_episode: None,
                absolute_episode: None,
                quality_profile_id: 1,
                downloader_id,
                site_ids: vec![first_site],
                save_path: None,
                enabled: true,
            })
            .await
            .unwrap();
        let manual_id = new_candidate_id().unwrap();
        {
            let mut cache = service.candidate_cache.lock().await;
            cache.insert(manual_id.clone(), resource_search_result(0), None, None);
            for index in 1..CANDIDATE_CACHE_CAPACITY {
                cache.insert(
                    format!("existing-{index}"),
                    resource_search_result(index),
                    None,
                    None,
                );
            }
            assert_eq!(cache.entries.len(), CANDIDATE_CACHE_CAPACITY);
        }

        let results = (0..RESOURCE_SEARCH_RESULT_LIMIT + 1)
            .map(|index| resource_search_result(index + CANDIDATE_CACHE_CAPACITY))
            .collect();
        install_static_indexer(&service, &db, first_site, results).await;
        let (automatic_search, _version) = service
            .search_resources_with_subscription_heartbeat(
                &subscription,
                "test-owner",
                30,
                resource_search_request(first_site),
            )
            .await
            .unwrap();
        let automatic_candidates = automatic_search.unwrap().candidates;

        assert_eq!(automatic_candidates.len(), RESOURCE_SEARCH_RESULT_LIMIT);
        let mut cache = service.candidate_cache.lock().await;
        assert_eq!(cache.entries.len(), CANDIDATE_CACHE_CAPACITY);
        assert!(cache.get(&manual_id).is_some());
        assert!(
            automatic_candidates
                .iter()
                .all(|candidate| cache.get(&candidate.candidate_id).is_none())
        );
    }

    #[tokio::test]
    async fn subscription_scan_persists_queries_and_candidate_decisions() {
        let (_dir, service, db, first_site, _second_site, downloader_id) = test_service().await;
        let subscription = db
            .create_subscription(&NewSubscription {
                tmdb_id: 42,
                media_type: "movie".to_string(),
                tmdb_is_animation: false,
                title: "Example Show".to_string(),
                original_title: None,
                aliases: Vec::new(),
                year: Some(2026),
                poster_path: None,
                season: None,
                start_episode: None,
                absolute_episode: None,
                quality_profile_id: 1,
                downloader_id,
                site_ids: vec![first_site],
                save_path: None,
                enabled: true,
            })
            .await
            .unwrap();
        install_static_indexer(&service, &db, first_site, vec![resource_search_result(1)]).await;

        let run = service.run_subscription(subscription.id).await.unwrap();
        let snapshot = service
            .get_subscription_last_run(subscription.id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(snapshot.target_key, run.target_key);
        assert_eq!(snapshot.queries.len(), run.query_count);
        assert_eq!(snapshot.candidates.len(), run.candidate_count);
        assert_eq!(snapshot.candidates.len(), 1);
        assert!(snapshot.candidates[0].decision.is_some());
        assert!(snapshot.error.is_none());
    }

    #[tokio::test]
    async fn requested_indexer_order_defines_site_priority() {
        let (_dir, service, _db, first_site, second_site, _downloader_id) = test_service().await;
        let (adapters, errors, priorities) = service
            .resolve_indexers(&[second_site, first_site])
            .await
            .unwrap();
        assert_eq!(adapters.len(), 2);
        assert!(errors.is_empty());
        assert_eq!(priorities.get(&second_site), Some(&0));
        assert_eq!(priorities.get(&first_site), Some(&1));

        let duplicate = match service.resolve_indexers(&[first_site, first_site]).await {
            Ok(_) => panic!("duplicate site ids must be rejected"),
            Err(error) => error,
        };
        assert!(matches!(duplicate, MediaServiceError::Invalid(_)));
    }
}
