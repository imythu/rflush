use std::cmp::Ordering;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::decision::MatchDecision;
use super::quality::QualityProfile;
use super::release::ReleaseInfo;
use super::target::MediaTarget;

const MIB: u64 = 1024 * 1024;
const ESTIMATED_SEASON_EPISODES: u64 = 8;
const SIZE_FITNESS_MAX: u32 = 1_000;

/// A larger `SortKey` is a better candidate. Accepted releases are ranked by
/// resolution, source, size fitness, eligible video features, codec, and availability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortKey {
    pub accepted: bool,
    pub score: u32,
    /// Legacy composite quality rank retained in snapshots and the public API.
    /// Candidate ordering uses the explicit component ranks below.
    pub quality_rank: u64,
    #[serde(default)]
    pub resolution_rank: u64,
    #[serde(default)]
    /// Zero until the codec-adjusted size target is reached.
    pub video_feature_rank: u32,
    #[serde(default)]
    pub source_rank: u64,
    /// 0..=1000. Reaching the target size saturates this value, so an
    /// excessively large release is not rewarded indefinitely.
    #[serde(default)]
    pub size_fitness: u32,
    #[serde(default)]
    pub size_per_item: u64,
    #[serde(default)]
    pub size_target: u64,
    #[serde(default)]
    pub codec_rank: u64,
    pub seeders: u32,
    pub publish_time: Option<DateTime<Utc>>,
    pub site_priority: u32,
    pub stable_release_key: String,
}

