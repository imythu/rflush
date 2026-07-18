use std::collections::{HashMap, HashSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use reqwest::redirect::Policy;
use reqwest::{Client, Proxy};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::site::SiteRecord;

use super::{
    IndexerAdapter, IndexerError, SearchRequest, SearchResult, create_indexer, normalize_base_url,
    same_origin,
};

const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";

struct CachedIndexer {
    config_digest: u64,
    adapter: Arc<dyn IndexerAdapter>,
}

/// Reuses HTTP clients and adapters until any site, credential or effective proxy setting changes.
pub struct IndexerPool {
    cache: Mutex<HashMap<i64, CachedIndexer>>,
}

#[allow(dead_code)]
impl IndexerPool {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            cache: Mutex::new(HashMap::new()),
        })
    }

    pub async fn get_or_create(
        &self,
        record: &SiteRecord,
        global_proxy: Option<&str>,
    ) -> Result<Arc<dyn IndexerAdapter>, IndexerError> {
        let effective_proxy = record
            .use_proxy
            .then_some(global_proxy)
            .flatten()
            .map(str::trim)
            .filter(|proxy| !proxy.is_empty());
        let digest = config_digest(record, effective_proxy);
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.get(&record.id)
            && cached.config_digest == digest
        {
            return Ok(Arc::clone(&cached.adapter));
        }

        let client = build_indexer_client(record, effective_proxy)?;
        let adapter = create_indexer(record, client)?;
        cache.insert(
            record.id,
            CachedIndexer {
                config_digest: digest,
                adapter: Arc::clone(&adapter),
            },
        );
        Ok(adapter)
    }

    pub async fn invalidate(&self, site_id: i64) {
        self.cache.lock().await.remove(&site_id);
    }

    pub async fn clear(&self) {
        self.cache.lock().await.clear();
    }

    pub async fn len(&self) -> usize {
        self.cache.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    #[cfg(test)]
    pub(crate) async fn insert_for_test(
        &self,
        record: &SiteRecord,
        global_proxy: Option<&str>,
        adapter: Arc<dyn IndexerAdapter>,
    ) {
        let effective_proxy = record
            .use_proxy
            .then_some(global_proxy)
            .flatten()
            .map(str::trim)
            .filter(|proxy| !proxy.is_empty());
        self.cache.lock().await.insert(
            record.id,
            CachedIndexer {
                config_digest: config_digest(record, effective_proxy),
                adapter,
            },
        );
    }
}

