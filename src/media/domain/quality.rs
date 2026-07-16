use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::release::ReleaseInfo;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QualityProfile {
    pub id: Option<i64>,
    pub name: String,
    /// Values are ordered from most preferred to least preferred.
    pub resolution_order: Vec<String>,
    pub allowed_resolutions: Vec<String>,
    pub blocked_resolutions: Vec<String>,
    /// Values are ordered from most preferred to least preferred.
    pub source_order: Vec<String>,
    pub allowed_sources: Vec<String>,
    /// Values are ordered from most preferred to least preferred.
    pub codec_order: Vec<String>,
    pub blocked_codecs: Vec<String>,
    pub allow_unknown_quality: bool,
    pub minimum_score: u32,
    pub min_seeders: u32,
}

impl Default for QualityProfile {
    fn default() -> Self {
        Self {
            id: None,
            name: "Default".to_owned(),
            resolution_order: vec!["2160p".into(), "1080p".into(), "720p".into(), "480p".into()],
            allowed_resolutions: vec!["2160p".into(), "1080p".into(), "720p".into(), "480p".into()],
            blocked_resolutions: Vec::new(),
            source_order: vec![
                "REMUX".into(),
                "BluRay".into(),
                "WEB-DL".into(),
                "WEBRip".into(),
                "HDTV".into(),
                "DVD".into(),
            ],
            // An empty allowed set means that a recognized value is unrestricted.
            allowed_sources: Vec::new(),
            codec_order: vec!["AV1".into(), "H265".into(), "H264".into()],
            blocked_codecs: Vec::new(),
            allow_unknown_quality: false,
            minimum_score: 80,
            min_seeders: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityRejection {
    QualityNotAllowed,
    UnknownQuality,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityAssessment {
    pub allowed: bool,
    pub rejection: Option<QualityRejection>,
    pub rank: u64,
    pub score: u32,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QualityProfileError {
    #[error("minimum_score must be between 0 and 100")]
    InvalidMinimumScore,
    #[error("{field} contains duplicate value '{value}'")]
    DuplicateValue { field: &'static str, value: String },
    #[error("resolution '{0}' is both allowed and blocked")]
    ConflictingResolution(String),
}

impl QualityProfile {
    pub fn validate(&self) -> Result<(), QualityProfileError> {
        if self.minimum_score > 100 {
            return Err(QualityProfileError::InvalidMinimumScore);
        }

        validate_unique("resolution_order", &self.resolution_order, quality_key)?;
        validate_unique(
            "allowed_resolutions",
            &self.allowed_resolutions,
            quality_key,
        )?;
        validate_unique(
            "blocked_resolutions",
            &self.blocked_resolutions,
            quality_key,
        )?;
        validate_unique("source_order", &self.source_order, quality_key)?;
        validate_unique("allowed_sources", &self.allowed_sources, quality_key)?;
        validate_unique("codec_order", &self.codec_order, quality_key)?;
        validate_unique("blocked_codecs", &self.blocked_codecs, quality_key)?;

        let allowed: HashSet<_> = self
            .allowed_resolutions
            .iter()
            .map(|value| quality_key(value))
            .collect();
        if let Some(value) = self
            .blocked_resolutions
            .iter()
            .find(|value| allowed.contains(&quality_key(value)))
        {
            return Err(QualityProfileError::ConflictingResolution(value.clone()));
        }

        Ok(())
    }

    pub fn assess(&self, release: &ReleaseInfo) -> QualityAssessment {
        let resolution = release.resolution.as_deref();
        let source = release.source.as_deref();
        let codec = release.codec.as_deref();
        let mut required_field_is_unknown = false;

        if contains(&self.blocked_resolutions, resolution) {
            return rejected(format!(
                "resolution {} is blocked by the quality profile",
                resolution.unwrap_or("unknown")
            ));
        }
        if contains(&self.blocked_codecs, codec) {
            return rejected(format!(
                "codec {} is blocked by the quality profile",
                codec.unwrap_or("unknown")
            ));
        }
        if !self.allowed_resolutions.is_empty() {
            match resolution {
                Some(value) if contains(&self.allowed_resolutions, Some(value)) => {}
                Some(value) => {
                    return rejected(format!(
                        "resolution {value} is not in the allowed resolution set"
                    ));
                }
                None => required_field_is_unknown = true,
            }
        }
        if !self.allowed_sources.is_empty() {
            match source {
                Some(value) if contains(&self.allowed_sources, Some(value)) => {}
                Some(value) => {
                    return rejected(format!("source {value} is not in the allowed source set"));
                }
                None => required_field_is_unknown = true,
            }
        }
        if required_field_is_unknown || (resolution.is_none() && source.is_none()) {
            return self.unknown_or_allowed(release);
        }

        accepted_assessment(self, resolution, source, codec, "quality is allowed")
    }

    pub fn quality_rank(&self, release: &ReleaseInfo) -> u64 {
        let resolution = preference_rank(&self.resolution_order, release.resolution.as_deref());
        let source = preference_rank(&self.source_order, release.source.as_deref());
        let codec = preference_rank(&self.codec_order, release.codec.as_deref());

        combined_rank(resolution, source, codec)
    }

    fn unknown_or_allowed(&self, release: &ReleaseInfo) -> QualityAssessment {
        if self.allow_unknown_quality {
            accepted_assessment(
                self,
                release.resolution.as_deref(),
                release.source.as_deref(),
                release.codec.as_deref(),
                "unknown quality is allowed by the profile",
            )
        } else {
            QualityAssessment {
                allowed: false,
                rejection: Some(QualityRejection::UnknownQuality),
                rank: 0,
                score: 0,
                explanation: "release quality is incomplete or unknown".to_owned(),
            }
        }
    }
}

fn accepted_assessment(
    profile: &QualityProfile,
    resolution: Option<&str>,
    source: Option<&str>,
    codec: Option<&str>,
    explanation: &str,
) -> QualityAssessment {
    let components = [
        preference_score(&profile.resolution_order, resolution),
        preference_score(&profile.source_order, source),
        preference_score(&profile.codec_order, codec),
    ];
    let known: Vec<u32> = components.into_iter().flatten().collect();
    let score = if known.is_empty() {
        0
    } else {
        known.iter().sum::<u32>() / known.len() as u32
    };

    QualityAssessment {
        allowed: true,
        rejection: None,
        rank: combined_rank(
            preference_rank(&profile.resolution_order, resolution),
            preference_rank(&profile.source_order, source),
            preference_rank(&profile.codec_order, codec),
        ),
        score,
        explanation: explanation.to_owned(),
    }
}

fn rejected(explanation: String) -> QualityAssessment {
    QualityAssessment {
        allowed: false,
        rejection: Some(QualityRejection::QualityNotAllowed),
        rank: 0,
        score: 0,
        explanation,
    }
}

fn contains(values: &[String], candidate: Option<&str>) -> bool {
    let Some(candidate) = candidate else {
        return false;
    };
    let candidate = quality_key(candidate);
    values.iter().any(|value| quality_key(value) == candidate)
}

fn preference_rank(order: &[String], candidate: Option<&str>) -> u64 {
    let Some(candidate) = candidate else {
        return 0;
    };
    let candidate = quality_key(candidate);
    order
        .iter()
        .position(|value| quality_key(value) == candidate)
        .map(|index| (order.len() - index) as u64)
        .unwrap_or(0)
}

fn combined_rank(resolution: u64, source: u64, codec: u64) -> u64 {
    const COMPONENT_MASK: u64 = (1 << 21) - 1;
    (resolution.min(COMPONENT_MASK) << 42)
        | (source.min(COMPONENT_MASK) << 21)
        | codec.min(COMPONENT_MASK)
}

fn preference_score(order: &[String], candidate: Option<&str>) -> Option<u32> {
    if order.is_empty() {
        return None;
    }
    candidate?;
    let rank = preference_rank(order, candidate);
    Some(((rank * 10 + order.len() as u64 - 1) / order.len() as u64) as u32)
}

fn validate_unique(
    field: &'static str,
    values: &[String],
    normalize: fn(&str) -> String,
) -> Result<(), QualityProfileError> {
    let mut seen = HashSet::new();
    for value in values {
        if !seen.insert(normalize(value)) {
            return Err(QualityProfileError::DuplicateValue {
                field,
                value: value.clone(),
            });
        }
    }
    Ok(())
}

fn quality_key(value: &str) -> String {
    let compact: String = value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();

    match compact.as_str() {
        "4k" | "uhd" | "uhd4k" | "2160" | "2160p" => "2160p".into(),
        "2k" | "1440" | "1440p" => "1440p".into(),
        "1080" | "1080p" | "1080i" => "1080p".into(),
        "720" | "720p" | "720i" => "720p".into(),
        "576" | "576p" | "576i" => "576p".into(),
        "480" | "480p" | "480i" => "480p".into(),
        "webdl" => "webdl".into(),
        "web" | "webrip" => "webrip".into(),
        "bluray" | "bdrip" | "brrip" | "bdmv" => "bluray".into(),
        "remux" => "remux".into(),
        "hevc" | "h265" | "x265" => "h265".into(),
        "avc" | "h264" | "x264" => "h264".into(),
        "av01" | "av1" => "av1".into(),
        _ => compact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(resolution: Option<&str>, source: Option<&str>, codec: Option<&str>) -> ReleaseInfo {
        ReleaseInfo {
            raw_title: "Example".into(),
            title: "Example".into(),
            alternate_titles: Vec::new(),
            year: None,
            season: None,
            episodes: Vec::new(),
            absolute_episodes: Vec::new(),
            full_season: false,
            resolution: resolution.map(str::to_owned),
            codec: codec.map(str::to_owned),
            source: source.map(str::to_owned),
            revision: None,
            release_group: None,
            matched_rule: "quality_only".into(),
        }
    }

    #[test]
    fn aliases_compare_using_canonical_quality_values() {
        let profile = QualityProfile::default();
        let assessment = profile.assess(&release(Some("4K"), Some("WEB.DL"), Some("HEVC")));

        assert!(assessment.allowed);
        assert!(assessment.rank > 0);
    }

    #[test]
    fn blocked_codec_is_a_hard_quality_rejection() {
        let profile = QualityProfile {
            blocked_codecs: vec!["HEVC".into()],
            ..QualityProfile::default()
        };
        let assessment = profile.assess(&release(Some("1080p"), None, Some("H265")));

        assert_eq!(
            assessment.rejection,
            Some(QualityRejection::QualityNotAllowed)
        );
    }

    #[test]
    fn unknown_quality_obeys_profile_flag() {
        let unknown = release(None, None, None);
        let denied = QualityProfile::default().assess(&unknown);
        assert_eq!(denied.rejection, Some(QualityRejection::UnknownQuality));

        let allowed = QualityProfile {
            allow_unknown_quality: true,
            ..QualityProfile::default()
        }
        .assess(&unknown);
        assert!(allowed.allowed);
        assert_eq!(allowed.score, 0);
    }

    #[test]
    fn allowing_unknown_does_not_bypass_a_known_disallowed_field() {
        let profile = QualityProfile {
            allowed_sources: vec!["WEB-DL".into()],
            allow_unknown_quality: true,
            ..QualityProfile::default()
        };
        let assessment = profile.assess(&release(None, Some("CAM"), None));

        assert_eq!(
            assessment.rejection,
            Some(QualityRejection::QualityNotAllowed)
        );
    }

    #[test]
    fn rank_follows_user_order_not_enum_order() {
        let profile = QualityProfile {
            resolution_order: vec!["720p".into(), "2160p".into(), "1080p".into()],
            ..QualityProfile::default()
        };

        assert!(
            profile.quality_rank(&release(Some("720p"), None, None))
                > profile.quality_rank(&release(Some("2160p"), None, None))
        );
    }

    #[test]
    fn profile_validation_rejects_alias_duplicates_and_conflicts() {
        let duplicate = QualityProfile {
            codec_order: vec!["H265".into(), "HEVC".into()],
            ..QualityProfile::default()
        };
        assert!(matches!(
            duplicate.validate(),
            Err(QualityProfileError::DuplicateValue {
                field: "codec_order",
                ..
            })
        ));

        let conflict = QualityProfile {
            blocked_resolutions: vec!["4K".into()],
            ..QualityProfile::default()
        };
        assert!(matches!(
            conflict.validate(),
            Err(QualityProfileError::ConflictingResolution(_))
        ));
    }
}
