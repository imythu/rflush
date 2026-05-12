use std::time::Duration;

use chrono::Utc;
use futures::future::join_all;
use tracing::{error, info};

use crate::db::Database;
use crate::error::AppError;
use crate::site::factory as site_factory;
use crate::site::SiteWithStats;

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
        let db = self.db.clone();
        join_all(sites.into_iter().map(|site| {
            let db = db.clone();
            async move {
                let checked_at = Utc::now().to_rfc3339();
                let result = match site_factory::create_adapter(&site) {
                    Ok(adapter) => adapter.get_user_stats().await,
                    Err(error) => Err(error),
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
