use std::ffi::OsStr;
use std::path::Path;

use reqwest::header::{HeaderMap, CONTENT_DISPOSITION};

use crate::rss::TorrentItem;

pub fn sanitize_component(value: &str) -> String {
    let invalid = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let sanitized = value
        .chars()
        .map(|ch| {
            if invalid.contains(&ch) || ch.is_control() {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>();

    let trimmed = sanitized.trim_matches([' ', '.']);
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn guid_key(guid: &str) -> String {
    sanitize_component(guid)
}

pub fn extract_original_filename(headers: &HeaderMap) -> Option<String> {
    let header_value = headers.get(CONTENT_DISPOSITION)?.to_str().ok()?;
    let parts = header_value.split(';').map(str::trim);

    for part in parts {
        if let Some(value) = part.strip_prefix("filename*=") {
            let raw = value.split("''").nth(1).unwrap_or(value);
            return Some(raw.trim_matches('"').to_string());
        }

        if let Some(value) = part.strip_prefix("filename=") {
            return Some(value.trim_matches('"').to_string());
        }
    }

    None
}

pub fn extract_guid_key_from_file_name(file_name: &str) -> Option<String> {
    let path = Path::new(file_name);
    let extension = path.extension()?.to_string_lossy();
    if !extension.eq_ignore_ascii_case("torrent") {
        return None;
    }

    let stem = path.file_stem()?.to_string_lossy();
    let (_, guid_key) = stem.rsplit_once('-')?;
    if guid_key.is_empty() {
        return None;
    }

    Some(guid_key.to_string())
}

pub fn build_target_file_name(original_name: Option<&str>, item: &TorrentItem) -> String {
    match original_name
        .map(sanitize_component)
        .filter(|value| !value.is_empty())
    {
        Some(original_name) => append_guid_to_file_name(&original_name, &item.guid),
        None => format!("{}-{}.torrent", sanitize_component(&item.title), item.guid),
    }
}

fn append_guid_to_file_name(file_name: &str, guid: &str) -> String {
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .unwrap_or_else(|| OsStr::new("torrent"))
        .to_string_lossy();
    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_string());

    match extension {
        Some(extension) if !extension.is_empty() => {
            format!("{}-{}.{}", stem, sanitize_component(guid), extension)
        }
        _ => format!("{}-{}", stem, sanitize_component(guid)),
    }
}
