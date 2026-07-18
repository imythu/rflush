use std::cmp::Ordering;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::decision::MatchDecision;

/// A larger `SortKey` is a better candidate. Use descending ordering, or
/// `SortKey::compare_best_first`, when sorting a result list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortKey {
    pub accepted: bool,
    pub score: u32,
    pub quality_rank: u64,
    pub seeders: u32,
    pub publish_time: Option<DateTime<Utc>>,
    pub site_priority: u32,
    pub stable_release_key: String,
}

impl SortKey {
    pub fn from_decision(
        decision: &MatchDecision,
        seeders: u32,
        publish_time: Option<DateTime<Utc>>,
        site_priority: u32,
        stable_release_key: impl Into<String>,
    ) -> Self {
        Self {
            accepted: decision.accepted,
            score: decision.score,
            quality_rank: decision.quality_rank,
            seeders,
            publish_time,
            site_priority,
            stable_release_key: stable_release_key.into(),
        }
    }

    pub fn compare_best_first(left: &Self, right: &Self) -> Ordering {
        right.cmp(left)
    }
}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.accepted
            .cmp(&other.accepted)
            .then_with(|| self.quality_rank.cmp(&other.quality_rank))
            .then_with(|| self.score.cmp(&other.score))
            .then_with(|| self.seeders.cmp(&other.seeders))
            .then_with(|| self.publish_time.cmp(&other.publish_time))
            // Lower site priority and lexical stable key are preferred, so these
            // comparisons are reversed while the other fields remain descending.
            .then_with(|| other.site_priority.cmp(&self.site_priority))
            .then_with(|| other.stable_release_key.cmp(&self.stable_release_key))
    }
}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub fn stable_release_key(source_site: &str, torrent_id: &str, raw_title: &str) -> String {
    let source_site = normalize_component(source_site);
    let torrent_id = normalize_component(torrent_id);
    let raw_title = normalize_component(raw_title);
    format!("{source_site}:{torrent_id}:{raw_title}")
}

fn normalize_component(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::domain::decision::{MatchDecision, ScoreBreakdown};
    use chrono::TimeZone;

    fn decision(accepted: bool, score: u32, quality_rank: u64) -> MatchDecision {
        MatchDecision {
            accepted,
            score,
            breakdown: ScoreBreakdown::default(),
            quality_rank,
            rejections: Vec::new(),
            explanations: Vec::new(),
        }
    }

    fn key(
        accepted: bool,
        score: u32,
        quality_rank: u64,
        seeders: u32,
        timestamp: i64,
        site_priority: u32,
        stable: &str,
    ) -> SortKey {
        SortKey::from_decision(
            &decision(accepted, score, quality_rank),
            seeders,
            Some(Utc.timestamp_opt(timestamp, 0).unwrap()),
            site_priority,
            stable,
        )
    }

    #[test]
    fn sort_key_follows_the_documented_precedence() {
        let mut values = vec![
            key(false, 100, 100, 100, 100, 0, "a"),
            key(true, 90, 100, 100, 100, 0, "a"),
            key(true, 90, 200, 1, 1, 9, "z"),
            key(true, 90, 200, 5, 1, 9, "z"),
            key(true, 90, 200, 5, 2, 9, "z"),
            key(true, 90, 200, 5, 2, 1, "z"),
            key(true, 90, 200, 5, 2, 1, "a"),
            key(true, 95, 1, 1, 1, 99, "z"),
        ];

        values.sort_by(SortKey::compare_best_first);

        assert_eq!(values[0].quality_rank, 200);
        assert_eq!(values[0].stable_release_key, "a");
        assert_eq!(values[1].site_priority, 1);
        assert_eq!(values[2].publish_time.unwrap().timestamp(), 2);
        assert_eq!(values[3].seeders, 5);
        assert_eq!(values[4].quality_rank, 200);
        assert_eq!(values[5].quality_rank, 100);
        assert_eq!(values[6].score, 95);
        assert!(values.last().is_some_and(|value| !value.accepted));
    }

    #[test]
    fn stable_tie_breaker_is_independent_of_input_order() {
        let a = key(true, 90, 10, 3, 100, 1, "a");
        let b = key(true, 90, 10, 3, 100, 1, "b");

        let mut first = vec![b.clone(), a.clone()];
        let mut second = vec![a, b];
        first.sort_by(SortKey::compare_best_first);
        second.sort_by(SortKey::compare_best_first);

        assert_eq!(first, second);
        assert_eq!(first[0].stable_release_key, "a");
    }

    #[test]
    fn stable_release_key_normalizes_case_and_whitespace() {
        assert_eq!(
            stable_release_key(" Site A ", " 42 ", "Show   S01E01"),
            "site a:42:show s01e01"
        );
    }
}
