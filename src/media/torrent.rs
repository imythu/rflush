use sha1::{Digest, Sha1};

const MAX_BENCODE_DEPTH: usize = 128;
type BencodeEntries<'a> = Vec<(&'a [u8], &'a [u8])>;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TorrentMetadataError {
    #[error("torrent payload is not a bencoded dictionary")]
    InvalidRoot,
    #[error("invalid bencode at byte {0}")]
    InvalidBencode(usize),
    #[error("bencode nesting is too deep")]
    NestingTooDeep,
    #[error("torrent payload has no info dictionary")]
    MissingInfo,
    #[error("torrent payload contains duplicate info dictionaries")]
    DuplicateInfo,
    #[error("torrent payload has trailing bytes")]
    TrailingBytes,
    #[error("torrent info dictionary has no valid name")]
    MissingName,
    #[error("torrent file manifest is invalid or unsupported")]
    InvalidFiles,
    #[error("torrent file path is not valid UTF-8")]
    InvalidUtf8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentManifestFile {
    pub path: String,
    pub size: i64,
}

pub fn torrent_infohash(data: &[u8]) -> Result<String, TorrentMetadataError> {
    Ok(sha1_hex(info_dictionary(data)?))
}

pub fn torrent_file_manifest(
    data: &[u8],
) -> Result<Vec<TorrentManifestFile>, TorrentMetadataError> {
    let info = info_dictionary(data)?;
    let entries = dictionary_entries(info)?;
    let name = dictionary_bytes(&entries, b"name.utf-8")?
        .or(dictionary_bytes(&entries, b"name")?)
        .ok_or(TorrentMetadataError::MissingName)?;
    let name = std::str::from_utf8(name).map_err(|_| TorrentMetadataError::InvalidUtf8)?;
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\']) {
        return Err(TorrentMetadataError::MissingName);
    }

    if let Some(files) = dictionary_value(&entries, b"files")? {
        return parse_multi_file_manifest(name, files);
    }
    let length = dictionary_value(&entries, b"length")?
        .ok_or(TorrentMetadataError::InvalidFiles)
        .and_then(parse_nonnegative_integer)?;
    Ok(vec![TorrentManifestFile {
        path: name.to_string(),
        size: length,
    }])
}

fn info_dictionary(data: &[u8]) -> Result<&[u8], TorrentMetadataError> {
    if data.first() != Some(&b'd') {
        return Err(TorrentMetadataError::InvalidRoot);
    }
    let mut cursor = 1;
    let mut info = None;
    while data.get(cursor) != Some(&b'e') {
        let (key, next) = parse_bytes(data, cursor)?;
        cursor = next;
        let value_start = cursor;
        cursor = skip_value(data, cursor, 1)?;
        if key == b"info" && info.replace(&data[value_start..cursor]).is_some() {
            return Err(TorrentMetadataError::DuplicateInfo);
        }
    }
    cursor += 1;
    if cursor != data.len() {
        return Err(TorrentMetadataError::TrailingBytes);
    }
    let info = info.ok_or(TorrentMetadataError::MissingInfo)?;
    Ok(info)
}

fn dictionary_entries(data: &[u8]) -> Result<BencodeEntries<'_>, TorrentMetadataError> {
    if data.first() != Some(&b'd') {
        return Err(TorrentMetadataError::InvalidFiles);
    }
    let mut cursor = 1;
    let mut entries = Vec::new();
    while data.get(cursor) != Some(&b'e') {
        let (key, next) = parse_bytes(data, cursor)?;
        let value_start = next;
        cursor = skip_value(data, value_start, 1)?;
        entries.push((key, &data[value_start..cursor]));
    }
    if cursor + 1 != data.len() {
        return Err(TorrentMetadataError::InvalidFiles);
    }
    Ok(entries)
}

