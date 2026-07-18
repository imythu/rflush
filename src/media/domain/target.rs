use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum MediaType {
    Movie,
    Tv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SeasonEpisode {
    pub season: u32,
    pub episode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target_type", rename_all = "snake_case")]
pub enum MediaTarget {
    Movie {
        tmdb_id: i64,
        titles: Vec<String>,
        year: Option<u32>,
    },
    Episode {
        tmdb_id: i64,
        titles: Vec<String>,
        year: Option<u32>,
        season: u32,
        episode: u32,
        #[serde(default)]
        allow_season_pack: bool,
    },
    Anime {
        tmdb_id: i64,
        titles: Vec<String>,
        year: Option<u32>,
        absolute_episode: u32,
        season_episode: Option<SeasonEpisode>,
    },
    Season {
        tmdb_id: i64,
        titles: Vec<String>,
        year: Option<u32>,
        season: u32,
    },
}

impl MediaTarget {
    #[allow(dead_code)]
    pub fn media_type(&self) -> MediaType {
        match self {
            Self::Movie { .. } => MediaType::Movie,
            Self::Episode { .. } | Self::Anime { .. } | Self::Season { .. } => MediaType::Tv,
        }
    }

    pub fn tmdb_id(&self) -> i64 {
        match self {
            Self::Movie { tmdb_id, .. }
            | Self::Episode { tmdb_id, .. }
            | Self::Anime { tmdb_id, .. }
            | Self::Season { tmdb_id, .. } => *tmdb_id,
        }
    }

    pub fn titles(&self) -> &[String] {
        match self {
            Self::Movie { titles, .. }
            | Self::Episode { titles, .. }
            | Self::Anime { titles, .. }
            | Self::Season { titles, .. } => titles,
        }
    }

    pub fn year(&self) -> Option<u32> {
        match self {
            Self::Movie { year, .. }
            | Self::Episode { year, .. }
            | Self::Anime { year, .. }
            | Self::Season { year, .. } => *year,
        }
    }

    pub fn target_key(&self) -> String {
        match self {
            Self::Movie { tmdb_id, .. } => format!("movie:{tmdb_id}"),
            Self::Episode {
                tmdb_id,
                season,
                episode,
                ..
            } => format!("tv:{tmdb_id}:s{season:02}e{episode:02}"),
            Self::Anime {
                tmdb_id,
                absolute_episode,
                ..
            } => format!("tv:{tmdb_id}:abs{absolute_episode:04}"),
            Self::Season {
                tmdb_id, season, ..
            } => format!("tv:{tmdb_id}:s{season:02}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_keys_are_canonical_and_non_empty() {
        let episode = MediaTarget::Episode {
            tmdb_id: 42,
            titles: vec!["Example".into()],
            year: None,
            season: 2,
            episode: 3,
            allow_season_pack: false,
        };
        let anime = MediaTarget::Anime {
            tmdb_id: 7,
            titles: vec!["Example".into()],
            year: None,
            absolute_episode: 123,
            season_episode: None,
        };

        assert_eq!(episode.target_key(), "tv:42:s02e03");
        assert_eq!(anime.target_key(), "tv:7:abs0123");
    }

    #[test]
    fn target_serialization_has_stable_discriminator() {
        let target = MediaTarget::Movie {
            tmdb_id: 11,
            titles: vec!["Dune Part Two".into()],
            year: Some(2024),
        };
        let value = serde_json::to_value(target).unwrap();

        assert_eq!(value["target_type"], "movie");
        assert_eq!(value["tmdb_id"], 11);
    }
}
