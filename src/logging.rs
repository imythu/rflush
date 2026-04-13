use std::sync::atomic::{AtomicU64, Ordering};

use tracing_subscriber::EnvFilter;

use crate::error::AppError;

const DEPENDENCY_LOG_DIRECTIVES: &[&str] = &[
    "hyper=info",
    "hyper_util=info",
    "h2=info",
    "reqwest=info",
    "rustls=info",
];

static NEXT_ASYNC_TASK_ID: AtomicU64 = AtomicU64::new(1);

tokio::task_local! {
    pub static TASK_LOG_CONTEXT: String;
}

pub fn build_log_filter(log_level: Option<&str>) -> Result<EnvFilter, AppError> {
    match log_level {
        Some(level) => {
            let filter = normalize_log_filter(level);
            EnvFilter::try_new(filter).map_err(|error| AppError::InvalidConfig {
                message: format!("global.log_level is invalid: {}", error),
            })
        }
        None => Ok(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))),
    }
}

fn normalize_log_filter(level: &str) -> String {
    if level.contains('=') || level.contains(',') {
        level.to_string()
    } else {
        let mut directives = Vec::with_capacity(1 + DEPENDENCY_LOG_DIRECTIVES.len());
        directives.push(level.to_string());
        directives.extend(
            DEPENDENCY_LOG_DIRECTIVES
                .iter()
                .map(|directive| directive.to_string()),
        );
        directives.join(",")
    }
}

pub fn init_logging(filter: EnvFilter) {
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_thread_ids(true)
        .with_thread_names(true)
        .init();
}

pub fn next_async_task_id() -> u64 {
    NEXT_ASYNC_TASK_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn current_task_context() -> String {
    TASK_LOG_CONTEXT
        .try_with(Clone::clone)
        .unwrap_or_else(|_| "main".to_string())
}