fn dictionary_value<'a>(
    entries: &[(&[u8], &'a [u8])],
    expected: &[u8],
) -> Result<Option<&'a [u8]>, TorrentMetadataError> {
    let mut found = None;
    for (key, value) in entries {
        if *key == expected && found.replace(*value).is_some() {
            return Err(TorrentMetadataError::InvalidFiles);
        }
    }
    Ok(found)
}

fn dictionary_bytes<'a>(
    entries: &[(&[u8], &'a [u8])],
    expected: &[u8],
) -> Result<Option<&'a [u8]>, TorrentMetadataError> {
    dictionary_value(entries, expected)?
        .map(|value| {
            let (bytes, end) = parse_bytes(value, 0)?;
            (end == value.len())
                .then_some(bytes)
                .ok_or(TorrentMetadataError::InvalidFiles)
        })
        .transpose()
}

fn parse_multi_file_manifest(
    root_name: &str,
    files: &[u8],
) -> Result<Vec<TorrentManifestFile>, TorrentMetadataError> {
    if files.first() != Some(&b'l') {
        return Err(TorrentMetadataError::InvalidFiles);
    }
    let mut cursor = 1;
    let mut manifest = Vec::new();
    while files.get(cursor) != Some(&b'e') {
        let end = skip_value(files, cursor, 1)?;
        let entries = dictionary_entries(&files[cursor..end])?;
        let size = dictionary_value(&entries, b"length")?
            .ok_or(TorrentMetadataError::InvalidFiles)
            .and_then(parse_nonnegative_integer)?;
        let path = dictionary_value(&entries, b"path.utf-8")?
            .or(dictionary_value(&entries, b"path")?)
            .ok_or(TorrentMetadataError::InvalidFiles)?;
        let components = parse_path_components(path)?;
        manifest.push(TorrentManifestFile {
            path: std::iter::once(root_name)
                .chain(components.iter().copied())
                .collect::<Vec<_>>()
                .join("/"),
            size,
        });
        cursor = end;
    }
    if cursor + 1 != files.len() || manifest.is_empty() {
        return Err(TorrentMetadataError::InvalidFiles);
    }
    Ok(manifest)
}

fn parse_path_components(path: &[u8]) -> Result<Vec<&str>, TorrentMetadataError> {
    if path.first() != Some(&b'l') {
        return Err(TorrentMetadataError::InvalidFiles);
    }
    let mut cursor = 1;
    let mut components = Vec::new();
    while path.get(cursor) != Some(&b'e') {
        let (component, next) = parse_bytes(path, cursor)?;
        let component =
            std::str::from_utf8(component).map_err(|_| TorrentMetadataError::InvalidUtf8)?;
        if component.is_empty()
            || matches!(component, "." | "..")
            || component.contains(['/', '\\'])
        {
            return Err(TorrentMetadataError::InvalidFiles);
        }
        components.push(component);
        cursor = next;
    }
    if cursor + 1 != path.len() || components.is_empty() {
        return Err(TorrentMetadataError::InvalidFiles);
    }
    Ok(components)
}

fn parse_nonnegative_integer(value: &[u8]) -> Result<i64, TorrentMetadataError> {
    if value.first() != Some(&b'i') || skip_integer(value, 0)? != value.len() {
        return Err(TorrentMetadataError::InvalidFiles);
    }
    let raw = std::str::from_utf8(&value[1..value.len() - 1])
        .map_err(|_| TorrentMetadataError::InvalidFiles)?;
    raw.parse::<i64>()
        .ok()
        .filter(|size| *size >= 0)
        .ok_or(TorrentMetadataError::InvalidFiles)
}

