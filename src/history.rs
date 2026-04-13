use serde::{Deserialize, Serialize};
use tracing::{debug, info};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalStatus {
    Success,
    SkippedExisting,
    Failed,
}

pub fn count_by_status<'a>(
    statuses: impl Iterator<Item = &'a FinalStatus>,
) -> (usize, usize, usize) {
    let mut succeeded = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    for status in statuses {
        match status {
            FinalStatus::Success => succeeded += 1,
            FinalStatus::SkippedExisting => skipped += 1,
            FinalStatus::Failed => failed += 1,
        }
    }
    (succeeded, skipped, failed)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentRunRecord {
    pub rss_name: String,
    pub guid: String,
    pub title: String,
    pub retry_count: u32,
    pub refresh_count: u32,
    pub bytes: Option<u64>,
    pub file_name: Option<String>,
    pub saved_path: Option<String>,
    pub final_status: FinalStatus,
    pub final_message: Option<String>,
}

impl TorrentRunRecord {
    pub fn new(rss_name: String, guid: String, title: String) -> Self {
        Self {
            rss_name,
            guid,
            title,
            retry_count: 0,
            refresh_count: 0,
            bytes: None,
            file_name: None,
            saved_path: None,
            final_status: FinalStatus::Failed,
            final_message: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RssRunSummary {
    pub name: String,
    pub url: String,
    pub fetch_attempts: u32,
    pub fetch_error: Option<String>,
    pub final_rss_version: u64,
    pub parsed_items: usize,
    pub succeeded: usize,
    pub skipped_existing: usize,
    pub failed: usize,
}

impl RssRunSummary {
    pub fn from_records(
        name: String,
        url: String,
        fetch_attempts: u32,
        final_rss_version: u64,
        parsed_items: usize,
        records: &[&TorrentRunRecord],
    ) -> Self {
        let (succeeded, skipped_existing, failed) =
            count_by_status(records.iter().map(|r| &r.final_status));

        Self {
            name,
            url,
            fetch_attempts,
            fetch_error: None,
            final_rss_version,
            parsed_items,
            succeeded,
            skipped_existing,
            failed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub total: usize,
    pub succeeded: usize,
    pub skipped_existing: usize,
    pub failed: usize,
}

impl RunSummary {
    pub fn from_records(records: &[TorrentRunRecord]) -> Self {
        let (succeeded, skipped_existing, failed) =
            count_by_status(records.iter().map(|r| &r.final_status));

        Self {
            total: records.len(),
            succeeded,
            skipped_existing,
            failed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunHistory {
    pub started_at: String,
    pub finished_at: String,
    pub retry_delay_secs: u64,
    pub rss: Vec<RssRunSummary>,
    pub torrents: Vec<TorrentRunRecord>,
    pub summary: RunSummary,
}

pub struct OutputLogger;

macro_rules! log_by_status {
    ($status:expr, $($arg:tt)*) => {
        match $status {
            FinalStatus::Failed => info!($($arg)*),
            FinalStatus::Success | FinalStatus::SkippedExisting => debug!($($arg)*),
        }
    };
}

impl OutputLogger {
    pub fn log(history: &RunHistory) {
        info!("Run finished.");
        info!(
            "Summary: total={}, success={}, skipped={}, failed={}",
            history.summary.total,
            history.summary.succeeded,
            history.summary.skipped_existing,
            history.summary.failed
        );
        info!("Per RSS:");

        for rss in &history.rss {
            if let Some(fetch_error) = &rss.fetch_error {
                info!(
                    "- {} | fetch_attempts={} | fetch_failed={} | url={}",
                    rss.name, rss.fetch_attempts, fetch_error, rss.url
                );
            } else {
                info!(
                    "- {} | items={} | version={} | fetch_attempts={} | success={} | skipped={} | failed={}",
                    rss.name,
                    rss.parsed_items,
                    rss.final_rss_version,
                    rss.fetch_attempts,
                    rss.succeeded,
                    rss.skipped_existing,
                    rss.failed
                );
            }
        }

        info!("Failed torrents:");
        let failures = history
            .torrents
            .iter()
            .filter(|torrent| matches!(torrent.final_status, FinalStatus::Failed));
        let mut failure_count = 0usize;
        for torrent in failures {
            failure_count += 1;
            log_torrent(torrent);
        }
        if failure_count == 0 {
            info!("- none");
        }
    }
}

fn log_torrent(torrent: &TorrentRunRecord) {
    let status = match torrent.final_status {
        FinalStatus::Success => "SUCCESS",
        FinalStatus::SkippedExisting => "SKIPPED",
        FinalStatus::Failed => "FAILED",
    };
    let s = torrent.final_status;

    log_by_status!(
        s,
        "- [{}] {} | rss={} | retries={} | refreshes={}",
        status,
        torrent.title,
        torrent.rss_name,
        torrent.retry_count,
        torrent.refresh_count
    );
    log_by_status!(s, "  guid={}", torrent.guid);

    if let Some(path) = &torrent.saved_path {
        log_by_status!(s, "  file={}", path);
    }

    if let Some(message) = &torrent.final_message {
        log_by_status!(s, "  final_message={}", message);
    }
}
