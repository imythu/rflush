use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::{Instant, sleep};
use tracing::{debug, warn};

#[derive(Debug, Clone, Copy)]
pub struct RateLimitPolicy {
    pub max_requests: usize,
    pub interval: Duration,
    pub throttle_duration: Duration,
}

impl RateLimitPolicy {
    pub fn new(max_requests: u32, interval: Duration, throttle_duration: Duration) -> Self {
        Self {
            max_requests: max_requests as usize,
            interval,
            throttle_duration,
        }
    }
}

#[derive(Default)]
pub struct SharedRateLimiter {
    states: Mutex<HashMap<String, Arc<DomainGate>>>,
}

struct DomainState {
    timestamps: VecDeque<Instant>,
    throttle_until: Option<Instant>,
}

struct DomainGate {
    state: Mutex<DomainState>,
}

impl SharedRateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn throttle(&self, key: &str, policy: RateLimitPolicy) {
        let gate = self.get_or_create_gate(key).await;
        let mut state = gate.state.lock().await;
        let now = Instant::now();
        if state.throttle_until.is_some_and(|t| t > now) {
            debug!("rate limiter: key={} throttle already active, skipping", key);
            return;
        }
        let until = now + policy.throttle_duration;
        state.throttle_until = Some(until);
        warn!(
            "rate limiter: key={} THROTTLED for {}s",
            key,
            policy.throttle_duration.as_secs()
        );
    }

    pub async fn acquire(&self, key: &str, policy: RateLimitPolicy) {
        let gate = self.get_or_create_gate(key).await;
        let mut state = gate.state.lock().await;
        loop {
            let now = Instant::now();

            if let Some(until) = state.throttle_until {
                if now < until {
                    let wait = until.saturating_duration_since(now);
                    warn!(
                        "rate limiter: key={} throttle active, waiting {}ms",
                        key,
                        wait.as_millis()
                    );
                    sleep(wait).await;
                    state.throttle_until = None;
                    continue;
                }
                state.throttle_until = None;
            }

            while state
                .timestamps
                .front()
                .is_some_and(|ts| now.duration_since(*ts) >= policy.interval)
            {
                state.timestamps.pop_front();
            }

            if state.timestamps.len() < policy.max_requests {
                state.timestamps.push_back(now);
                return;
            }

            let oldest = *state.timestamps.front().expect("non-empty after length check");
            let wait = (oldest + policy.interval).saturating_duration_since(now);
            debug!(
                "rate limiter: key={} wait_ms={} slots={}/{}",
                key,
                wait.as_millis(),
                state.timestamps.len(),
                policy.max_requests
            );
            sleep(wait).await;
        }
    }

    async fn get_or_create_gate(&self, key: &str) -> Arc<DomainGate> {
        let mut states = self.states.lock().await;
        states
            .entry(key.to_string())
            .or_insert_with(|| {
                Arc::new(DomainGate {
                    state: Mutex::new(DomainState {
                        timestamps: VecDeque::new(),
                        throttle_until: None,
                    }),
                })
            })
            .clone()
    }
}
