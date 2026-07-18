use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::quality::{QualityAssessment, QualityProfile, QualityRejection};
use super::release::ReleaseInfo;
use super::target::MediaTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectCode {
    WrongMedia,
    WrongTitle,
    WrongYear,
    WrongSeason,
    WrongEpisode,
    AmbiguousNumbering,
    SeasonPackNotAllowed,
    QualityNotAllowed,
    UnknownQuality,
    MinimumSeeders,
    BelowMinimumScore,
}

impl RejectCode {
    #[allow(dead_code)]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WrongMedia => "wrong_media",
            Self::WrongTitle => "wrong_title",
            Self::WrongYear => "wrong_year",
            Self::WrongSeason => "wrong_season",
            Self::WrongEpisode => "wrong_episode",
            Self::AmbiguousNumbering => "ambiguous_numbering",
            Self::SeasonPackNotAllowed => "season_pack_not_allowed",
            Self::QualityNotAllowed => "quality_not_allowed",
            Self::UnknownQuality => "unknown_quality",
            Self::MinimumSeeders => "minimum_seeders",
            Self::BelowMinimumScore => "below_minimum_score",
        }
    }

    pub const fn is_permanent(self) -> bool {
        !matches!(self, Self::MinimumSeeders)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchRejection {
    pub code: RejectCode,
    pub message: String,
    pub permanent: bool,
}

impl MatchRejection {
    fn new(code: RejectCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            permanent: code.is_permanent(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub title: u32,
    pub year: u32,
    pub season: u32,
    pub episode: u32,
    pub quality: u32,
}

impl ScoreBreakdown {
    pub fn total(&self) -> u32 {
        self.title + self.year + self.season + self.episode + self.quality
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchDecision {
    pub accepted: bool,
    pub score: u32,
    pub breakdown: ScoreBreakdown,
    pub quality_rank: u64,
    pub rejections: Vec<MatchRejection>,
    pub explanations: Vec<String>,
}

pub struct IdentityGate;

impl IdentityGate {
    pub fn check(target: &MediaTarget, release: &ReleaseInfo) -> Vec<MatchRejection> {
        let mut rejections = Vec::new();

        let release_is_episode = release.season.is_some()
            || !release.episodes.is_empty()
            || !release.absolute_episodes.is_empty()
            || release.full_season;
        match target {
            MediaTarget::Movie { .. } if release_is_episode => push_rejection(
                &mut rejections,
                RejectCode::WrongMedia,
                "an episodic release cannot satisfy a movie target",
            ),
            MediaTarget::Episode { .. }
            | MediaTarget::Anime { .. }
            | MediaTarget::Season { .. }
                if release.matched_rule == "movie" =>
            {
                push_rejection(
                    &mut rejections,
                    RejectCode::WrongMedia,
                    "a movie release cannot satisfy a TV target",
                );
            }
            _ => {}
        }

        if !title_matches(target, release) {
            push_rejection(
                &mut rejections,
                RejectCode::WrongTitle,
                format!(
                    "release title '{}' does not match the target aliases",
                    release.title
                ),
            );
        }

        if let (Some(expected), Some(actual)) = (target.year(), release.year) {
            if expected != actual {
                push_rejection(
                    &mut rejections,
                    RejectCode::WrongYear,
                    format!("release year {actual} does not match target year {expected}"),
                );
            }
        }

        match target {
            MediaTarget::Movie { .. } => {}
            MediaTarget::Episode {
                season,
                episode,
                allow_season_pack,
                ..
            } => {
                check_season(&mut rejections, *season, release.season);
                if release.full_season {
                    if !allow_season_pack {
                        push_rejection(
                            &mut rejections,
                            RejectCode::SeasonPackNotAllowed,
                            "the target does not allow a full-season release",
                        );
                    }
                } else if release.episodes.is_empty() {
                    if !release.absolute_episodes.is_empty()
                        || has_unexplained_number(target, release)
                    {
                        push_rejection(
                            &mut rejections,
                            RejectCode::AmbiguousNumbering,
                            "release numbering cannot be mapped safely to season/episode numbering",
                        );
                    } else {
                        push_rejection(
                            &mut rejections,
                            RejectCode::WrongEpisode,
                            "release does not identify the requested episode",
                        );
                    }
                } else if !release.episodes.contains(episode) {
                    push_rejection(
                        &mut rejections,
                        RejectCode::WrongEpisode,
                        format!("release does not contain episode {episode}"),
                    );
                }
            }
            MediaTarget::Anime {
                absolute_episode,
                season_episode,
                ..
            } => {
                if !release.absolute_episodes.is_empty() {
                    if !release.absolute_episodes.contains(absolute_episode) {
                        push_rejection(
                            &mut rejections,
                            RejectCode::WrongEpisode,
                            format!("release does not contain absolute episode {absolute_episode}"),
                        );
                    }
                } else if !release.episodes.is_empty() {
                    if let Some(mapping) = season_episode {
                        check_season(&mut rejections, mapping.season, release.season);
                        if !release.episodes.contains(&mapping.episode) {
                            push_rejection(
                                &mut rejections,
                                RejectCode::WrongEpisode,
                                format!(
                                    "release does not contain mapped episode {}",
                                    mapping.episode
                                ),
                            );
                        }
                    } else {
                        push_rejection(
                            &mut rejections,
                            RejectCode::AmbiguousNumbering,
                            "season numbering is present but no absolute-number mapping is available",
                        );
                    }
                } else if release.full_season || has_unexplained_number(target, release) {
                    push_rejection(
                        &mut rejections,
                        RejectCode::AmbiguousNumbering,
                        "release does not provide an unambiguous absolute episode number",
                    );
                } else {
                    push_rejection(
                        &mut rejections,
                        RejectCode::WrongEpisode,
                        "release does not identify the requested absolute episode",
                    );
                }
            }
            MediaTarget::Season { season, .. } => {
                check_season(&mut rejections, *season, release.season);
                if !release.full_season {
                    push_rejection(
                        &mut rejections,
                        RejectCode::WrongEpisode,
                        "an individual episode cannot satisfy a season target",
                    );
                }
            }
        }

        rejections.sort_by_key(|rejection| rejection.code);
        rejections
    }
}

pub struct DecisionEngine;

impl DecisionEngine {
    pub fn evaluate(
        target: &MediaTarget,
        release: &ReleaseInfo,
        profile: &QualityProfile,
        seeders: u32,
    ) -> MatchDecision {
        let quality = profile.assess(release);
        let mut rejections = IdentityGate::check(target, release);

        if let Some(rejection) = &quality.rejection {
            match rejection {
                QualityRejection::QualityNotAllowed => push_rejection(
                    &mut rejections,
                    RejectCode::QualityNotAllowed,
                    quality.explanation.clone(),
                ),
                QualityRejection::UnknownQuality => push_rejection(
                    &mut rejections,
                    RejectCode::UnknownQuality,
                    quality.explanation.clone(),
                ),
            }
        }
        if seeders < profile.min_seeders {
            push_rejection(
                &mut rejections,
                RejectCode::MinimumSeeders,
                format!(
                    "release has {seeders} seeders, fewer than the required {}",
                    profile.min_seeders
                ),
            );
        }
        rejections.sort_by_key(|rejection| rejection.code);

        // Identity, quality, and availability gates are hard constraints. A rejected
        // candidate deliberately receives no compensating score.
        if !rejections.is_empty() {
            return MatchDecision {
                accepted: false,
                score: 0,
                breakdown: ScoreBreakdown::default(),
                quality_rank: quality.rank,
                explanations: rejections
                    .iter()
                    .map(|rejection| rejection.message.clone())
                    .collect(),
                rejections,
            };
        }

        let breakdown = score(target, release, &quality);
        let total = breakdown.total();
        let mut explanations = score_explanations(&breakdown, release);

        if total < profile.minimum_score {
            let rejection = MatchRejection::new(
                RejectCode::BelowMinimumScore,
                format!(
                    "score {total} is below the profile minimum {}",
                    profile.minimum_score
                ),
            );
            explanations.push(rejection.message.clone());
            rejections.push(rejection);
        }

        MatchDecision {
            accepted: rejections.is_empty(),
            score: total,
            breakdown,
            quality_rank: quality.rank,
            rejections,
            explanations,
        }
    }
}

fn score(
    target: &MediaTarget,
    release: &ReleaseInfo,
    quality: &QualityAssessment,
) -> ScoreBreakdown {
    let year = match (target.year(), release.year) {
        (Some(expected), Some(actual)) if expected == actual => 10,
        (Some(_), None) => 5,
        (None, _) => 10,
        _ => 0,
    };

    let season = match target {
        MediaTarget::Movie { .. } | MediaTarget::Anime { .. } => 20,
        MediaTarget::Episode {
            season: expected, ..
        }
        | MediaTarget::Season {
            season: expected, ..
        } => match release.season {
            Some(actual) if actual == *expected => 20,
            None => 10,
            _ => 0,
        },
    };

    ScoreBreakdown {
        title: 40,
        year,
        season,
        episode: 20,
        quality: quality.score.min(10),
    }
}

fn score_explanations(breakdown: &ScoreBreakdown, release: &ReleaseInfo) -> Vec<String> {
    vec![
        format!("title matched (+{})", breakdown.title),
        if release.year.is_some() {
            format!("year matched (+{})", breakdown.year)
        } else {
            format!("release year is unspecified (+{})", breakdown.year)
        },
        format!("season identity matched (+{})", breakdown.season),
        format!("episode identity matched (+{})", breakdown.episode),
        format!("quality preference score (+{})", breakdown.quality),
    ]
}

fn check_season(rejections: &mut Vec<MatchRejection>, expected: u32, actual: Option<u32>) {
    match actual {
        Some(actual) if actual != expected => push_rejection(
            rejections,
            RejectCode::WrongSeason,
            format!("release season {actual} does not match target season {expected}"),
        ),
        None if expected > 1 => push_rejection(
            rejections,
            RejectCode::AmbiguousNumbering,
            format!("release does not identify season {expected}"),
        ),
        _ => {}
    }
}

fn title_matches(target: &MediaTarget, release: &ReleaseInfo) -> bool {
    let release_titles = std::iter::once(&release.title).chain(release.alternate_titles.iter());
    let mut normalized_release = HashSet::new();
    for title in release_titles {
        let normalized = normalize_title(title);
        if !normalized.is_empty() {
            normalized_release.insert(normalized);
        }
        if let Some(year) = target.year() {
            let without_year = normalize_title_without_year(title, year);
            if !without_year.is_empty() {
                normalized_release.insert(without_year);
            }
        }
    }

    target.titles().iter().any(|title| {
        let title = normalize_title(title);
        !title.is_empty() && normalized_release.contains(&title)
    })
}

fn normalize_title(title: &str) -> String {
    title
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalize_title_without_year(title: &str, year: u32) -> String {
    let year = year.to_string();
    let mut normalized = String::new();
    let mut token = String::new();
    for character in title.chars().chain(std::iter::once(' ')) {
        if character.is_alphanumeric() {
            token.extend(character.to_lowercase());
            continue;
        }
        if !token.is_empty() {
            if token != year {
                normalized.push_str(&token);
            }
            token.clear();
        }
    }
    normalized
}

fn has_unexplained_number(target: &MediaTarget, release: &ReleaseInfo) -> bool {
    let title_numbers: HashSet<u32> = target
        .titles()
        .iter()
        .flat_map(|title| numeric_tokens(title))
        .collect();
    let target_year = target.year();

    numeric_tokens(&release.raw_title)
        .into_iter()
        .any(|number| {
            !title_numbers.contains(&number)
                && Some(number) != target_year
                && !matches!(
                    number,
                    264 | 265 | 480 | 576 | 720 | 1080 | 1440 | 2160 | 4320
                )
        })
}

fn numeric_tokens(value: &str) -> Vec<u32> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in value.chars().chain(std::iter::once(' ')) {
        if character.is_ascii_digit() {
            current.push(character);
        } else if !current.is_empty() {
            if let Ok(number) = current.parse() {
                tokens.push(number);
            }
            current.clear();
        }
    }
    tokens
}

fn push_rejection(
    rejections: &mut Vec<MatchRejection>,
    code: RejectCode,
    message: impl Into<String>,
) {
    if !rejections.iter().any(|rejection| rejection.code == code) {
        rejections.push(MatchRejection::new(code, message));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::domain::release::ReleaseParser;
    use crate::media::domain::target::SeasonEpisode;

    fn episode_target(season: u32, episode: u32) -> MediaTarget {
        MediaTarget::Episode {
            tmdb_id: 1,
            titles: vec!["Example Show".into()],
            year: Some(2024),
            season,
            episode,
            allow_season_pack: false,
        }
    }

    fn profile() -> QualityProfile {
        QualityProfile {
            min_seeders: 1,
            ..QualityProfile::default()
        }
    }

    fn parse(title: &str) -> ReleaseInfo {
        ReleaseParser::with_limits(50, 2027).parse(title).unwrap()
    }

    fn codes(decision: &MatchDecision) -> Vec<RejectCode> {
        decision
            .rejections
            .iter()
            .map(|rejection| rejection.code)
            .collect()
    }

    #[test]
    fn wrong_season_and_episode_are_hard_rejections_before_scoring() {
        let release = parse("Example.Show.S02E09.2024.2160p.REMUX.HEVC");
        let decision = DecisionEngine::evaluate(&episode_target(1, 3), &release, &profile(), 100);

        assert!(!decision.accepted);
        assert_eq!(decision.score, 0);
        assert_eq!(decision.breakdown, ScoreBreakdown::default());
        assert!(codes(&decision).contains(&RejectCode::WrongSeason));
        assert!(codes(&decision).contains(&RejectCode::WrongEpisode));
    }

    #[test]
    fn wrong_title_cannot_be_compensated_by_quality() {
        let release = parse("Other.Show.S01E03.2160p.REMUX.HEVC");
        let decision = DecisionEngine::evaluate(&episode_target(1, 3), &release, &profile(), 100);

        assert_eq!(decision.score, 0);
        assert!(codes(&decision).contains(&RejectCode::WrongTitle));
    }

    #[test]
    fn movie_identity_rejects_episode_media_and_wrong_year() {
        let movie = MediaTarget::Movie {
            tmdb_id: 5,
            titles: vec!["Dune Part Two".into()],
            year: Some(2024),
        };
        let episode = parse("Dune.Part.Two.S01E01.2160p.WEB-DL");
        let wrong_media = DecisionEngine::evaluate(&movie, &episode, &profile(), 10);
        assert!(codes(&wrong_media).contains(&RejectCode::WrongMedia));
        assert_eq!(wrong_media.score, 0);

        let wrong_year_release = parse("Dune.Part.Two.2023.2160p.WEB-DL");
        let wrong_year = DecisionEngine::evaluate(&movie, &wrong_year_release, &profile(), 10);
        assert!(codes(&wrong_year).contains(&RejectCode::WrongYear));
        assert_eq!(wrong_year.score, 0);
    }

    #[test]
    fn unknown_quality_has_the_stable_identity_gate_code() {
        let release = parse("Example Show S01E03");
        let decision = DecisionEngine::evaluate(&episode_target(1, 3), &release, &profile(), 10);

        assert_eq!(codes(&decision), vec![RejectCode::UnknownQuality]);
        assert_eq!(decision.rejections[0].code.as_str(), "unknown_quality");
        assert_eq!(decision.score, 0);
    }

    #[test]
    fn season_pack_requires_explicit_permission() {
        let release = parse("Example Show S01 Complete 1080p WEB-DL");
        let denied = DecisionEngine::evaluate(&episode_target(1, 3), &release, &profile(), 10);
        assert!(codes(&denied).contains(&RejectCode::SeasonPackNotAllowed));

        let allowed_target = MediaTarget::Episode {
            tmdb_id: 1,
            titles: vec!["Example Show".into()],
            year: Some(2024),
            season: 1,
            episode: 3,
            allow_season_pack: true,
        };
        let allowed = DecisionEngine::evaluate(&allowed_target, &release, &profile(), 10);
        assert!(allowed.accepted);
    }

    #[test]
    fn quality_and_seeder_gates_have_stable_codes_and_permanence() {
        let release = parse("Example Show S01E03 1080p WEB-DL H265");
        let blocked = QualityProfile {
            blocked_codecs: vec!["HEVC".into()],
            min_seeders: 5,
            ..QualityProfile::default()
        };
        let decision = DecisionEngine::evaluate(&episode_target(1, 3), &release, &blocked, 1);

        assert_eq!(
            codes(&decision),
            vec![RejectCode::QualityNotAllowed, RejectCode::MinimumSeeders]
        );
        assert!(decision.rejections[0].permanent);
        assert!(!decision.rejections[1].permanent);
        assert_eq!(
            serde_json::to_value(RejectCode::MinimumSeeders).unwrap(),
            "minimum_seeders"
        );
    }

    #[test]
    fn accepted_candidate_uses_documented_40_10_20_20_10_weights() {
        let release = parse("Example.Show.S01E03.2024.2160p.REMUX.AV1");
        let decision = DecisionEngine::evaluate(&episode_target(1, 3), &release, &profile(), 20);

        assert!(decision.accepted);
        assert_eq!(decision.breakdown.title, 40);
        assert_eq!(decision.breakdown.year, 10);
        assert_eq!(decision.breakdown.season, 20);
        assert_eq!(decision.breakdown.episode, 20);
        assert_eq!(decision.breakdown.quality, 10);
        assert_eq!(decision.score, 100);
    }

    #[test]
    fn anime_absolute_and_mapped_numbering_are_distinct() {
        let target = MediaTarget::Anime {
            tmdb_id: 2,
            titles: vec!["One Piece".into()],
            year: None,
            absolute_episode: 1122,
            season_episode: Some(SeasonEpisode {
                season: 21,
                episode: 31,
            }),
        };

        let absolute = parse("[SubsPlease] One Piece - 1122 (1080p) [WEB-DL]");
        assert!(DecisionEngine::evaluate(&target, &absolute, &profile(), 10).accepted);

        let mapped = parse("One Piece S21E31 1080p WEB-DL");
        assert!(DecisionEngine::evaluate(&target, &mapped, &profile(), 10).accepted);

        let ambiguous_target = MediaTarget::Anime {
            tmdb_id: 2,
            titles: vec!["One Piece".into()],
            year: None,
            absolute_episode: 1122,
            season_episode: None,
        };
        let ambiguous = DecisionEngine::evaluate(&ambiguous_target, &mapped, &profile(), 10);
        assert!(codes(&ambiguous).contains(&RejectCode::AmbiguousNumbering));
    }

    #[test]
    fn bare_query_number_is_ambiguous_not_an_episode_match() {
        let release = parse("Example Show 03 1080p WEB-DL");
        let decision = DecisionEngine::evaluate(&episode_target(1, 3), &release, &profile(), 10);
        assert!(codes(&decision).contains(&RejectCode::AmbiguousNumbering));
        assert_eq!(decision.score, 0);
    }

    #[test]
    fn later_season_requires_an_explicit_season_identity() {
        let release = parse("Example Show 第3集 1080p WEB-DL H265");
        let later_season =
            DecisionEngine::evaluate(&episode_target(2, 3), &release, &profile(), 10);
        assert!(codes(&later_season).contains(&RejectCode::AmbiguousNumbering));
        assert_eq!(later_season.score, 0);

        let first_season =
            DecisionEngine::evaluate(&episode_target(1, 3), &release, &profile(), 10);
        assert!(first_season.accepted);
    }
}
