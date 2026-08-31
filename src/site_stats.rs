use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::future::join_all;
use tokio::sync::Mutex;
use tracing::{error, info};

use crate::db::Database;
use crate::error::AppError;
use crate::net::client_factory;
use crate::site::SiteWithStats;
use crate::site::factory as site_factory;

const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone)]
pub struct SiteStatsRefresher {
    db: Database,
    refresh_lock: Arc<Mutex<()>>,
}

impl SiteStatsRefresher {
    pub fn new(db: Database) -> Self {
        Self {
            db,
            refresh_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn start(&self) {
        info!("site stats refresher started (interval: 1h)");
        if let Err(error) = self.refresh_all().await {
            error!("initial site stats refresh failed: {}", error);
        }

        let mut interval = tokio::time::interval(REFRESH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(error) = self.refresh_all().await {
                error!("site stats refresh failed: {}", error);
            }
        }
    }

    pub async fn refresh_all(&self) -> Result<Vec<SiteWithStats>, AppError> {
        let _refresh_guard = self.refresh_lock.lock().await;
        self.refresh_all_unlocked().await
    }

    pub fn refresh_all_in_background(self: &Arc<Self>) -> bool {
        let Ok(refresh_guard) = Arc::clone(&self.refresh_lock).try_lock_owned() else {
            return false;
        };
        let refresher = Arc::clone(self);
        tokio::spawn(async move {
            let _refresh_guard = refresh_guard;
            match refresher.refresh_all_unlocked().await {
                Ok(sites) => info!(
                    site_count = sites.len(),
                    "manual site stats refresh completed"
                ),
                Err(error) => error!("manual site stats refresh failed: {}", error),
            }
        });
        true
    }

    pub fn is_refreshing(&self) -> bool {
        self.refresh_lock.try_lock().is_err()
    }

    async fn refresh_all_unlocked(&self) -> Result<Vec<SiteWithStats>, AppError> {
        let sites = self.db.list_sites().await?;
        let settings = self.db.get_settings().await?;
        let proxy = settings.proxy.as_deref();
        let db = self.db.clone();
        join_all(sites.into_iter().map(|site| {
            let db = db.clone();
            async move {
                let checked_at = Utc::now().to_rfc3339();
                let result = match client_factory::resolve_site_client(proxy, site.use_proxy) {
                    Ok(client) => match site_factory::create_adapter(&site, client) {
                        Ok(adapter) => adapter.get_user_stats().await,
                        Err(error) => Err(error),
                    },
                    Err(error) => Err(format!("创建 HTTP 客户端失败: {}", error)),
                };

                match result {
                    Ok(stats) => {
                        db.upsert_site_stats_success(site.id, &stats, &checked_at)
                            .await
                    }
                    Err(error) => {
                        db.upsert_site_stats_error(site.id, &error, &checked_at)
                            .await
                    }
                }
            }
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

        self.db.list_sites_with_stats().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn background_refresh_is_deduplicated_while_another_refresh_is_running() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(temp.path()).await.unwrap();
        let refresher = Arc::new(SiteStatsRefresher::new(db));
        let refresh_guard = Arc::clone(&refresher.refresh_lock).lock_owned().await;

        assert!(refresher.is_refreshing());
        assert!(!refresher.refresh_all_in_background());

        drop(refresh_guard);
        assert!(!refresher.is_refreshing());
        assert!(refresher.refresh_all_in_background());

        tokio::time::timeout(Duration::from_secs(1), async {
            while refresher.is_refreshing() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
