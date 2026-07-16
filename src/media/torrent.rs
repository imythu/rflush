use sha1::{Digest, Sha1};

const MAX_BENCODE_DEPTH: usize = 128;

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
}

pub fn torrent_infohash(data: &[u8]) -> Result<String, TorrentMetadataError> {
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
        if key == b"info" {
            if info.replace(&data[value_start..cursor]).is_some() {
                return Err(TorrentMetadataError::DuplicateInfo);
            }
        }
    }
    cursor += 1;
    if cursor != data.len() {
        return Err(TorrentMetadataError::TrailingBytes);
    }
    let info = info.ok_or(TorrentMetadataError::MissingInfo)?;
    Ok(sha1_hex(info))
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
}
