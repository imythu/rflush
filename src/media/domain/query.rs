use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::target::{MediaTarget, SeasonEpisode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SearchCriteria {
    Movie {
        titles: Vec<String>,
        year: Option<u32>,
    },
    Episode {
        titles: Vec<String>,
        season: u32,
        episode: u32,
    },
    Anime {
        titles: Vec<String>,
        absolute_episode: u32,
        season_episode: Option<SeasonEpisode>,
    },
    Season {
        titles: Vec<String>,
        season: u32,
    },
}

impl From<&MediaTarget> for SearchCriteria {
    fn from(target: &MediaTarget) -> Self {
        match target {
            MediaTarget::Movie { titles, year, .. } => Self::Movie {
                titles: titles.clone(),
                year: *year,
            },
            MediaTarget::Episode {
                titles,
                season,
                episode,
                ..
            } => Self::Episode {
                titles: titles.clone(),
                season: *season,
                episode: *episode,
            },
            MediaTarget::Anime {
                titles,
                absolute_episode,
                season_episode,
                ..
            } => Self::Anime {
                titles: titles.clone(),
                absolute_episode: *absolute_episode,
                season_episode: *season_episode,
            },
            MediaTarget::Season { titles, season, .. } => Self::Season {
                titles: titles.clone(),
                season: *season,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    pub tier: u8,
    pub source_title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryGenerator {
    pub max_queries: usize,
}

impl Default for QueryGenerator {
    fn default() -> Self {
        Self { max_queries: 8 }
    }
}

impl QueryGenerator {
    pub fn new(max_queries: usize) -> Self {
        Self { max_queries }
    }

    pub fn generate(&self, criteria: &SearchCriteria) -> Vec<SearchQuery> {
        if self.max_queries == 0 {
            return Vec::new();
        }

        let mut queries = Vec::new();
        let mut seen = HashSet::new();

        match criteria {
            SearchCriteria::Movie { titles, year } => {
                for title in clean_titles(titles) {
                    let query = year
                        .map(|year| format!("{title} {year}"))
                        .unwrap_or_else(|| title.clone());
                    push_query(&mut queries, &mut seen, self.max_queries, query, 1, &title);
                }
            }
            SearchCriteria::Episode {
                titles,
                season,
                episode,
            } => {
                for title in clean_titles(titles) {
                    push_query(
                        &mut queries,
                        &mut seen,
                        self.max_queries,
                        format!("{title} S{season:02}E{episode:02}"),
                        1,
                        &title,
                    );
                    push_query(
                        &mut queries,
                        &mut seen,
                        self.max_queries,
                        format!("{title} S{season}E{episode}"),
                        2,
                        &title,
                    );
                    push_query(
                        &mut queries,
                        &mut seen,
                        self.max_queries,
                        format!("{title} {episode:02}"),
                        3,
                        &title,
                    );
                }
            }
            SearchCriteria::Anime {
                titles,
                absolute_episode,
                season_episode,
            } => {
                for title in clean_titles(titles) {
                    push_query(
                        &mut queries,
                        &mut seen,
                        self.max_queries,
                        format!("{title} {absolute_episode:03}"),
                        1,
                        &title,
                    );
                    push_query(
                        &mut queries,
                        &mut seen,
                        self.max_queries,
                        format!("{title} {absolute_episode}"),
                        2,
                        &title,
                    );
                    if let Some(mapping) = season_episode {
                        push_query(
                            &mut queries,
                            &mut seen,
                            self.max_queries,
                            format!("{title} S{:02}E{:02}", mapping.season, mapping.episode),
                            3,
                            &title,
                        );
                    }
                }
            }
            SearchCriteria::Season { titles, season } => {
                for title in clean_titles(titles) {
                    push_query(
                        &mut queries,
                        &mut seen,
                        self.max_queries,
                        format!("{title} S{season:02}"),
                        1,
                        &title,
                    );
                    push_query(
                        &mut queries,
                        &mut seen,
                        self.max_queries,
                        format!("{title} Season {season}"),
                        2,
                        &title,
                    );
                    push_query(
                        &mut queries,
                        &mut seen,
                        self.max_queries,
                        format!("{title} 第{season}季"),
                        3,
                        &title,
                    );
                }
            }
        }

        queries
    }
}

fn clean_titles(titles: &[String]) -> Vec<String> {
    titles
        .iter()
        .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|title| !title.is_empty())
        .collect()
}

fn push_query(
    queries: &mut Vec<SearchQuery>,
    seen: &mut HashSet<String>,
    max_queries: usize,
    query: String,
    tier: u8,
    source_title: &str,
) {
    if queries.len() >= max_queries {
        return;
    }
    let dedupe_key = query.to_lowercase();
    if seen.insert(dedupe_key) {
        queries.push(SearchQuery {
            query,
            tier,
            source_title: source_title.to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(queries: &[SearchQuery]) -> Vec<&str> {
        queries.iter().map(|query| query.query.as_str()).collect()
    }

    #[test]
    fn generates_documented_episode_queries_in_tiers() {
        let criteria = SearchCriteria::Episode {
            titles: vec!["百日成王".into(), "Original Title".into()],
            season: 1,
            episode: 3,
        };
        let queries = QueryGenerator::default().generate(&criteria);

        assert_eq!(
            text(&queries),
            vec![
                "百日成王 S01E03",
                "百日成王 S1E3",
                "百日成王 03",
                "Original Title S01E03",
                "Original Title S1E3",
                "Original Title 03",
            ]
        );
        assert_eq!(queries[0].tier, 1);
        assert_eq!(queries[2].tier, 3);
    }

    #[test]
    fn generates_documented_movie_queries() {
        let criteria = SearchCriteria::Movie {
            titles: vec!["沙丘2".into(), "Dune Part Two".into()],
            year: Some(2024),
        };
        let queries = QueryGenerator::default().generate(&criteria);

        assert_eq!(text(&queries), vec!["沙丘2 2024", "Dune Part Two 2024"]);
    }

    #[test]
    fn deduplicates_case_and_whitespace_and_honors_limit() {
        let criteria = SearchCriteria::Movie {
            titles: vec![" Dune   Part Two ".into(), "dune part two".into()],
            year: Some(2024),
        };
        let queries = QueryGenerator::new(2).generate(&criteria);

        assert_eq!(text(&queries), vec!["Dune Part Two 2024"]);
    }

    #[test]
    fn zero_budget_produces_no_queries() {
        let criteria = SearchCriteria::Season {
            titles: vec!["Show".into()],
            season: 1,
        };
        assert!(QueryGenerator::new(0).generate(&criteria).is_empty());
    }

    #[test]
    fn anime_and_season_queries_keep_typed_numbering() {
        let anime = SearchCriteria::Anime {
            titles: vec!["One Piece".into()],
            absolute_episode: 7,
            season_episode: Some(SeasonEpisode {
                season: 1,
                episode: 7,
            }),
        };
        assert_eq!(
            text(&QueryGenerator::default().generate(&anime)),
            vec!["One Piece 007", "One Piece 7", "One Piece S01E07"]
        );

        let season = SearchCriteria::Season {
            titles: vec!["庆余年".into()],
            season: 2,
        };
        assert_eq!(
            text(&QueryGenerator::default().generate(&season)),
            vec!["庆余年 S02", "庆余年 Season 2", "庆余年 第2季"]
        );
    }
}
