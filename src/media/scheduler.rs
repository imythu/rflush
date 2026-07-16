use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinSet;
use tracing::{debug, error, info, trace, warn};

use crate::error::AppError;

use super::lease::{OwnerProcessIdentity, parse_process_owner, process_owner_id};
use super::service::MediaService;

const MIN_POLL_INTERVAL: Duration = Duration::from_millis(250);
const STARTUP_RECOVERY_RETRY_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
pub struct MediaSchedulerConfig {
    pub subscription_poll_interval: Duration,
    pub download_poll_interval: Duration,
    pub recovery_interval: Duration,
    pub subscription_lease: Duration,
    pub download_lease: Duration,
    pub subscription_batch_size: usize,
    pub download_batch_size: usize,
}

impl Default for MediaSchedulerConfig {
    fn default() -> Self {
        Self {
            subscription_poll_interval: Duration::from_secs(15),
            download_poll_interval: Duration::from_secs(2),
            recovery_interval: Duration::from_secs(30),
            subscription_lease: Duration::from_secs(10 * 60),
            download_lease: Duration::from_secs(5 * 60),
            subscription_batch_size: 1,
            download_batch_size: 16,
        }
    }
}

impl MediaSchedulerConfig {
    fn normalized(mut self) -> Self {
        self.subscription_poll_interval = bounded_poll_interval(self.subscription_poll_interval);
        self.download_poll_interval = bounded_poll_interval(self.download_poll_interval);
        self.recovery_interval = bounded_poll_interval(self.recovery_interval);
        self.subscription_lease = self.subscription_lease.max(Duration::from_secs(10));
        self.download_lease = self.download_lease.max(Duration::from_secs(10));
        self.subscription_batch_size = 1;
        self.download_batch_size = self.download_batch_size.max(1);
        self
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct SchedulerControl {
    stopped: bool,
    wake_generation: u64,
}

pub struct MediaScheduler {
    service: Arc<MediaService>,
    config: MediaSchedulerConfig,
    control: watch::Sender<SchedulerControl>,
    started: AtomicBool,
    subscription_owner: String,
    download_owner: String,
}

impl MediaScheduler {
    pub fn new(service: Arc<MediaService>) -> Arc<Self> {
        Self::with_config(service, MediaSchedulerConfig::default())
    }

    pub fn with_config(service: Arc<MediaService>, config: MediaSchedulerConfig) -> Arc<Self> {
        let (control, _) = watch::channel(SchedulerControl::default());
        Arc::new(Self {
            service,
            config: config.normalized(),
            control,
            started: AtomicBool::new(false),
            subscription_owner: worker_owner("subscriptions"),
            download_owner: worker_owner("downloads"),
        })
    }

    /// Runs until [`Self::stop`] is called. A scheduler instance is single-use.
    pub async fn start(&self) {
        if self.started.swap(true, Ordering::AcqRel) {
            warn!("media scheduler start ignored because it is already running or has stopped");
            return;
        }

        info!(
            subscription_poll_secs = self.config.subscription_poll_interval.as_secs_f64(),
            download_poll_secs = self.config.download_poll_interval.as_secs_f64(),
            "media scheduler started"
        );

        let mut startup_control = self.control.subscribe();
        if !self.recover_startup_leases(&mut startup_control).await {
            info!("media scheduler stopped before startup recovery completed");
            return;
        }

        tokio::join!(
            self.subscription_loop(self.control.subscribe()),
            self.download_loop(self.control.subscribe()),
            self.recovery_loop(self.control.subscribe()),
        );
        info!("media scheduler stopped");
    }

    /// Interrupts idle waits so newly-created subscriptions or downloads run promptly.
    pub fn wake(&self) {
        self.control.send_modify(|control| {
            if !control.stopped {
                control.wake_generation = control.wake_generation.wrapping_add(1);
            }
        });
    }

    /// Requests a graceful stop. Already-claimed work is allowed to finish.
    pub fn stop(&self) {
        self.control.send_modify(|control| {
            control.stopped = true;
            control.wake_generation = control.wake_generation.wrapping_add(1);
        });
    }

    pub fn is_stopped(&self) -> bool {
        self.control.borrow().stopped
    }

    pub async fn release_owned_leases(&self) -> Result<(usize, usize), AppError> {
        let subscriptions = self
            .service
            .database()
            .recover_subscription_leases_for_owners(std::slice::from_ref(&self.subscription_owner))
            .await?;
        let downloads = self
            .service
            .database()
            .recover_media_download_leases_for_owners(std::slice::from_ref(&self.download_owner))
            .await?;
        Ok((subscriptions, downloads))
    }

    async fn recover_startup_leases(
        &self,
        control: &mut watch::Receiver<SchedulerControl>,
    ) -> bool {
        loop {
            if control.borrow().stopped {
                return false;
            }
            match self.recover_media_leases().await {
                Ok((
                    abandoned_subscriptions,
                    abandoned_downloads,
                    expired_subscriptions,
                    expired_downloads,
                )) => {
                    info!(
                        recovered_abandoned_subscriptions = abandoned_subscriptions,
                        recovered_abandoned_downloads = abandoned_downloads,
                        recovered_expired_subscriptions = expired_subscriptions,
                        recovered_expired_downloads = expired_downloads,
                        "media scheduler startup lease recovery completed"
                    );
                    return true;
                }
                Err(error) => {
                    error!(
                        %error,
                        retry_secs = STARTUP_RECOVERY_RETRY_INTERVAL.as_secs(),
                        "media scheduler startup lease recovery failed"
                    );
                }
            }

            if !wait_for_tick(control, STARTUP_RECOVERY_RETRY_INTERVAL).await {
                return false;
            }
        }
    }

    async fn subscription_loop(&self, mut control: watch::Receiver<SchedulerControl>) {
        loop {
            if control.borrow().stopped {
                return;
            }
            self.run_subscription_batch().await;
            if !wait_for_tick(&mut control, self.config.subscription_poll_interval).await {
                return;
            }
        }
    }

    async fn download_loop(&self, mut control: watch::Receiver<SchedulerControl>) {
        loop {
            if control.borrow().stopped {
                return;
            }
            self.run_download_batch().await;
            if !wait_for_tick(&mut control, self.config.download_poll_interval).await {
                return;
            }
        }
    }

    async fn recovery_loop(&self, mut control: watch::Receiver<SchedulerControl>) {
        loop {
            if !wait_for_tick(&mut control, self.config.recovery_interval).await {
                return;
            }
            match self.recover_media_leases().await {
                Ok((0, 0, 0, 0)) => {}
                Ok((
                    abandoned_subscriptions,
                    abandoned_downloads,
                    expired_subscriptions,
                    expired_downloads,
                )) => info!(
                    recovered_abandoned_subscriptions = abandoned_subscriptions,
                    recovered_abandoned_downloads = abandoned_downloads,
                    recovered_expired_subscriptions = expired_subscriptions,
                    recovered_expired_downloads = expired_downloads,
                    "media scheduler recovered expired leases"
                ),
                Err(error) => error!(%error, "media scheduler lease recovery failed"),
            }
        }
    }

    async fn recover_media_leases(&self) -> Result<(usize, usize, usize, usize), AppError> {
        let mut owners = self
            .service
            .database()
            .list_active_subscription_lease_owners()
            .await?;
        owners.extend(
            self.service
                .database()
                .list_active_media_download_lease_owners()
                .await?,
        );
        owners.sort_unstable();
        owners.dedup();
        let abandoned = tokio::task::spawn_blocking(move || abandoned_lease_owners(&owners))
            .await
            .map_err(|error| AppError::Server {
                message: format!("failed to inspect media lease owner processes: {error}"),
            })?;
        let abandoned_subscriptions = self
            .service
            .database()
            .recover_subscription_leases_for_owners(&abandoned)
            .await?;
        let abandoned_downloads = self
            .service
            .database()
            .recover_media_download_leases_for_owners(&abandoned)
            .await?;
        let (expired_subscriptions, expired_downloads) = self
            .service
            .database()
            .recover_expired_media_leases()
            .await?;
        Ok((
            abandoned_subscriptions,
            abandoned_downloads,
            expired_subscriptions,
            expired_downloads,
        ))
    }

    async fn run_subscription_batch(&self) {
        let claimed = match self
            .service
            .database()
            .claim_due_subscriptions(
                &self.subscription_owner,
                duration_as_i64_seconds(self.config.subscription_lease),
                self.config.subscription_batch_size,
            )
            .await
        {
            Ok(claimed) => claimed,
            Err(error) => {
                error!(%error, "media scheduler failed to claim due subscriptions");
                return;
            }
        };
        if claimed.is_empty() {
            trace!("media scheduler found no due subscriptions");
            return;
        }

        debug!(
            count = claimed.len(),
            "media scheduler claimed subscriptions"
        );
        for subscription in claimed {
            let subscription_id = subscription.id;
            match self
                .service
                .run_claimed_subscription(
                    subscription,
                    &self.subscription_owner,
                    duration_as_i64_seconds(self.config.subscription_lease),
                )
                .await
            {
                Ok(run) => info!(
                    subscription_id,
                    target_key = %run.target_key,
                    candidates = run.candidate_count,
                    accepted = run.accepted_count,
                    queued = run.download.is_some(),
                    "media subscription scan completed"
                ),
                Err(error) => {
                    error!(subscription_id, %error, "media subscription scan failed");
                }
            }
        }
    }

    async fn run_download_batch(&self) {
        let claimed = match self
            .service
            .database()
            .claim_due_media_downloads(
                &self.download_owner,
                duration_as_i64_seconds(self.config.download_lease),
                self.config.download_batch_size,
            )
            .await
        {
            Ok(claimed) => claimed,
            Err(error) => {
                error!(%error, "media scheduler failed to claim due downloads");
                return;
            }
        };
        if claimed.is_empty() {
            trace!("media scheduler found no due downloads");
            return;
        }

        debug!(count = claimed.len(), "media scheduler claimed downloads");
        let mut jobs = JoinSet::new();
        for download in claimed {
            let service = Arc::clone(&self.service);
            let owner = self.download_owner.clone();
            let download_id = download.id;
            jobs.spawn(async move {
                (
                    download_id,
                    service.process_download(download, &owner).await,
                )
            });
        }

        while let Some(result) = jobs.join_next().await {
            match result {
                Ok((download_id, Ok(()))) => {
                    info!(download_id, "media download processing completed");
                }
                Ok((download_id, Err(error))) => {
                    error!(download_id, %error, "media download processing failed");
                }
                Err(error) => {
                    error!(%error, "media download worker terminated unexpectedly");
                }
            }
        }
    }
}

async fn wait_for_tick(
    control: &mut watch::Receiver<SchedulerControl>,
    interval: Duration,
) -> bool {
    if control.borrow().stopped {
        return false;
    }
    tokio::select! {
        _ = tokio::time::sleep(bounded_poll_interval(interval)) => true,
        changed = control.changed() => changed.is_ok() && !control.borrow().stopped,
    }
}

fn bounded_poll_interval(interval: Duration) -> Duration {
    interval.max(MIN_POLL_INTERVAL)
}

fn duration_as_i64_seconds(duration: Duration) -> i64 {
    duration.as_secs().min(i64::MAX as u64) as i64
}

fn worker_owner(role: &str) -> String {
    process_owner_id(&format!("media-{role}"))
}

fn lease_owner_process_identity(owner: &str) -> Option<OwnerProcessIdentity> {
    [
        "manual-subscription",
        "media-subscriptions",
        "media-downloads",
    ]
    .into_iter()
    .find_map(|prefix| parse_process_owner(owner, prefix))
}

fn abandoned_lease_owners(owners: &[String]) -> Vec<String> {
    let mut pids: HashSet<_> = owners
        .iter()
        .filter_map(|owner| match lease_owner_process_identity(owner) {
            Some(OwnerProcessIdentity::Verified { pid, .. })
            | Some(OwnerProcessIdentity::Legacy { pid }) => Some(pid),
            Some(OwnerProcessIdentity::Unverifiable) | None => None,
        })
        .map(sysinfo::Pid::from_u32)
        .collect();
    if pids.is_empty() {
        return Vec::new();
    }
    let current_pid = sysinfo::Pid::from_u32(std::process::id());
    pids.insert(current_pid);
    let pids: Vec<_> = pids.into_iter().collect();
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&pids), true);
    if system.process(current_pid).is_none() {
        warn!("media lease recovery skipped because process inspection is unavailable");
        return Vec::new();
    }
    let live_processes: HashMap<_, _> = pids
        .into_iter()
        .filter_map(|pid| {
            system
                .process(pid)
                .map(|process| (pid.as_u32(), process.start_time()))
        })
        .collect();
    lease_owners_without_matching_process(owners, &live_processes)
}