fn build_indexer_client(
    record: &SiteRecord,
    effective_proxy: Option<&str>,
) -> Result<Client, IndexerError> {
    let base_url = normalize_base_url(&record.base_url)?;
    let redirect_origin = base_url.clone();
    let redirect_policy = Policy::custom(move |attempt| {
        if attempt.previous().len() < 5 && same_origin(&redirect_origin, attempt.url()) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    });
    let mut builder = Client::builder()
        .user_agent(BROWSER_USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(redirect_policy);
    if let Some(proxy) = effective_proxy {
        builder = builder.proxy(Proxy::all(proxy).map_err(|error| {
            IndexerError::Configuration(format!("invalid indexer proxy URL: {error}"))
        })?);
    }
    builder.build().map_err(|error| {
        IndexerError::Configuration(format!("failed to build indexer HTTP client: {error}"))
    })
}

fn config_digest(record: &SiteRecord, effective_proxy: Option<&str>) -> u64 {
    let mut hasher = DefaultHasher::new();
    record.id.hash(&mut hasher);
    record.name.hash(&mut hasher);
    record.site_type.hash(&mut hasher);
    record.base_url.hash(&mut hasher);
    record.auth_config.hash(&mut hasher);
    record.use_proxy.hash(&mut hasher);
    effective_proxy.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteSearchError {
    pub site_id: i64,
    pub source_site: String,
    pub query: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateSearchResult {
    pub results: Vec<SearchResult>,
    pub errors: Vec<SiteSearchError>,
    pub total_sites: usize,
    pub total_queries: usize,
    pub successful_sites: usize,
    pub successful_requests: usize,
}

pub struct IndexerAggregator {
    max_concurrency: usize,
}

impl IndexerAggregator {
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            max_concurrency: max_concurrency.max(1),
        }
    }

    /// Search every site/query pair concurrently. A failed pair is recorded and never cancels
    /// successful work from another query or site.
    pub async fn search(
        &self,
        indexers: &[Arc<dyn IndexerAdapter>],
        requests: &[SearchRequest],
    ) -> AggregateSearchResult {
        let requests = unique_requests(requests);
        let total_sites = indexers.len();
        let total_queries = requests.len();
        let jobs: Vec<(Arc<dyn IndexerAdapter>, SearchRequest)> = indexers
            .iter()
            .flat_map(|adapter| {
                requests
                    .iter()
                    .cloned()
                    .map(|request| (Arc::clone(adapter), request))
            })
            .collect();

        let mut jobs = jobs.into_iter();
        let mut pending = FuturesUnordered::new();
        for _ in 0..self.max_concurrency {
            if let Some((adapter, request)) = jobs.next() {
                pending.push(run_search_job(adapter, request));
            }
        }
        let mut completed = Vec::new();
        while let Some(result) = pending.next().await {
            completed.push(result);
            if let Some((adapter, request)) = jobs.next() {
                pending.push(run_search_job(adapter, request));
            }
        }

        let mut deduplicated = HashMap::<String, SearchResult>::new();
        let mut errors = Vec::new();
        let mut successful_requests = 0;
        let mut successful_sites = HashSet::new();
        for (site_id, source_site, query, outcome) in completed {
            match outcome {
                Ok(results) => {
                    successful_requests += 1;
                    successful_sites.insert(site_id);
                    for result in results {
                        let key = dedupe_key(&result);
                        match deduplicated.get_mut(&key) {
                            Some(current) if result_is_preferred(&result, current) => {
                                *current = result;
                            }
                            None => {
                                deduplicated.insert(key, result);
                            }
                            _ => {}
                        }
                    }
                }
                Err(error) => errors.push(SiteSearchError {
                    site_id,
                    source_site,
                    query,
                    code: error.code().to_string(),
                    message: error.to_string(),
                }),
            }
        }

        let mut results: Vec<_> = deduplicated.into_values().collect();
        results.sort_by(|left, right| {
            right
                .publish_time
                .cmp(&left.publish_time)
                .then_with(|| right.seeders.cmp(&left.seeders))
                .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
                .then_with(|| left.site_id.cmp(&right.site_id))
                .then_with(|| left.torrent_id.cmp(&right.torrent_id))
        });
        errors.sort_by(|left, right| {
            left.site_id
                .cmp(&right.site_id)
                .then_with(|| left.query.cmp(&right.query))
        });

        AggregateSearchResult {
            results,
            errors,
            total_sites,
            total_queries,
            successful_sites: successful_sites.len(),
            successful_requests,
        }
    }
}

async fn run_search_job(
    adapter: Arc<dyn IndexerAdapter + 'static>,
    request: SearchRequest,
) -> (i64, String, String, Result<Vec<SearchResult>, IndexerError>) {
    let site_id = adapter.site_id();
    let site_name = adapter.site_name().to_string();
    let query = request.query.clone();
    let result = adapter.search(&request).await;
    (site_id, site_name, query, result)
}

impl Default for IndexerAggregator {
    fn default() -> Self {
        Self::new(4)
    }
}

fn unique_requests(requests: &[SearchRequest]) -> Vec<SearchRequest> {
    let mut seen = HashSet::new();
    requests
        .iter()
        .filter(|request| {
            seen.insert((
                request.query.trim().to_lowercase(),
                request.page,
                request.page_size,
            ))
        })
        .cloned()
        .collect()
}

fn dedupe_key(result: &SearchResult) -> String {
    if let Some(info_hash) = result.magnet.as_deref().and_then(magnet_info_hash) {
        return format!("hash:{info_hash}");
    }

    if result.size > 0 {
        let normalized_title: String = result
            .title
            .chars()
            .flat_map(char::to_lowercase)
            .filter(|character| character.is_alphanumeric())
            .collect();
        if !normalized_title.is_empty() {
            return format!("release:{normalized_title}:{}", result.size);
        }
    }
    format!("site:{}:{}", result.site_id, result.torrent_id)
}

fn magnet_info_hash(magnet: &str) -> Option<String> {
    let url = reqwest::Url::parse(magnet).ok()?;
    url.query_pairs().find_map(|(key, value)| {
        if key != "xt" {
            return None;
        }
        value
            .to_ascii_lowercase()
            .strip_prefix("urn:btih:")
            .filter(|hash| !hash.is_empty())
            .map(str::to_string)
    })
}

fn result_is_preferred(candidate: &SearchResult, current: &SearchResult) -> bool {
    candidate.seeders > current.seeders
        || (candidate.seeders == current.seeders
            && (candidate.publish_time > current.publish_time
                || (candidate.publish_time == current.publish_time
                    && (candidate.site_id, &candidate.torrent_id)
                        < (current.site_id, &current.torrent_id))))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{IndexerAggregator, config_digest};
    use crate::indexer::{
        IndexerAdapter, IndexerCapabilities, IndexerError, IndexerFuture, SearchRequest,
        SearchResult,
    };
    use crate::site::SiteRecord;

    struct FakeIndexer {
        site_id: i64,
        site_name: String,
        seeders: u32,
        fail_query: Option<String>,
    }

    impl IndexerAdapter for FakeIndexer {
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
            request: &'a SearchRequest,
        ) -> IndexerFuture<'a, Vec<SearchResult>> {
            Box::pin(async move {
                if self.fail_query.as_deref() == Some(request.query.as_str()) {
                    return Err(IndexerError::Http("isolated failure".to_string()));
                }
                Ok(vec![SearchResult {
                    site_id: self.site_id,
                    source_site: self.site_name.clone(),
                    torrent_id: format!("{}-{}", self.site_id, request.query),
                    title: "Same.Release.S01E01".to_string(),
                    detail_url: None,
                    download_locator: Some("id".to_string()),
                    magnet: None,
                    size: 1024,
                    seeders: self.seeders,
                    leechers: 0,
                    publish_time: None,
                }])
            })
        }

        fn fetch_torrent<'a>(&'a self, _result: &'a SearchResult) -> IndexerFuture<'a, Vec<u8>> {
            Box::pin(async move {
                Err(IndexerError::Configuration(
                    "fake fetch is unsupported".to_string(),
                ))
            })
        }
    }

    #[tokio::test]
    async fn aggregates_concurrently_deduplicates_and_isolates_errors() {
        let indexers: Vec<Arc<dyn IndexerAdapter>> = vec![
            Arc::new(FakeIndexer {
                site_id: 1,
                site_name: "One".to_string(),
                seeders: 3,
                fail_query: None,
            }),
            Arc::new(FakeIndexer {
                site_id: 2,
                site_name: "Two".to_string(),
                seeders: 9,
                fail_query: Some("second".to_string()),
            }),
        ];
        let requests = vec![SearchRequest::new("first"), SearchRequest::new("second")];
        let aggregate = IndexerAggregator::new(2).search(&indexers, &requests).await;

        assert_eq!(aggregate.total_sites, 2);
        assert_eq!(aggregate.total_queries, 2);
        assert_eq!(aggregate.successful_requests, 3);
        assert_eq!(aggregate.errors.len(), 1);
        assert_eq!(aggregate.errors[0].site_id, 2);
        assert_eq!(aggregate.results.len(), 1);
        assert_eq!(aggregate.results[0].site_id, 2);
        assert_eq!(aggregate.results[0].seeders, 9);
    }

    #[test]
    fn configuration_digest_changes_with_credentials_and_proxy() {
        let mut record = SiteRecord {
            id: 1,
            name: "Site".to_string(),
            site_type: "nexusphp".to_string(),
            base_url: "https://tracker.example".to_string(),
            auth_config: r#"{"auth_type":"api_key","api_key":"one"}"#.to_string(),
            use_proxy: true,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let initial = config_digest(&record, Some("http://proxy-one:8080"));
        record.auth_config = r#"{"auth_type":"api_key","api_key":"two"}"#.to_string();
        assert_ne!(
            initial,
            config_digest(&record, Some("http://proxy-one:8080"))
        );
        assert_ne!(
            initial,
            config_digest(&record, Some("http://proxy-two:8080"))
        );
    }
}
