use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_OWNER_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OwnerProcessIdentity {
    Verified { pid: u32, start_time: u64 },
    Legacy { pid: u32 },
    Unverifiable,
}

pub(super) fn process_owner_id(prefix: &str) -> String {
    let sequence = NEXT_OWNER_ID.fetch_add(1, Ordering::Relaxed);
    let start_time = current_process_start_time()
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "{prefix}-v2-{}-{start_time}-{sequence:x}",
        std::process::id()
    )
}

pub(super) fn parse_process_owner(owner: &str, prefix: &str) -> Option<OwnerProcessIdentity> {
    let suffix = owner.strip_prefix(prefix)?.strip_prefix('-')?;
    if let Some(versioned) = suffix.strip_prefix("v2-") {
        let mut parts = versioned.split('-');
        let pid = parts.next()?.parse().ok()?;
        return Some(match parts.next().and_then(|value| value.parse().ok()) {
            Some(start_time) => OwnerProcessIdentity::Verified { pid, start_time },
            None => OwnerProcessIdentity::Unverifiable,
        });
    }

    suffix
        .split('-')
        .next()
        .and_then(|pid| pid.parse().ok())
        .map(|pid| OwnerProcessIdentity::Legacy { pid })
}

fn current_process_start_time() -> Option<u64> {
    static START_TIME: OnceLock<Option<u64>> = OnceLock::new();
    *START_TIME.get_or_init(|| {
        let pid = sysinfo::Pid::from_u32(std::process::id());
        let mut system = sysinfo::System::new();
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        system.process(pid).map(sysinfo::Process::start_time)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_owner_round_trips_the_current_process_identity() {
        let owner = process_owner_id("manual-subscription");
        let parsed = parse_process_owner(&owner, "manual-subscription").unwrap();

        match parsed {
            OwnerProcessIdentity::Verified { pid, start_time } => {
                assert_eq!(pid, std::process::id());
                assert!(start_time > 0);
            }
            other => panic!("expected a verified process owner, got {other:?}"),
        }
    }

    #[test]
    fn legacy_and_unverifiable_owners_are_distinguished() {
        assert_eq!(
            parse_process_owner("manual-subscription-41-1000", "manual-subscription"),
            Some(OwnerProcessIdentity::Legacy { pid: 41 })
        );
        assert_eq!(
            parse_process_owner("manual-subscription-v2-41-unknown-1", "manual-subscription"),
            Some(OwnerProcessIdentity::Unverifiable)
        );
    }
}
