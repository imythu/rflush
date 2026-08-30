use std::time::Duration;

use chrono::Utc;
use futures::future::join_all;
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
}

impl SiteStatsRefresher {
    pub fn new(db: Database) -> Self {
        Self { db }
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