fn skip_value(data: &[u8], cursor: usize, depth: usize) -> Result<usize, TorrentMetadataError> {
    if depth > MAX_BENCODE_DEPTH {
        return Err(TorrentMetadataError::NestingTooDeep);
    }
    match data.get(cursor).copied() {
        Some(b'i') => skip_integer(data, cursor),
        Some(b'l') => {
            let mut next = cursor + 1;
            while data.get(next) != Some(&b'e') {
                next = skip_value(data, next, depth + 1)?;
            }
            Ok(next + 1)
        }
        Some(b'd') => {
            let mut next = cursor + 1;
            while data.get(next) != Some(&b'e') {
                let (_, after_key) = parse_bytes(data, next)?;
                next = skip_value(data, after_key, depth + 1)?;
            }
            Ok(next + 1)
        }
        Some(b'0'..=b'9') => parse_bytes(data, cursor).map(|(_, next)| next),
        _ => Err(TorrentMetadataError::InvalidBencode(cursor)),
    }
}

fn skip_integer(data: &[u8], cursor: usize) -> Result<usize, TorrentMetadataError> {
    let start = cursor + 1;
    let relative_end = data
        .get(start..)
        .and_then(|rest| rest.iter().position(|byte| *byte == b'e'))
        .ok_or(TorrentMetadataError::InvalidBencode(cursor))?;
    let end = start + relative_end;
    let raw = &data[start..end];
    if raw.is_empty()
        || raw == b"-0"
        || (raw.starts_with(b"0") && raw.len() > 1)
        || (raw.starts_with(b"-0"))
        || !raw
            .iter()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_digit() || (index == 0 && *byte == b'-'))
    {
        return Err(TorrentMetadataError::InvalidBencode(cursor));
    }
    Ok(end + 1)
}

fn parse_bytes(data: &[u8], cursor: usize) -> Result<(&[u8], usize), TorrentMetadataError> {
    let relative_colon = data
        .get(cursor..)
        .and_then(|rest| rest.iter().position(|byte| *byte == b':'))
        .ok_or(TorrentMetadataError::InvalidBencode(cursor))?;
    let colon = cursor + relative_colon;
    let length_bytes = &data[cursor..colon];
    if length_bytes.is_empty()
        || (length_bytes.starts_with(b"0") && length_bytes.len() > 1)
        || !length_bytes.iter().all(u8::is_ascii_digit)
    {
        return Err(TorrentMetadataError::InvalidBencode(cursor));
    }
    let length = std::str::from_utf8(length_bytes)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or(TorrentMetadataError::InvalidBencode(cursor))?;
    let start = colon + 1;
    let end = start
        .checked_add(length)
        .filter(|end| *end <= data.len())
        .ok_or(TorrentMetadataError::InvalidBencode(cursor))?;
    Ok((&data[start..end], end))
}

fn sha1_hex(data: &[u8]) -> String {
    let digest = Sha1::digest(data);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_the_raw_info_dictionary() {
        let data = b"d8:announce13:https://track4:infod4:name4:testee";
        let expected = sha1_hex(b"d4:name4:teste");
        assert_eq!(torrent_infohash(data).unwrap(), expected);
    }

    #[test]
    fn sha1_matches_the_standard_vector() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn rejects_missing_duplicate_or_trailing_info() {
        assert_eq!(
            torrent_infohash(b"d4:name4:teste"),
            Err(TorrentMetadataError::MissingInfo)
        );
        assert_eq!(
            torrent_infohash(b"d4:infode4:infodee"),
            Err(TorrentMetadataError::DuplicateInfo)
        );
        assert_eq!(
            torrent_infohash(b"d4:infodeejunk"),
            Err(TorrentMetadataError::TrailingBytes)
        );
    }

    #[test]
    fn extracts_single_and_multi_file_manifests() {
        assert_eq!(
            torrent_file_manifest(b"d4:infod6:lengthi12e4:name8:file.mkvee").unwrap(),
            vec![TorrentManifestFile {
                path: "file.mkv".to_string(),
                size: 12,
            }]
        );
        assert_eq!(
            torrent_file_manifest(
                b"d4:infod5:filesld6:lengthi10e4:pathl6:Season7:E01.mkveee4:name4:Showee"
            )
            .unwrap(),
            vec![TorrentManifestFile {
                path: "Show/Season/E01.mkv".to_string(),
                size: 10,
            }]
        );
    }
}
