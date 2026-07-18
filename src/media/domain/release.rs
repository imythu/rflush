use std::sync::OnceLock;

use chrono::{Datelike, Utc};
use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub raw_title: String,
    pub title: String,
    pub alternate_titles: Vec<String>,
    pub year: Option<u32>,
    pub season: Option<u32>,
    pub episodes: Vec<u32>,
    pub absolute_episodes: Vec<u32>,
    pub full_season: bool,
    pub resolution: Option<String>,
    pub codec: Option<String>,
    pub source: Option<String>,
    pub revision: Option<String>,
    pub release_group: Option<String>,
    pub matched_rule: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ReleaseParseError {
    #[error("release title is empty")]
    EmptyTitle,
    #[error("episode range is reversed: {start}-{end}")]
    ReversedEpisodeRange { start: u32, end: u32 },
    #[error("episode range {start}-{end} exceeds maximum expansion of {maximum}")]
    EpisodeRangeTooLarge { start: u32, end: u32, maximum: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseParser {
    pub max_episode_range: u32,
    pub latest_year: u32,
}

impl Default for ReleaseParser {
    fn default() -> Self {
        Self {
            max_episode_range: 50,
            latest_year: (Utc::now().year() + 1) as u32,
        }
    }
}

impl ReleaseParser {
    #[allow(dead_code)]
    pub fn new(max_episode_range: u32) -> Self {
        Self {
            max_episode_range,
            ..Self::default()
        }
    }

    #[allow(dead_code)]
    pub fn with_limits(max_episode_range: u32, latest_year: u32) -> Self {
        Self {
            max_episode_range,
            latest_year,
        }
    }

    pub fn parse(&self, raw_title: &str) -> Result<ReleaseInfo, ReleaseParseError> {
        let raw_title = raw_title.trim();
        if raw_title.is_empty() {
            return Err(ReleaseParseError::EmptyTitle);
        }

        let resolution = parse_resolution(raw_title);
        let codec = parse_codec(raw_title);
        let source = parse_source(raw_title);
        let mut revision = parse_revision(raw_title);
        let mut release_group = parse_release_group(raw_title);
        let parsed_year = self.parse_movie_year(raw_title);

        let mut season = None;
        let mut episodes = Vec::new();
        let mut absolute_episodes = Vec::new();
        let mut full_season = false;
        let year = parsed_year.map(|parsed| parsed.year);
        let matched_rule;
        let title;

        if let Some(parsed) = self.parse_s_episode(raw_title)? {
            season = Some(parsed.season);
            episodes = parsed.episodes;
            title = title_before(raw_title, parsed.marker_start);
            matched_rule = "standard_episode";
        } else if let Some(parsed) = self.parse_x_episode(raw_title)? {
            season = Some(parsed.season);
            episodes = parsed.episodes;
            title = title_before(raw_title, parsed.marker_start);
            matched_rule = "x_episode";
        } else if let Some(parsed) = self.parse_chinese_episode(raw_title)? {
            season = parsed.season;
            episodes = parsed.episodes;
            title = title_before(raw_title, parsed.marker_start);
            matched_rule = "chinese_episode";
        } else if let Some(parsed) = self.parse_anime_absolute(raw_title)? {
            absolute_episodes = parsed.episodes;
            title = clean_title(&parsed.title);
            if parsed.release_group.is_some() {
                release_group = parsed.release_group;
            }
            if parsed.revision.is_some() {
                revision = parsed.revision;
            }
            matched_rule = "anime_absolute";
        } else if let Some(parsed) = parse_season_pack(raw_title) {
            season = Some(parsed.season);
            full_season = true;
            title = title_before(raw_title, parsed.marker_start);
            matched_rule = "season_pack";
        } else if let Some(parsed) = parsed_year {
            title = title_before(raw_title, parsed.marker_start);
            matched_rule = "movie";
        } else {
            let quality_start = first_quality_marker(raw_title).unwrap_or(raw_title.len());
            title = title_before(raw_title, quality_start);
            matched_rule = if resolution.is_some() || codec.is_some() || source.is_some() {
                "quality_only"
            } else {
                "unknown"
            };
        }

        let title = if title.is_empty() {
            clean_title(raw_title)
        } else {
            title
        };

        Ok(ReleaseInfo {
            raw_title: raw_title.to_owned(),
            title,
            alternate_titles: Vec::new(),
            year,
            season,
            episodes,
            absolute_episodes,
            full_season,
            resolution,
            codec,
            source,
            revision,
            release_group,
            matched_rule: matched_rule.to_owned(),
        })
    }

    fn parse_s_episode(&self, title: &str) -> Result<Option<NumberedEpisode>, ReleaseParseError> {
        if let Some(captures) = s_range_regex().captures(title) {
            return self.numbered_range(&captures, "season", "start", "end");
        }
        if let Some(captures) = s_multi_regex().captures(title) {
            let season = capture_number(&captures, "season");
            let mut episodes = vec![capture_number(&captures, "start")];
            let rest = captures.name("rest").expect("rest capture").as_str();
            episodes.extend(e_number_regex().captures_iter(rest).map(|capture| {
                capture[1]
                    .parse::<u32>()
                    .expect("regex only captures digits")
            }));
            episodes.sort_unstable();
            episodes.dedup();
            return Ok(Some(NumberedEpisode {
                season,
                episodes,
                marker_start: season_marker_start(&captures),
            }));
        }
        Ok(s_single_regex()
            .captures(title)
            .map(|captures| NumberedEpisode {
                season: capture_number(&captures, "season"),
                episodes: vec![capture_number(&captures, "start")],
                marker_start: season_marker_start(&captures),
            }))
    }

    fn parse_x_episode(&self, title: &str) -> Result<Option<NumberedEpisode>, ReleaseParseError> {
        if let Some(captures) = x_range_regex().captures(title) {
            return self.numbered_range(&captures, "season", "start", "end");
        }
        Ok(x_single_regex()
            .captures(title)
            .map(|captures| NumberedEpisode {
                season: capture_number(&captures, "season"),
                episodes: vec![capture_number(&captures, "start")],
                marker_start: captures.name("season").expect("season capture").start(),
            }))
    }

    fn numbered_range(
        &self,
        captures: &Captures<'_>,
        season_name: &str,
        start_name: &str,
        end_name: &str,
    ) -> Result<Option<NumberedEpisode>, ReleaseParseError> {
        let start = capture_number(captures, start_name);
        let end = capture_number(captures, end_name);
        let episodes = self.expand_range(start, end)?;
        Ok(Some(NumberedEpisode {
            season: capture_number(captures, season_name),
            episodes,
            marker_start: captures
                .name(season_name)
                .expect("season capture")
                .start()
                .saturating_sub(1),
        }))
    }

    fn parse_chinese_episode(
        &self,
        title: &str,
    ) -> Result<Option<ChineseEpisode>, ReleaseParseError> {
        let season_capture = chinese_season_regex().captures(title);
        let season = season_capture
            .as_ref()
            .and_then(|captures| parse_chinese_number(&captures["number"]));

        if let Some(captures) = chinese_episode_range_regex().captures(title) {
            let Some(start) = parse_chinese_number(&captures["start"]) else {
                return Ok(None);
            };
            let Some(end) = parse_chinese_number(&captures["end"]) else {
                return Ok(None);
            };
            let marker_start = season_capture
                .as_ref()
                .map(|capture| capture.get(0).expect("whole capture").start())
                .unwrap_or_else(|| captures.get(0).expect("whole capture").start());
            return Ok(Some(ChineseEpisode {
                season,
                episodes: self.expand_range(start, end)?,
                marker_start,
            }));
        }

        Ok(chinese_episode_single_regex()
            .captures(title)
            .and_then(|captures| {
                let episode = parse_chinese_number(&captures["number"])?;
                let marker_start = season_capture
                    .as_ref()
                    .map(|capture| capture.get(0).expect("whole capture").start())
                    .unwrap_or_else(|| captures.get(0).expect("whole capture").start());
                Some(ChineseEpisode {
                    season,
                    episodes: vec![episode],
                    marker_start,
                })
            }))
    }

    fn parse_anime_absolute(&self, title: &str) -> Result<Option<AnimeEpisode>, ReleaseParseError> {
        let captures = anime_dash_regex()
            .captures(title)
            .or_else(|| anime_bracket_regex().captures(title));
        let Some(captures) = captures else {
            return Ok(None);
        };

        let start = capture_number(&captures, "start");
        let end = captures
            .name("end")
            .map(|value| value.as_str().parse().expect("regex only captures digits"));

        // A bare current-year-shaped number after a release group is more likely a movie year.
        if end.is_none() && captures.name("revision").is_none() && self.is_plausible_year(start) {
            return Ok(None);
        }

        let episodes = match end {
            Some(end) => self.expand_range(start, end)?,
            None => vec![start],
        };
        Ok(Some(AnimeEpisode {
            title: captures["title"].to_owned(),
            episodes,
            release_group: Some(captures["group"].trim().to_owned()),
            revision: captures
                .name("revision")
                .map(|revision| format!("v{}", revision.as_str())),
        }))
    }

    fn parse_movie_year(&self, title: &str) -> Option<MovieYear> {
        year_regex()
            .captures_iter(title)
            .filter_map(|captures| {
                let value = captures.name("year")?;
                let year = value.as_str().parse().ok()?;
                if !self.is_plausible_year(year) {
                    return None;
                }
                // Numeric movie names such as "1917" and "2001" are not release years
                // when the number is the first meaningful title token.
                if clean_title(&title[..value.start()]).is_empty() {
                    return None;
                }
                Some(MovieYear {
                    year,
                    marker_start: value.start(),
                })
            })
            .last()
    }

    fn is_plausible_year(&self, value: u32) -> bool {
        (1900..=self.latest_year).contains(&value)
    }

    fn expand_range(&self, start: u32, end: u32) -> Result<Vec<u32>, ReleaseParseError> {
        if end < start {
            return Err(ReleaseParseError::ReversedEpisodeRange { start, end });
        }
        let count = end - start + 1;
        if count > self.max_episode_range {
            return Err(ReleaseParseError::EpisodeRangeTooLarge {
                start,
                end,
                maximum: self.max_episode_range,
            });
        }
        Ok((start..=end).collect())
    }
}

struct NumberedEpisode {
    season: u32,
    episodes: Vec<u32>,
    marker_start: usize,
}

struct ChineseEpisode {
    season: Option<u32>,
    episodes: Vec<u32>,
    marker_start: usize,
}

struct AnimeEpisode {
    title: String,
    episodes: Vec<u32>,
    release_group: Option<String>,
    revision: Option<String>,
}

struct SeasonPack {
    season: u32,
    marker_start: usize,
}

#[derive(Clone, Copy)]
struct MovieYear {
    year: u32,
    marker_start: usize,
}

fn parse_season_pack(title: &str) -> Option<SeasonPack> {
    if let Some(captures) = s_season_regex().captures(title) {
        return Some(SeasonPack {
            season: capture_number(&captures, "season"),
            marker_start: season_marker_start(&captures),
        });
    }
    if let Some(captures) = english_season_regex().captures(title) {
        return Some(SeasonPack {
            season: capture_number(&captures, "season"),
            marker_start: captures.get(0).expect("whole capture").start(),
        });
    }
    chinese_season_regex().captures(title).and_then(|captures| {
        Some(SeasonPack {
            season: parse_chinese_number(&captures["number"])?,
            marker_start: captures.get(0).expect("whole capture").start(),
        })
    })
}

fn capture_number(captures: &Captures<'_>, name: &str) -> u32 {
    captures[name]
        .parse()
        .expect("numeric regex capture must parse")
}

fn season_marker_start(captures: &Captures<'_>) -> usize {
    captures
        .name("season")
        .expect("season capture")
        .start()
        .saturating_sub(1)
}

fn title_before(title: &str, marker_start: usize) -> String {
    clean_title(&title[..marker_start])
}

fn clean_title(title: &str) -> String {
    let mut title = title.trim();
    while let Some(captures) = leading_group_regex().captures(title) {
        let whole = captures.get(0).expect("whole capture");
        if whole.start() != 0 {
            break;
        }
        title = title[whole.end()..].trim_start();
    }

    let replaced: String = title
        .chars()
        .map(|character| match character {
            '.' | '_' => ' ',
            _ => character,
        })
        .collect();
    replaced
        .trim_matches(|character: char| {
            character.is_whitespace() || matches!(character, '-' | '[' | ']' | '(' | ')')
        })
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_chinese_number(value: &str) -> Option<u32> {
    if value.chars().all(|character| character.is_ascii_digit()) {
        return value.parse().ok();
    }

    let mut result = 0_u32;
    let mut digit = 0_u32;
    for character in value.chars() {
        match character {
            '零' | '〇' => digit = 0,
            '一' => digit = 1,
            '二' | '两' => digit = 2,
            '三' => digit = 3,
            '四' => digit = 4,
            '五' => digit = 5,
            '六' => digit = 6,
            '七' => digit = 7,
            '八' => digit = 8,
            '九' => digit = 9,
            '十' => {
                result += digit.max(1) * 10;
                digit = 0;
            }
            '百' => {
                result += digit.max(1) * 100;
                digit = 0;
            }
            '千' => {
                result += digit.max(1) * 1_000;
                digit = 0;
            }
            _ => return None,
        }
    }
    Some(result + digit)
}

fn parse_resolution(title: &str) -> Option<String> {
    resolution_regex().captures(title).map(|captures| {
        let compact = captures["value"].to_ascii_lowercase();
        match compact.as_str() {
            "8k" | "4320p" => "4320p",
            "4k" | "uhd" | "2160" | "2160p" | "2160i" => "2160p",
            "2k" | "1440p" => "1440p",
            "1080p" | "1080i" => "1080p",
            "720p" => "720p",
            "576p" | "576i" => "576p",
            "480p" | "480i" => "480p",
            _ => unreachable!("resolution regex and mapping must stay aligned"),
        }
        .to_owned()
    })
}

fn parse_codec(title: &str) -> Option<String> {
    if av1_regex().is_match(title) {
        Some("AV1".into())
    } else if h265_regex().is_match(title) {
        Some("H265".into())
    } else if h264_regex().is_match(title) {
        Some("H264".into())
    } else if xvid_regex().is_match(title) {
        Some("XviD".into())
    } else if mpeg2_regex().is_match(title) {
        Some("MPEG2".into())
    } else {
        None
    }
}

fn parse_source(title: &str) -> Option<String> {
    if remux_regex().is_match(title) {
        Some("REMUX".into())
    } else if web_dl_regex().is_match(title) {
        Some("WEB-DL".into())
    } else if web_rip_regex().is_match(title) {
        Some("WEBRip".into())
    } else if bluray_regex().is_match(title) {
        Some("BluRay".into())
    } else if hdtv_regex().is_match(title) {
        Some("HDTV".into())
    } else if dvd_regex().is_match(title) {
        Some("DVD".into())
    } else if cam_regex().is_match(title) {
        Some("CAM".into())
    } else {
        None
    }
}

fn parse_revision(title: &str) -> Option<String> {
    if let Some(captures) = attached_revision_regex().captures(title) {
        return Some(captures["value"].to_ascii_lowercase());
    }
    revision_regex()
        .captures_iter(title)
        .find(|captures| {
            let start = captures.name("value").expect("value capture").start();
            !clean_title(&title[..start]).is_empty()
        })
        .map(|captures| {
            let value = captures["value"].to_owned();
            if value.to_ascii_lowercase().starts_with('v') {
                value.to_ascii_lowercase()
            } else {
                value.to_ascii_uppercase()
            }
        })
}

fn parse_release_group(title: &str) -> Option<String> {
    if let Some(captures) = leading_group_regex().captures(title) {
        return Some(captures["group"].trim().to_owned());
    }
    let captures = trailing_group_regex().captures(title)?;
    let group = captures["group"].to_owned();
    let key = group.to_ascii_lowercase();
    if matches!(
        key.as_str(),
        "dl" | "rip" | "remux" | "proper" | "repack" | "h264" | "h265" | "x264" | "x265"
    ) {
        None
    } else {
        Some(group)
    }
}

fn first_quality_marker(title: &str) -> Option<usize> {
    let quality_marker = [
        resolution_regex(),
        av1_regex(),
        h265_regex(),
        h264_regex(),
        xvid_regex(),
        mpeg2_regex(),
        remux_regex(),
        web_dl_regex(),
        web_rip_regex(),
        bluray_regex(),
        hdtv_regex(),
        dvd_regex(),
        cam_regex(),
        attached_revision_regex(),
    ]
    .into_iter()
    .filter_map(|regex| {
        regex
            .find(title)
            .map(|found| marker_start(title, found.start()))
    })
    .min();
    let revision_marker = revision_regex().captures_iter(title).find_map(|captures| {
        let start = captures.name("value").expect("value capture").start();
        (!clean_title(&title[..start]).is_empty()).then_some(start)
    });

    quality_marker.into_iter().chain(revision_marker).min()
}

fn marker_start(title: &str, start: usize) -> usize {
    let Some(character) = title[start..].chars().next() else {
        return start;
    };
    if character.is_alphanumeric() {
        start
    } else {
        start + character.len_utf8()
    }
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("release parser regex must compile"))
}

macro_rules! regex_fn {
    ($name:ident, $pattern:literal) => {
        fn $name() -> &'static Regex {
            static CELL: OnceLock<Regex> = OnceLock::new();
            regex(&CELL, $pattern)
        }
    };
}

regex_fn!(
    s_range_regex,
    r"(?i)(?:^|[^a-z0-9])s(?P<season>\d{1,2})[ ._-]*e(?P<start>\d{1,4})[ ._]*(?:-|~|to)[ ._]*(?:e)?(?P<end>\d{1,4})(?:v[2-9]\d*)?(?:[^a-z0-9]|$)"
);
regex_fn!(
    s_multi_regex,
    r"(?i)(?:^|[^a-z0-9])s(?P<season>\d{1,2})[ ._-]*e(?P<start>\d{1,4})(?P<rest>(?:[ ._+&-]*e\d{1,4})+)(?:v[2-9]\d*)?(?:[^a-z0-9]|$)"
);
regex_fn!(
    s_single_regex,
    r"(?i)(?:^|[^a-z0-9])s(?P<season>\d{1,2})[ ._-]*e(?P<start>\d{1,4})(?:v[2-9]\d*)?(?:[^a-z0-9]|$)"
);
regex_fn!(e_number_regex, r"(?i)e(\d{1,4})");
regex_fn!(
    x_range_regex,
    r"(?i)(?:^|[^a-z0-9])(?P<season>\d{1,2})x(?P<start>\d{2,4})[ ._]*(?:-|~|to)[ ._]*(?P<end>\d{2,4})(?:v[2-9]\d*)?(?:[^a-z0-9]|$)"
);
regex_fn!(
    x_single_regex,
    r"(?i)(?:^|[^a-z0-9])(?P<season>\d{1,2})x(?P<start>\d{2,4})(?:v[2-9]\d*)?(?:[^a-z0-9]|$)"
);
regex_fn!(
    chinese_season_regex,
    r"第\s*(?P<number>[0-9零〇一二两三四五六七八九十百千]+)\s*季"
);
regex_fn!(
    chinese_episode_range_regex,
    r"第\s*(?P<start>[0-9零〇一二两三四五六七八九十百千]+)\s*(?:[集话]\s*)?(?:-|~|至|到)\s*(?:第\s*)?(?P<end>[0-9零〇一二两三四五六七八九十百千]+)\s*[集话]"
);
regex_fn!(
    chinese_episode_single_regex,
    r"第\s*(?P<number>[0-9零〇一二两三四五六七八九十百千]+)\s*[集话]"
);
regex_fn!(
    anime_dash_regex,
    r"(?i)^\s*\[(?P<group>[^\]\r\n]{1,80})\]\s*(?P<title>.+?)\s+-\s+(?P<start>\d{1,4})(?:\s*(?:-|~)\s*(?P<end>\d{1,4}))?(?:v(?P<revision>\d+))?(?:\s|[\[(.]|$)"
);
regex_fn!(
    anime_bracket_regex,
    r"(?i)^\s*\[(?P<group>[^\]\r\n]{1,80})\]\s*(?P<title>.+?)\s+\[(?P<start>\d{1,4})(?:\s*(?:-|~)\s*(?P<end>\d{1,4}))?(?:v(?P<revision>\d+))?\](?:\s|$)"
);
regex_fn!(
    s_season_regex,
    r"(?i)(?:^|[^a-z0-9])s(?P<season>\d{1,2})(?:[^a-z0-9]|$)"
);
regex_fn!(
    english_season_regex,
    r"(?i)(?:^|[^a-z0-9])season[ ._-]*(?P<season>\d{1,2})(?:[^a-z0-9]|$)"
);
regex_fn!(
    year_regex,
    r"(?:^|[^0-9])(?P<year>(?:19|20|21)\d{2})(?:[^0-9]|$)"
);
regex_fn!(
    resolution_regex,
    r"(?i)(?:^|[^a-z0-9])(?P<value>4320p|8k|2160[pi]|4k|uhd|1440p|2k|1080[pi]|720p|576[pi]|480[pi])(?:[^a-z0-9]|$)"
);
regex_fn!(av1_regex, r"(?i)(?:^|[^a-z0-9])(?:av1|av01)(?:[^a-z0-9]|$)");
regex_fn!(
    h265_regex,
    r"(?i)(?:^|[^a-z0-9])(?:h[ ._-]*265|x265|hevc)(?:[^a-z0-9]|$)"
);
regex_fn!(
    h264_regex,
    r"(?i)(?:^|[^a-z0-9])(?:h[ ._-]*264|x264|avc)(?:[^a-z0-9]|$)"
);
regex_fn!(xvid_regex, r"(?i)(?:^|[^a-z0-9])xvid(?:[^a-z0-9]|$)");
regex_fn!(
    mpeg2_regex,
    r"(?i)(?:^|[^a-z0-9])mpeg[ ._-]*2(?:[^a-z0-9]|$)"
);
regex_fn!(remux_regex, r"(?i)(?:^|[^a-z0-9])remux(?:[^a-z0-9]|$)");
regex_fn!(
    web_dl_regex,
    r"(?i)(?:^|[^a-z0-9])web[ ._-]*dl(?:[^a-z0-9]|$)"
);
regex_fn!(
    web_rip_regex,
    r"(?i)(?:^|[^a-z0-9])web[ ._-]*rip(?:[^a-z0-9]|$)"
);
regex_fn!(
    bluray_regex,
    r"(?i)(?:^|[^a-z0-9])(?:blu[ ._-]*ray|bd[ ._-]*(?:rip|remux)|brrip|bdmv)(?:[^a-z0-9]|$)"
);
regex_fn!(hdtv_regex, r"(?i)(?:^|[^a-z0-9])hdtv(?:[^a-z0-9]|$)");
regex_fn!(
    dvd_regex,
    r"(?i)(?:^|[^a-z0-9])(?:dvd(?:rip)?|r5)(?:[^a-z0-9]|$)"
);
regex_fn!(
    cam_regex,
    r"(?i)(?:^|[^a-z0-9])(?:hdcam|cam|telesync|ts)(?:[^a-z0-9]|$)"
);
regex_fn!(
    revision_regex,
    r"(?i)(?:^|[^a-z0-9])(?P<value>repack\d*|proper|real|v[2-9]\d*)(?:[^a-z0-9]|$)"
);
regex_fn!(
    attached_revision_regex,
    r"(?i)(?:s\d{1,2}e\d{1,4}|\d{1,2}x\d{2,4}|\d{1,4})(?P<value>v[2-9]\d*)(?:[^a-z0-9]|$)"
);
regex_fn!(
    leading_group_regex,
    r"^\s*\[(?P<group>[^\]\r\n]{1,80})\]\s*"
);
regex_fn!(
    trailing_group_regex,
    r"-(?P<group>[A-Za-z][A-Za-z0-9]{1,20})$"
);

#[cfg(test)]
mod tests {
    use super::*;

    fn parser() -> ReleaseParser {
        ReleaseParser::with_limits(20, 2027)
    }

    #[test]
    fn parses_standard_episode_single_multi_and_ranges() {
        let single = parser()
            .parse("Example.Show.S01E02.1080p.WEB-DL.x265-GROUP")
            .unwrap();
        assert_eq!(single.title, "Example Show");
        assert_eq!(single.season, Some(1));
        assert_eq!(single.episodes, vec![2]);
        assert_eq!(single.release_group.as_deref(), Some("GROUP"));

        let multi = parser().parse("Example Show S01E02E03.E05").unwrap();
        assert_eq!(multi.episodes, vec![2, 3, 5]);

        let range = parser().parse("Example Show S01E02-E04").unwrap();
        assert_eq!(range.episodes, vec![2, 3, 4]);
    }

    #[test]
    fn parses_x_episode_but_not_3x3_eyes() {
        let episode = parser().parse("Example.Show.1x02.720p").unwrap();
        assert_eq!(episode.matched_rule, "x_episode");
        assert_eq!(episode.season, Some(1));
        assert_eq!(episode.episodes, vec![2]);

        let numeric_title = parser().parse("3x3 Eyes 1080p BluRay").unwrap();
        assert_eq!(numeric_title.matched_rule, "quality_only");
        assert_eq!(numeric_title.title, "3x3 Eyes");
        assert!(numeric_title.episodes.is_empty());
    }

    #[test]
    fn rejects_reversed_and_oversized_ranges_before_allocation() {
        assert!(matches!(
            parser().parse("Show S01E05-E03"),
            Err(ReleaseParseError::ReversedEpisodeRange { start: 5, end: 3 })
        ));
        assert!(matches!(
            parser().parse("Show S01E01-E99"),
            Err(ReleaseParseError::EpisodeRangeTooLarge { maximum: 20, .. })
        ));
    }

    #[test]
    fn parses_chinese_season_episode_and_spelled_numbers() {
        let release = parser().parse("庆余年 第2季 第3-5集 2160p WEB-DL").unwrap();
        assert_eq!(release.title, "庆余年");
        assert_eq!(release.season, Some(2));
        assert_eq!(release.episodes, vec![3, 4, 5]);

        let spelled = parser().parse("动画 第十二话 1080p").unwrap();
        assert_eq!(spelled.episodes, vec![12]);

        let repeated_markers = parser().parse("动画 第三话-第五话 1080p").unwrap();
        assert_eq!(repeated_markers.episodes, vec![3, 4, 5]);
    }

    #[test]
    fn parses_anime_absolute_episode_and_revision() {
        let release = parser()
            .parse("[Lilith-Raws] One Piece - 1122-1124v2 [Baha][WEB-DL][1080p][AVC]")
            .unwrap();

        assert_eq!(release.title, "One Piece");
        assert_eq!(release.absolute_episodes, vec![1122, 1123, 1124]);
        assert_eq!(release.release_group.as_deref(), Some("Lilith-Raws"));
        assert_eq!(release.revision.as_deref(), Some("v2"));
        assert_eq!(release.matched_rule, "anime_absolute");
    }

    #[test]
    fn parses_season_packs() {
        let western = parser().parse("Example Show S02 Complete 1080p").unwrap();
        assert_eq!(western.season, Some(2));
        assert!(western.full_season);

        let chinese = parser().parse("庆余年 第二季 全集 2160p").unwrap();
        assert_eq!(chinese.season, Some(2));
        assert!(chinese.full_season);
    }

    #[test]
    fn parses_movie_and_all_quality_fields() {
        let release = parser()
            .parse("Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.HEVC.PROPER-GROUP")
            .unwrap();

        assert_eq!(release.title, "Dune Part Two");
        assert_eq!(release.year, Some(2024));
        assert_eq!(release.resolution.as_deref(), Some("2160p"));
        assert_eq!(release.codec.as_deref(), Some("H265"));
        assert_eq!(release.source.as_deref(), Some("REMUX"));
        assert_eq!(release.revision.as_deref(), Some("PROPER"));
    }

    #[test]
    fn parses_revision_attached_to_episode_number() {
        let standard = parser()
            .parse("Example Show S01E02v2 1080p WEB-DL")
            .unwrap();
        assert_eq!(standard.episodes, vec![2]);
        assert_eq!(standard.revision.as_deref(), Some("v2"));

        let x_style = parser().parse("Example Show 1x02v3 720p").unwrap();
        assert_eq!(x_style.episodes, vec![2]);
        assert_eq!(x_style.revision.as_deref(), Some("v3"));
    }

    #[test]
    fn numeric_titles_are_not_episode_or_year_tokens() {
        let the_100 = parser().parse("The 100 1080p WEB-DL H264").unwrap();
        assert_eq!(the_100.title, "The 100");
        assert_eq!(the_100.year, None);
        assert!(the_100.episodes.is_empty());

        let blade_runner = parser()
            .parse("Blade.Runner.2049.1080p.BluRay.x265")
            .unwrap();
        assert_eq!(blade_runner.title, "Blade Runner 2049");
        assert_eq!(blade_runner.year, None);
        assert!(blade_runner.absolute_episodes.is_empty());

        let real_steel = parser().parse("Real Steel 1080p BluRay").unwrap();
        assert_eq!(real_steel.title, "Real Steel");
        assert_eq!(real_steel.revision, None);
    }

    #[test]
    fn a_numeric_movie_name_at_the_start_is_not_assumed_to_be_a_year() {
        let release = parser().parse("2001 A Space Odyssey 1080p BluRay").unwrap();
        assert_eq!(release.title, "2001 A Space Odyssey");
        assert_eq!(release.year, None);

        let with_year = parser()
            .parse("2001 A Space Odyssey 1968 1080p BluRay")
            .unwrap();
        assert_eq!(with_year.title, "2001 A Space Odyssey");
        assert_eq!(with_year.year, Some(1968));
    }

    #[test]
    fn release_info_round_trips_through_json() {
        let release = parser().parse("Show S01E01 1080p WEB-DL").unwrap();
        let json = serde_json::to_string(&release).unwrap();
        let restored: ReleaseInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, release);
    }
}
