use std::collections::{BTreeMap, btree_map::Entry};

use chrono::{DateTime, Duration, NaiveDate, Utc};

use super::models::{SubscriptionTargetRecord, target_key};
use super::tmdb::{TmdbDetails, TmdbMediaType, TmdbSeason};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionTargetSeedStatus {
    MetadataPending,
    Pending,
    Skipped,
}

impl SubscriptionTargetSeedStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MetadataPending => "metadata_pending",
            Self::Pending => "pending",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionTargetSeed {
    pub target_key: String,
    pub season: u32,
    pub episode: u32,
    pub absolute_episode: Option<u32>,
    pub air_date: Option<String>,
    pub status: SubscriptionTargetSeedStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TvTargetPlan {
    pub targets: Vec<SubscriptionTargetSeed>,
    pub terminal: bool,
}

#[derive(Debug, Clone)]
pub struct TargetSyncResult {
    pub version: i64,
    pub current: Option<SubscriptionTargetRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetReadiness {
    Due,
    Future(DateTime<Utc>),
    AwaitingMetadata,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProgressionError {
    #[error("TV target planning requires TV details")]
    NotTv,
    #[error("TMDB details id {details_id} does not match season owner {season_tmdb_id}")]
    TmdbIdMismatch {
        details_id: i64,
        season_tmdb_id: i64,
    },
    #[error("start_episode must be greater than zero")]
    InvalidStartEpisode,
    #[error("absolute episode anchor must be greater than zero")]
    InvalidAbsoluteEpisode,
    #[error("episode {episode} is absent from TMDB season {season}")]
    MissingStartEpisode { season: u32, episode: u32 },
    #[error(
        "TMDB season {season} has a gap after the requested start: expected episode {expected}, found {found}"
    )]
    EpisodeGap {
        season: u32,
        expected: u32,
        found: u32,
    },
    #[error("absolute episode mapping overflowed at season episode {episode}")]
    AbsoluteEpisodeOverflow { episode: u32 },
    #[error("cannot create a metadata frontier after episode {episode}")]
    FrontierEpisodeOverflow { episode: u32 },
    #[error("metadata refresh interval must be positive")]
    InvalidRefreshInterval,
    #[error("next metadata refresh time is outside the supported range")]
    ScheduleOverflow,
}

/// Builds all TMDB-confirmed targets from `start_episode` onward. For a season
/// that may still grow, the final item is the one unconfirmed metadata frontier.
pub fn plan_tv_targets(
    details: &TmdbDetails,
    season: &TmdbSeason,
    start_episode: u32,
    absolute_anchor: Option<u32>,
) -> Result<TvTargetPlan, ProgressionError> {
    if details.media.media_type != TmdbMediaType::Tv {
        return Err(ProgressionError::NotTv);
    }
    if details.media.tmdb_id != season.tmdb_id {
        return Err(ProgressionError::TmdbIdMismatch {
            details_id: details.media.tmdb_id,
            season_tmdb_id: season.tmdb_id,
        });
    }
    if start_episode == 0 {
        return Err(ProgressionError::InvalidStartEpisode);
    }
    if absolute_anchor == Some(0) {
        return Err(ProgressionError::InvalidAbsoluteEpisode);
    }

    let terminal = is_terminal_season(details, season.season_number);
    let episode_dates = normalized_episode_dates(season);
    let max_known = episode_dates.last_key_value().map(|(episode, _)| *episode);

    if !episode_dates.contains_key(&start_episode) {
        if terminal || max_known.is_some_and(|episode| episode >= start_episode) {
            return Err(ProgressionError::MissingStartEpisode {
                season: season.season_number,
                episode: start_episode,
            });
        }

        return Ok(TvTargetPlan {
            targets: vec![target_seed(
                details.media.tmdb_id,
                season.season_number,
                start_episode,
                start_episode,
                absolute_anchor,
                None,
                SubscriptionTargetSeedStatus::MetadataPending,
            )?],
            terminal,
        });
    }

    let mut targets = Vec::new();
    let mut previous_episode = None;
    for (&episode, air_date) in episode_dates.range(start_episode..) {
        if let Some(previous) = previous_episode {
            let expected = previous + 1;
            if episode != expected {
                return Err(ProgressionError::EpisodeGap {
                    season: season.season_number,
                    expected,
                    found: episode,
                });
            }
        }
        targets.push(target_seed(
            details.media.tmdb_id,
            season.season_number,
            start_episode,
            episode,
            absolute_anchor,
            air_date.clone(),
            SubscriptionTargetSeedStatus::Pending,
        )?);
        previous_episode = Some(episode);
    }

    let last_episode = previous_episode.expect("start episode exists in the normalized map");
    if let Some(frontier_episode) = last_episode.checked_add(1) {
        targets.push(target_seed(
            details.media.tmdb_id,
            season.season_number,
            start_episode,
            frontier_episode,
            absolute_anchor,
            None,
            if terminal {
                SubscriptionTargetSeedStatus::Skipped
            } else {
                SubscriptionTargetSeedStatus::MetadataPending
            },
        )?);
    } else if !terminal {
        return Err(ProgressionError::FrontierEpisodeOverflow {
            episode: last_episode,
        });
    }

    Ok(TvTargetPlan { targets, terminal })
}

/// Returns true only when TMDB provides positive evidence that the selected
/// season cannot receive another regular episode.
pub fn is_terminal_season(details: &TmdbDetails, season_number: u32) -> bool {
    let terminal_status = details.status.as_deref().is_some_and(|status| {
        matches!(
            status.trim().to_ascii_lowercase().as_str(),
            "ended" | "canceled" | "cancelled"
        )
    });
    let later_regular_season = season_number > 0
        && details
            .number_of_seasons
            .is_some_and(|number_of_seasons| number_of_seasons > season_number);

    terminal_status || later_regular_season
}

/// TMDB supplies a calendar date, not an airing timezone. Midnight UTC is used
/// as a deterministic lower bound; ordinary scans handle later availability.
pub fn air_date_eligible_at(air_date: &str) -> Option<DateTime<Utc>> {
    NaiveDate::parse_from_str(air_date.trim(), "%Y-%m-%d")
        .ok()?
        .and_hms_opt(0, 0, 0)
        .map(|date_time| date_time.and_utc())
}

pub fn target_readiness(
    status: SubscriptionTargetSeedStatus,
    air_date: Option<&str>,
    terminal_season: bool,
    now: DateTime<Utc>,
) -> TargetReadiness {
    if status != SubscriptionTargetSeedStatus::Pending {
        return TargetReadiness::AwaitingMetadata;
    }

    match air_date.and_then(air_date_eligible_at) {
        Some(eligible_at) if eligible_at > now => TargetReadiness::Future(eligible_at),
        Some(_) => TargetReadiness::Due,
        None if terminal_season => TargetReadiness::Due,
        None => TargetReadiness::AwaitingMetadata,
    }
}

pub fn next_run_at(
    readiness: &TargetReadiness,
    now: DateTime<Utc>,
    metadata_refresh_interval: Duration,
) -> Result<DateTime<Utc>, ProgressionError> {
    match readiness {
        TargetReadiness::Due => Ok(now),
        TargetReadiness::Future(eligible_at) => Ok((*eligible_at).max(now)),
        TargetReadiness::AwaitingMetadata => {
            if metadata_refresh_interval <= Duration::zero() {
                return Err(ProgressionError::InvalidRefreshInterval);
            }
            now.checked_add_signed(metadata_refresh_interval)
                .ok_or(ProgressionError::ScheduleOverflow)
        }
    }
}

fn normalized_episode_dates(season: &TmdbSeason) -> BTreeMap<u32, Option<String>> {
    let mut dates = BTreeMap::new();
    for episode in &season.episodes {
        if episode.season_number != season.season_number || episode.episode_number == 0 {
            continue;
        }
        let candidate = episode.air_date.as_deref().and_then(normalized_air_date);
        match dates.entry(episode.episode_number) {
            Entry::Vacant(entry) => {
                entry.insert(candidate);
            }
            Entry::Occupied(mut entry) => prefer_air_date(entry.get_mut(), candidate),
        }
    }
    dates
}

fn normalized_air_date(value: &str) -> Option<String> {
    let date = NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()?;
    Some(date.format("%Y-%m-%d").to_string())
}

fn prefer_air_date(existing: &mut Option<String>, candidate: Option<String>) {
    let Some(candidate) = candidate else {
        return;
    };
    if existing.as_ref().is_none_or(|current| candidate < *current) {
        *existing = Some(candidate);
    }
}

fn target_seed(
    tmdb_id: i64,
    season: u32,
    start_episode: u32,
    episode: u32,
    absolute_anchor: Option<u32>,
    air_date: Option<String>,
    status: SubscriptionTargetSeedStatus,
) -> Result<SubscriptionTargetSeed, ProgressionError> {
    let absolute_episode = absolute_anchor
        .map(|anchor| {
            let offset = episode - start_episode;
            anchor
                .checked_add(offset)
                .ok_or(ProgressionError::AbsoluteEpisodeOverflow { episode })
        })
        .transpose()?;

    Ok(SubscriptionTargetSeed {
        target_key: target_key("tv", tmdb_id, Some(season), Some(episode), absolute_episode),
        season,
        episode,
        absolute_episode,
        air_date,
        status,
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::media::tmdb::{TmdbEpisode, TmdbMedia};

    fn details(status: Option<&str>, number_of_seasons: Option<u32>) -> TmdbDetails {
        TmdbDetails {
            media: TmdbMedia {
                tmdb_id: 42,
                media_type: TmdbMediaType::Tv,
                title: "Example".to_string(),
                original_title: None,
                year: Some(2026),
                overview: String::new(),
                poster_path: None,
                is_animation: false,
            },
            aliases: Vec::new(),
            number_of_seasons,
            status: status.map(str::to_string),
        }
    }

    fn episode(season: u32, number: u32, air_date: Option<&str>) -> TmdbEpisode {
        TmdbEpisode {
            id: i64::from(number),
            season_number: season,
            episode_number: number,
            name: format!("Episode {number}"),
            overview: String::new(),
            air_date: air_date.map(str::to_string),
            runtime: None,
        }
    }

    fn season(episodes: Vec<TmdbEpisode>) -> TmdbSeason {
        TmdbSeason {
            id: 7,
            tmdb_id: 42,
            season_number: 2,
            name: "Season 2".to_string(),
            overview: String::new(),
            poster_path: None,
            air_date: None,
            episodes,
        }
    }

    #[test]
    fn sorts_deduplicates_maps_absolute_numbers_and_adds_one_frontier() {
        let plan = plan_tv_targets(
            &details(Some("Returning Series"), Some(2)),
            &season(vec![
                episode(2, 4, Some("2026-04-04")),
                episode(2, 2, None),
                episode(2, 3, Some("invalid")),
                episode(2, 2, Some("2026-04-02")),
                episode(1, 99, Some("2026-01-01")),
            ]),
            2,
            Some(100),
        )
        .unwrap();

        assert!(!plan.terminal);
        assert_eq!(
            plan.targets
                .iter()
                .map(|target| target.episode)
                .collect::<Vec<_>>(),
            vec![2, 3, 4, 5]
        );
        assert_eq!(plan.targets[0].air_date.as_deref(), Some("2026-04-02"));
        assert_eq!(plan.targets[1].air_date, None);
        assert_eq!(plan.targets[0].absolute_episode, Some(100));
        assert_eq!(plan.targets[2].absolute_episode, Some(102));
        assert_eq!(plan.targets[2].target_key, "tv:42:abs0102");
        assert_eq!(
            plan.targets.last().unwrap().status,
            SubscriptionTargetSeedStatus::MetadataPending
        );
        assert_eq!(plan.targets.last().unwrap().absolute_episode, Some(103));
    }

    #[test]
    fn standard_episode_keys_do_not_use_absolute_numbering() {
        let plan = plan_tv_targets(
            &details(Some("Returning Series"), Some(2)),
            &season(vec![episode(2, 3, Some("2026-03-01"))]),
            3,
            None,
        )
        .unwrap();

        assert_eq!(plan.targets[0].target_key, "tv:42:s02e03");
        assert_eq!(plan.targets[1].target_key, "tv:42:s02e04");
    }

    #[test]
    fn an_unannounced_start_becomes_the_only_frontier_for_an_ongoing_season() {
        let plan = plan_tv_targets(
            &details(Some("Returning Series"), Some(2)),
            &season(vec![episode(2, 1, Some("2026-01-01"))]),
            3,
            Some(12),
        )
        .unwrap();

        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].episode, 3);
        assert_eq!(plan.targets[0].absolute_episode, Some(12));
        assert_eq!(
            plan.targets[0].status,
            SubscriptionTargetSeedStatus::MetadataPending
        );
    }

    #[test]
    fn rejects_a_missing_start_inside_the_known_range() {
        let error = plan_tv_targets(
            &details(Some("Returning Series"), Some(2)),
            &season(vec![
                episode(2, 1, Some("2026-01-01")),
                episode(2, 3, Some("2026-01-15")),
            ]),
            2,
            None,
        )
        .unwrap_err();

        assert_eq!(
            error,
            ProgressionError::MissingStartEpisode {
                season: 2,
                episode: 2
            }
        );
    }

    #[test]
    fn rejects_internal_episode_gaps_instead_of_silently_skipping_them() {
        let error = plan_tv_targets(
            &details(Some("Returning Series"), Some(2)),
            &season(vec![
                episode(2, 2, Some("2026-01-01")),
                episode(2, 4, Some("2026-01-15")),
            ]),
            2,
            Some(20),
        )
        .unwrap_err();

        assert_eq!(
            error,
            ProgressionError::EpisodeGap {
                season: 2,
                expected: 3,
                found: 4
            }
        );
    }

    #[test]
    fn terminal_seasons_have_one_skipped_frontier() {
        let plan = plan_tv_targets(
            &details(Some(" Ended "), Some(2)),
            &season(vec![episode(2, 1, Some("2020-01-01"))]),
            1,
            None,
        )
        .unwrap();

        assert!(plan.terminal);
        assert_eq!(plan.targets.len(), 2);
        assert_eq!(
            plan.targets[0].status,
            SubscriptionTargetSeedStatus::Pending
        );
        assert_eq!(
            plan.targets[1].status,
            SubscriptionTargetSeedStatus::Skipped
        );
    }

    #[test]
    fn terminal_detection_requires_positive_tmdb_evidence() {
        assert!(is_terminal_season(&details(Some("Canceled"), Some(2)), 2));
        assert!(is_terminal_season(
            &details(Some("Returning Series"), Some(3)),
            2
        ));
        assert!(!is_terminal_season(
            &details(Some("Returning Series"), Some(2)),
            2
        ));
        assert!(!is_terminal_season(
            &details(Some("Returning Series"), Some(4)),
            0
        ));
    }

    #[test]
    fn checked_arithmetic_rejects_absolute_and_frontier_overflow() {
        let absolute_error = plan_tv_targets(
            &details(Some("Ended"), Some(2)),
            &season(vec![
                episode(2, 1, Some("2020-01-01")),
                episode(2, 2, Some("2020-01-08")),
            ]),
            1,
            Some(u32::MAX),
        )
        .unwrap_err();
        assert_eq!(
            absolute_error,
            ProgressionError::AbsoluteEpisodeOverflow { episode: 2 }
        );

        let frontier_error = plan_tv_targets(
            &details(Some("Returning Series"), Some(2)),
            &season(vec![episode(2, u32::MAX, Some("2026-01-01"))]),
            u32::MAX,
            None,
        )
        .unwrap_err();
        assert_eq!(
            frontier_error,
            ProgressionError::FrontierEpisodeOverflow { episode: u32::MAX }
        );
    }

    #[test]
    fn readiness_uses_air_date_without_searching_unconfirmed_targets() {
        let now = Utc.with_ymd_and_hms(2026, 7, 15, 10, 0, 0).unwrap();
        let future = Utc.with_ymd_and_hms(2026, 7, 20, 0, 0, 0).unwrap();

        assert_eq!(
            target_readiness(
                SubscriptionTargetSeedStatus::MetadataPending,
                Some("2020-01-01"),
                true,
                now
            ),
            TargetReadiness::AwaitingMetadata
        );
        assert_eq!(
            target_readiness(
                SubscriptionTargetSeedStatus::Pending,
                Some("2026-07-20"),
                false,
                now
            ),
            TargetReadiness::Future(future)
        );
        assert_eq!(
            target_readiness(
                SubscriptionTargetSeedStatus::Pending,
                Some("2026-07-15"),
                false,
                now
            ),
            TargetReadiness::Due
        );
        assert_eq!(
            target_readiness(SubscriptionTargetSeedStatus::Pending, None, false, now),
            TargetReadiness::AwaitingMetadata
        );
        assert_eq!(
            target_readiness(
                SubscriptionTargetSeedStatus::Pending,
                Some("bad-date"),
                true,
                now
            ),
            TargetReadiness::Due
        );
    }

    #[test]
    fn scheduling_uses_due_time_or_checked_metadata_refresh_interval() {
        let now = Utc.with_ymd_and_hms(2026, 7, 15, 10, 0, 0).unwrap();
        let future = Utc.with_ymd_and_hms(2026, 7, 20, 0, 0, 0).unwrap();

        assert_eq!(
            next_run_at(&TargetReadiness::Due, now, Duration::minutes(30)).unwrap(),
            now
        );
        assert_eq!(
            next_run_at(&TargetReadiness::Future(future), now, Duration::minutes(30)).unwrap(),
            future
        );
        assert_eq!(
            next_run_at(
                &TargetReadiness::AwaitingMetadata,
                now,
                Duration::minutes(30)
            )
            .unwrap(),
            now + Duration::minutes(30)
        );
        assert_eq!(
            next_run_at(&TargetReadiness::AwaitingMetadata, now, Duration::zero()).unwrap_err(),
            ProgressionError::InvalidRefreshInterval
        );
    }

    #[test]
    fn rejects_wrong_media_and_mismatched_tmdb_ids() {
        let mut movie = details(None, None);
        movie.media.media_type = TmdbMediaType::Movie;
        assert_eq!(
            plan_tv_targets(&movie, &season(Vec::new()), 1, None).unwrap_err(),
            ProgressionError::NotTv
        );

        let mut wrong_season = season(Vec::new());
        wrong_season.tmdb_id = 99;
        assert_eq!(
            plan_tv_targets(&details(None, None), &wrong_season, 1, None).unwrap_err(),
            ProgressionError::TmdbIdMismatch {
                details_id: 42,
                season_tmdb_id: 99
            }
        );
    }
}