fn lease_owners_without_matching_process(
    owners: &[String],
    live_processes: &HashMap<u32, u64>,
) -> Vec<String> {
    owners
        .iter()
        .filter(|owner| match lease_owner_process_identity(owner) {
            Some(OwnerProcessIdentity::Verified { pid, start_time }) => live_processes
                .get(&pid)
                .is_none_or(|observed_start| *observed_start != start_time),
            Some(OwnerProcessIdentity::Legacy { pid }) => !live_processes.contains_key(&pid),
            Some(OwnerProcessIdentity::Unverifiable) | None => false,
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_config_prevents_busy_loops_and_empty_batches() {
        let config = MediaSchedulerConfig {
            subscription_poll_interval: Duration::ZERO,
            download_poll_interval: Duration::from_nanos(1),
            recovery_interval: Duration::ZERO,
            subscription_lease: Duration::ZERO,
            download_lease: Duration::from_secs(1),
            subscription_batch_size: 0,
            download_batch_size: 0,
        }
        .normalized();

        assert_eq!(config.subscription_poll_interval, MIN_POLL_INTERVAL);
        assert_eq!(config.download_poll_interval, MIN_POLL_INTERVAL);
        assert_eq!(config.recovery_interval, MIN_POLL_INTERVAL);
        assert_eq!(config.subscription_lease, Duration::from_secs(10));
        assert_eq!(config.download_lease, Duration::from_secs(10));
        assert_eq!(config.subscription_batch_size, 1);
        assert_eq!(config.download_batch_size, 1);
    }

    #[test]
    fn default_intervals_prioritize_downloads_without_busy_polling() {
        let config = MediaSchedulerConfig::default().normalized();

        assert!(config.download_poll_interval >= MIN_POLL_INTERVAL);
        assert!(config.subscription_poll_interval > config.download_poll_interval);
        assert!(config.recovery_interval >= config.subscription_poll_interval);
    }

    #[test]
    fn worker_owners_include_role_and_are_unique() {
        let first = worker_owner("subscriptions");
        let second = worker_owner("subscriptions");
        let downloads = worker_owner("downloads");

        assert!(first.starts_with("media-subscriptions-v2-"));
        assert!(downloads.starts_with("media-downloads-v2-"));
        assert_ne!(first, second);
        assert_ne!(second, downloads);
    }

    #[test]
    fn abandoned_owner_detection_uses_pid_and_process_start_time() {
        let owners = vec![
            "manual-subscription-41-1000".to_string(),
            "media-subscriptions-v2-42-1000-1".to_string(),
            "media-downloads-43-deadbeef-2".to_string(),
            "media-subscriptions-v2-44-1000-3".to_string(),
            "manual-subscription-v2-45-1000-4".to_string(),
            "manual-subscription-v2-46-unknown-5".to_string(),
            "legacy-owner".to_string(),
        ];
        let live_processes = HashMap::from([(42, 1000), (43, 2000), (44, 999)]);
        let abandoned = lease_owners_without_matching_process(&owners, &live_processes);

        assert_eq!(
            abandoned,
            vec![
                "manual-subscription-41-1000",
                "media-subscriptions-v2-44-1000-3",
                "manual-subscription-v2-45-1000-4",
            ]
        );
        assert_eq!(
            lease_owner_process_identity("manual-subscription-41-1000"),
            Some(OwnerProcessIdentity::Legacy { pid: 41 })
        );
        assert_eq!(
            lease_owner_process_identity("media-subscriptions-v2-42-1000-1"),
            Some(OwnerProcessIdentity::Verified {
                pid: 42,
                start_time: 1000,
            })
        );
        assert_eq!(lease_owner_process_identity("legacy-owner"), None);
    }

    #[test]
    fn lease_duration_saturates_at_database_integer_width() {
        assert_eq!(duration_as_i64_seconds(Duration::MAX), i64::MAX);
    }

    #[tokio::test]
    async fn wake_interrupts_an_idle_wait() {
        let (control, mut receiver) = watch::channel(SchedulerControl::default());
        control.send_modify(|state| {
            state.wake_generation = state.wake_generation.wrapping_add(1);
        });

        let keep_running = tokio::time::timeout(
            Duration::from_secs(1),
            wait_for_tick(&mut receiver, Duration::from_secs(60)),
        )
        .await
        .expect("wake should interrupt the timer");

        assert!(keep_running);
    }

    #[tokio::test]
    async fn stop_interrupts_an_idle_wait_and_ends_the_loop() {
        let (control, mut receiver) = watch::channel(SchedulerControl::default());
        control.send_modify(|state| {
            state.stopped = true;
            state.wake_generation = state.wake_generation.wrapping_add(1);
        });

        let keep_running = tokio::time::timeout(
            Duration::from_secs(1),
            wait_for_tick(&mut receiver, Duration::from_secs(60)),
        )
        .await
        .expect("stop should interrupt the timer");

        assert!(!keep_running);
    }
}