impl SortKey {
    #[allow(clippy::too_many_arguments)]
    pub fn from_candidate(
        decision: &MatchDecision,
        profile: &QualityProfile,
        target: &MediaTarget,
        release: &ReleaseInfo,
        size: u64,
        seeders: u32,
        publish_time: Option<DateTime<Utc>>,
        site_priority: u32,
        stable_release_key: impl Into<String>,
    ) -> Self {
        let size = size_assessment(target, release, size);
        let video_feature_rank = if size.fitness == SIZE_FITNESS_MAX {
            video_feature_rank(release)
        } else {
            0
        };
        Self {
            accepted: decision.accepted,
            score: decision.score,
            quality_rank: decision.quality_rank,
            resolution_rank: profile.resolution_rank(release),
            video_feature_rank,
            source_rank: profile.source_rank(release),
            size_fitness: size.fitness,
            size_per_item: size.per_item,
            size_target: size.target,
            codec_rank: profile.codec_rank(release),
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
            .then_with(|| self.resolution_rank.cmp(&other.resolution_rank))
            .then_with(|| self.source_rank.cmp(&other.source_rank))
            .then_with(|| self.size_fitness.cmp(&other.size_fitness))
            .then_with(|| self.video_feature_rank.cmp(&other.video_feature_rank))
            .then_with(|| self.codec_rank.cmp(&other.codec_rank))
            .then_with(|| self.seeders.cmp(&other.seeders))
            // Identity score is only a late tie-breaker after the requested
            // quality and availability precedence.
            .then_with(|| self.score.cmp(&other.score))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SizeAssessment {
    fitness: u32,
    per_item: u64,
    target: u64,
}

fn size_assessment(target: &MediaTarget, release: &ReleaseInfo, size: u64) -> SizeAssessment {
    let units = content_units(target, release);
    let per_item = size / units;
    let mut target_size = baseline_size(target, release.resolution.as_deref());

    // Efficient codecs need fewer bytes for comparable visual quality. The
    // adjustment belongs in the size check; codec preference itself is still
    // evaluated later in the ordering chain.
    target_size = match release.codec.as_deref() {
        Some("AV1") => target_size.saturating_mul(60) / 100,
        Some("H265") => target_size.saturating_mul(70) / 100,
        _ => target_size,
    };
    if !release.hdr_formats.is_empty() {
        target_size = target_size.saturating_mul(115) / 100;
    } else if release.bit_depth.is_some_and(|depth| depth >= 10) {
        target_size = target_size.saturating_mul(110) / 100;
    }
    target_size = target_size.max(1);

    let fitness = ((per_item as u128 * SIZE_FITNESS_MAX as u128) / target_size as u128)
        .min(SIZE_FITNESS_MAX as u128) as u32;
    SizeAssessment {
        fitness,
        per_item,
        target: target_size,
    }
}

fn content_units(target: &MediaTarget, release: &ReleaseInfo) -> u64 {
    let explicit_episodes = release.episodes.len().max(release.absolute_episodes.len()) as u64;
    if explicit_episodes > 0 {
        explicit_episodes
    } else if release.full_season || matches!(target, MediaTarget::Season { .. }) {
        ESTIMATED_SEASON_EPISODES
    } else {
        1
    }
}

fn baseline_size(target: &MediaTarget, resolution: Option<&str>) -> u64 {
    let mib = match target {
        MediaTarget::Movie { .. } => match resolution {
            Some("4320p") => 40_000,
            Some("2160p") => 12_000,
            Some("1440p") => 7_000,
            Some("1080p") => 4_000,
            Some("720p") => 1_800,
            Some("576p" | "480p") => 900,
            _ => 3_000,
        },
        MediaTarget::Anime { .. } => match resolution {
            Some("4320p") => 3_000,
            Some("2160p") => 1_000,
            Some("1440p") => 750,
            Some("1080p") => 500,
            Some("720p") => 280,
            Some("576p") => 200,
            Some("480p") => 150,
            _ => 450,
        },
        MediaTarget::Episode { .. } | MediaTarget::Season { .. } => match resolution {
            Some("4320p") => 6_000,
            Some("2160p") => 1_800,
            Some("1440p") => 1_200,
            Some("1080p") => 750,
            Some("720p") => 400,
            Some("576p") => 300,
            Some("480p") => 220,
            _ => 600,
        },
    };
    mib * MIB
}

fn video_feature_rank(release: &ReleaseInfo) -> u32 {
    let has = |expected: &str| release.hdr_formats.iter().any(|value| value == expected);
    let dolby_vision = has("Dolby Vision");
    let hdr10_plus = has("HDR10+");
    let hdr10 = has("HDR10");
    let other_hdr = has("HDR") || has("HLG");

    let dynamic_range = if dolby_vision && hdr10_plus {
        7
    } else if dolby_vision && hdr10 {
        6
    } else if dolby_vision && other_hdr {
        5
    } else if dolby_vision {
        4
    } else if hdr10_plus {
        3
    } else if hdr10 {
        2
    } else if other_hdr {
        1
    } else {
        0
    };
    let bit_depth = match release.bit_depth {
        Some(depth) if depth >= 12 => 3,
        Some(depth) if depth >= 10 => 2,
        Some(depth) if depth >= 8 => 1,
        _ => 0,
    };
    dynamic_range * 4 + bit_depth
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
    use crate::media::domain::decision::DecisionEngine;
    use crate::media::domain::release::ReleaseParser;
    use chrono::TimeZone;

    fn target() -> MediaTarget {
        MediaTarget::Episode {
            tmdb_id: 1,
            titles: vec!["Example Show".into()],
            year: Some(2026),
            season: 1,
            episode: 8,
            allow_season_pack: false,
        }
    }

    fn profile() -> QualityProfile {
        QualityProfile {
            resolution_order: vec!["2160p".into(), "1080p".into(), "720p".into()],
            allowed_resolutions: vec!["2160p".into(), "1080p".into(), "720p".into()],
            source_order: vec!["WEB-DL".into(), "BluRay".into(), "WEBRip".into()],
            allowed_sources: vec!["WEB-DL".into(), "BluRay".into(), "WEBRip".into()],
            codec_order: vec!["H265".into(), "H264".into(), "AV1".into()],
            minimum_score: 0,
            ..QualityProfile::default()
        }
    }

    fn key(title: &str, size_mib: u64, seeders: u32, stable: &str) -> SortKey {
        let target = target();
        let profile = profile();
        let release = ReleaseParser::with_limits(50, 2027).parse(title).unwrap();
        let decision = DecisionEngine::evaluate(&target, &release, &profile, seeders);
        SortKey::from_candidate(
            &decision,
            &profile,
            &target,
            &release,
            size_mib * MIB,
            seeders,
            Some(Utc.timestamp_opt(100, 0).unwrap()),
            1,
            stable,
        )
    }

    #[test]
    fn ranking_follows_quality_and_sufficiency_precedence() {
        let resolution = key(
            "Example.Show.2026.S01E08.2160p.WEBRip.H264",
            100,
            1,
            "resolution",
        );
        let features = key(
            "Example.Show.2026.S01E08.1080p.DV.HDR10.WEB-DL.H265.10bit",
            4_000,
            1_000,
            "features",
        );
        assert!(
            resolution > features,
            "resolution must rank before all later fields"
        );

        let hdr = key(
            "Example.Show.2026.S01E08.2160p.HDR10.WEBRip.H264",
            100,
            1,
            "hdr",
        );
        let source = key(
            "Example.Show.2026.S01E08.2160p.WEB-DL.H265",
            4_000,
            1_000,
            "source",
        );
        assert!(
            source > hdr,
            "source must rank before size and video features"
        );

        let preferred_source = key(
            "Example.Show.2026.S01E08.2160p.WEB-DL.H264",
            100,
            1,
            "preferred-source",
        );
        let adequate_size = key(
            "Example.Show.2026.S01E08.2160p.WEBRip.H265",
            4_000,
            1_000,
            "adequate-size",
        );
        assert!(
            preferred_source > adequate_size,
            "source must rank before size"
        );
    }

    #[test]
    fn undersized_4k_release_loses_before_codec_preference() {
        let small_h265 = key(
            "Example.Show.2026.S01E08.2160p.WEB-DL.H265",
            249,
            181,
            "small-h265",
        );
        let larger_h264 = key(
            "Example.Show.2026.S01E08.2160p.WEB-DL.H264",
            511,
            176,
            "larger-h264",
        );

        assert!(larger_h264.size_fitness > small_h265.size_fitness);
        assert!(larger_h264 > small_h265);
    }

    #[test]
    fn larger_h264_wins_the_reported_four_candidate_scenario() {
        let mut candidates = [
            key(
                "Example.Show.2026.S01E08.2160p.WEB-DL.H265",
                249,
                181,
                "plain-h265",
            ),
            key(
                "Example.Show.2026.S01E08.2160p.WEB-DL.HDR10.H265.10bit",
                271,
                180,
                "hdr10-h265",
            ),
            key(
                "Example.Show.2026.S01E08.2160p.WEB-DL.HDR.H265",
                273,
                171,
                "hdr-h265",
            ),
            key(
                "Example.Show.2026.S01E08.2160p.WEB-DL.H264",
                511,
                176,
                "plain-h264",
            ),
        ];
        candidates.sort_by(SortKey::compare_best_first);

        assert_eq!(candidates[0].stable_release_key, "plain-h264");
        assert_eq!(candidates[1].stable_release_key, "plain-h265");
        assert_eq!(candidates[2].stable_release_key, "hdr-h265");
        assert_eq!(candidates[3].stable_release_key, "hdr10-h265");
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.video_feature_rank == 0)
        );
    }

    #[test]
    fn rejection_remains_the_highest_priority_sort_gate() {
        let accepted = key(
            "Example.Show.2026.S01E08.720p.WEBRip.H264",
            100,
            1,
            "accepted",
        );
        let rejected = key(
            "Other.Show.2026.S01E08.2160p.DV.HDR10.WEB-DL.H265.10bit",
            4_000,
            1_000,
            "rejected",
        );
        assert!(accepted.accepted);
        assert!(!rejected.accepted);
        assert!(accepted > rejected);
    }

    #[test]
    fn adequate_sizes_saturate_then_codec_and_seeders_break_ties() {
        let h265 = key(
            "Example.Show.2026.S01E08.2160p.WEB-DL.H265",
            2_000,
            10,
            "h265",
        );
        let h264 = key(
            "Example.Show.2026.S01E08.2160p.WEB-DL.H264",
            4_000,
            1_000,
            "h264",
        );
        assert_eq!(h265.size_fitness, SIZE_FITNESS_MAX);
        assert_eq!(h264.size_fitness, SIZE_FITNESS_MAX);
        assert!(h265 > h264);

        let more_seeders = key(
            "Example.Show.2026.S01E08.2160p.WEB-DL.H265",
            2_000,
            11,
            "more-seeders",
        );
        assert!(more_seeders > h265);
    }

    #[test]
    fn dolby_vision_with_hdr_fallback_beats_plain_hdr_and_sdr_10bit() {
        let dv = key(
            "Example.Show.2026.S01E08.2160p.DV.HDR10.WEB-DL.H265.10bit",
            2_000,
            1,
            "dv",
        );
        let hdr = key(
            "Example.Show.2026.S01E08.2160p.HDR10.WEB-DL.H265.10bit",
            2_000,
            1,
            "hdr",
        );
        let ten_bit = key(
            "Example.Show.2026.S01E08.2160p.WEB-DL.H265.10bit",
            2_000,
            1,
            "10bit",
        );
        assert!(dv > hdr);
        assert!(hdr > ten_bit);
    }

    #[test]
    fn stable_tie_breaker_is_independent_of_input_order() {
        let a = key("Example.Show.2026.S01E08.2160p.WEB-DL.H265", 2_000, 3, "a");
        let b = key("Example.Show.2026.S01E08.2160p.WEB-DL.H265", 2_000, 3, "b");

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
