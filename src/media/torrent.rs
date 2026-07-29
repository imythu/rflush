use sha1::{Digest, Sha1};
use sha2::Sha256;

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
    #[error("torrent infohash must be a 40 or 64 character hexadecimal string")]
    InvalidInfoHash,
    #[error("torrent metadata version is unsupported")]
    UnsupportedMetaVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentManifestFile {
    pub path: String,
    pub size: i64,
}

pub fn torrent_infohash(data: &[u8]) -> Result<String, TorrentMetadataError> {
    let info = info_dictionary(data)?;
    match info_meta_version(info)? {
        Some(2) => Ok(sha256_torrent_id_hex(info)),
        Some(_) => Err(TorrentMetadataError::UnsupportedMetaVersion),
        None => Ok(sha1_hex(info)),
    }
}

pub fn torrent_infohash_for(
    data: &[u8],
    expected_infohash: &str,
) -> Result<String, TorrentMetadataError> {
    if !expected_infohash
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(TorrentMetadataError::InvalidInfoHash);
    }
    let info = info_dictionary(data)?;
    let meta_version = info_meta_version(info)?;
    match expected_infohash.len() {
        40 => match meta_version {
            Some(2) => Ok(sha256_torrent_id_hex(info)),
            Some(_) => Err(TorrentMetadataError::UnsupportedMetaVersion),
            None => Ok(sha1_hex(info)),
        },
        64 => {
            if meta_version != Some(2) {
                return Err(TorrentMetadataError::UnsupportedMetaVersion);
            }
            Ok(sha256_hex(info))
        }
        _ => Err(TorrentMetadataError::InvalidInfoHash),
    }
}

fn info_meta_version(info: &[u8]) -> Result<Option<i64>, TorrentMetadataError> {
    let entries = dictionary_entries(info)?;
    dictionary_value(&entries, b"meta version")?
        .map(parse_nonnegative_integer)
        .transpose()
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

    let meta_version = dictionary_value(&entries, b"meta version")?
        .map(parse_nonnegative_integer)
        .transpose()?;
    if meta_version.is_some_and(|version| version != 2) {
        return Err(TorrentMetadataError::UnsupportedMetaVersion);
    }

    let v1_manifest = if let Some(files) = dictionary_value(&entries, b"files")? {
        Some(parse_multi_file_manifest(name, files)?)
    } else {
        dictionary_value(&entries, b"length")?
            .map(parse_nonnegative_integer)
            .transpose()?
            .map(|length| {
                vec![TorrentManifestFile {
                    path: name.to_string(),
                    size: length,
                }]
            })
    };
    if meta_version == Some(2) {
        let file_tree =
            dictionary_value(&entries, b"file tree")?.ok_or(TorrentMetadataError::InvalidFiles)?;
        let v2_manifest = parse_v2_file_tree(name, file_tree)?;
        if let Some(v1_manifest) = v1_manifest
            && v1_manifest != v2_manifest
        {
            return Err(TorrentMetadataError::InvalidFiles);
        }
        return Ok(v2_manifest);
    }
    v1_manifest.ok_or(TorrentMetadataError::InvalidFiles)
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

fn parse_v2_file_tree(
    root_name: &str,
    file_tree: &[u8],
) -> Result<Vec<TorrentManifestFile>, TorrentMetadataError> {
    let mut relative_manifest = Vec::new();
    walk_v2_file_tree(file_tree, &mut Vec::new(), &mut relative_manifest, 1)?;
    if relative_manifest.is_empty() {
        return Err(TorrentMetadataError::InvalidFiles);
    }

    let single_file_at_root =
        relative_manifest.len() == 1 && relative_manifest[0].path == root_name;
    if !single_file_at_root {
        for file in &mut relative_manifest {
            file.path = format!("{root_name}/{}", file.path);
        }
    }
    Ok(relative_manifest)
}

fn walk_v2_file_tree(
    tree: &[u8],
    components: &mut Vec<String>,
    manifest: &mut Vec<TorrentManifestFile>,
    depth: usize,
) -> Result<(), TorrentMetadataError> {
    if depth > MAX_BENCODE_DEPTH {
        return Err(TorrentMetadataError::NestingTooDeep);
    }
    let entries = dictionary_entries(tree)?;
    if entries.is_empty() {
        return Err(TorrentMetadataError::InvalidFiles);
    }
    let mut previous_key: Option<&[u8]> = None;
    for (key, _) in &entries {
        if previous_key.is_some_and(|previous| previous >= *key) {
            return Err(TorrentMetadataError::InvalidFiles);
        }
        previous_key = Some(key);
    }

    if let Some(properties) = dictionary_value(&entries, b"")? {
        if components.is_empty() || entries.len() != 1 {
            return Err(TorrentMetadataError::InvalidFiles);
        }
        let properties = dictionary_entries(properties)?;
        let length = dictionary_value(&properties, b"length")?
            .ok_or(TorrentMetadataError::InvalidFiles)
            .and_then(parse_nonnegative_integer)?;
        manifest.push(TorrentManifestFile {
            path: components.join("/"),
            size: length,
        });
        return Ok(());
    }

    for (raw_component, child) in entries {
        let component =
            std::str::from_utf8(raw_component).map_err(|_| TorrentMetadataError::InvalidUtf8)?;
        if component.is_empty()
            || matches!(component, "." | "..")
            || component.contains(['/', '\\', '\0'])
        {
            return Err(TorrentMetadataError::InvalidFiles);
        }
        components.push(component.to_string());
        walk_v2_file_tree(child, components, manifest, depth + 1)?;
        components.pop();
    }
    Ok(())
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
        let is_padding = dictionary_bytes(&entries, b"attr")?
            .is_some_and(|attributes| attributes.contains(&b'p'));
        if !is_padding {
            manifest.push(TorrentManifestFile {
                path: std::iter::once(root_name)
                    .chain(components.iter().copied())
                    .collect::<Vec<_>>()
                    .join("/"),
                size,
            });
        }
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

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_torrent_id_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest[..20]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bencoded_bytes(value: &[u8]) -> Vec<u8> {
        let mut encoded = format!("{}:", value.len()).into_bytes();
        encoded.extend_from_slice(value);
        encoded
    }

    fn bencoded_integer(value: i64) -> Vec<u8> {
        format!("i{value}e").into_bytes()
    }

    fn bencoded_dictionary(entries: Vec<(&[u8], Vec<u8>)>) -> Vec<u8> {
        let mut encoded = vec![b'd'];
        for (key, value) in entries {
            encoded.extend(bencoded_bytes(key));
            encoded.extend(value);
        }
        encoded.push(b'e');
        encoded
    }

    fn bencoded_list(values: Vec<Vec<u8>>) -> Vec<u8> {
        let mut encoded = vec![b'l'];
        for value in values {
            encoded.extend(value);
        }
        encoded.push(b'e');
        encoded
    }

    fn v2_single_torrent() -> Vec<u8> {
        let properties = bencoded_dictionary(vec![(b"length", bencoded_integer(12))]);
        let file = bencoded_dictionary(vec![(b"".as_slice(), properties)]);
        let tree = bencoded_dictionary(vec![(b"file.bin", file)]);
        let info = bencoded_dictionary(vec![
            (b"file tree", tree),
            (b"meta version", bencoded_integer(2)),
            (b"name", bencoded_bytes(b"file.bin")),
        ]);
        bencoded_dictionary(vec![(b"info", info)])
    }

    fn v2_multi_torrent() -> Vec<u8> {
        let file = |size| {
            bencoded_dictionary(vec![(
                b"".as_slice(),
                bencoded_dictionary(vec![(b"length", bencoded_integer(size))]),
            )])
        };
        let directory = bencoded_dictionary(vec![(b"a.mkv", file(10)), (b"b.mkv", file(20))]);
        let tree = bencoded_dictionary(vec![(b"dir", directory)]);
        let info = bencoded_dictionary(vec![
            (b"file tree", tree),
            (b"meta version", bencoded_integer(2)),
            (b"name", bencoded_bytes(b"Show")),
        ]);
        bencoded_dictionary(vec![(b"info", info)])
    }

    fn hybrid_torrent_with_padding_file() -> Vec<u8> {
        let path = |components: &[&[u8]]| {
            bencoded_list(
                components
                    .iter()
                    .map(|component| bencoded_bytes(component))
                    .collect(),
            )
        };
        let regular_file = |size, components: &[&[u8]]| {
            bencoded_dictionary(vec![
                (b"length", bencoded_integer(size)),
                (b"path", path(components)),
            ])
        };
        let padding_file = bencoded_dictionary(vec![
            (b"attr", bencoded_bytes(b"p")),
            (b"length", bencoded_integer(6)),
            (b"path", path(&[b".pad", b"6"])),
        ]);
        let files = bencoded_list(vec![
            regular_file(10, &[b"dir", b"a.mkv"]),
            padding_file,
            regular_file(20, &[b"dir", b"b.mkv"]),
        ]);

        let v2_file = |size| {
            bencoded_dictionary(vec![(
                b"".as_slice(),
                bencoded_dictionary(vec![(b"length", bencoded_integer(size))]),
            )])
        };
        let directory = bencoded_dictionary(vec![(b"a.mkv", v2_file(10)), (b"b.mkv", v2_file(20))]);
        let tree = bencoded_dictionary(vec![(b"dir", directory)]);
        let info = bencoded_dictionary(vec![
            (b"file tree", tree),
            (b"files", files),
            (b"meta version", bencoded_integer(2)),
            (b"name", bencoded_bytes(b"Show")),
        ]);
        bencoded_dictionary(vec![(b"info", info)])
    }

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
    fn sha256_matches_the_standard_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_torrent_id_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a3"
        );
    }

    #[test]
    fn hashes_v2_info_dictionary_with_sha256() {
        let data = v2_single_torrent();
        let info = info_dictionary(&data).unwrap();
        assert_eq!(
            torrent_infohash_for(&data, &"0".repeat(64)).unwrap(),
            sha256_hex(info)
        );
        assert_eq!(
            torrent_infohash_for(&data, &"0".repeat(40)).unwrap(),
            sha256_torrent_id_hex(info)
        );
        assert_eq!(
            torrent_infohash(&data).unwrap(),
            sha256_torrent_id_hex(info)
        );
        assert_eq!(
            torrent_infohash_for(&data, "not-a-hash"),
            Err(TorrentMetadataError::InvalidInfoHash)
        );
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

    #[test]
    fn extracts_v2_single_and_multi_file_manifests() {
        assert_eq!(
            torrent_file_manifest(&v2_single_torrent()).unwrap(),
            vec![TorrentManifestFile {
                path: "file.bin".to_string(),
                size: 12,
            }]
        );

        assert_eq!(
            torrent_file_manifest(&v2_multi_torrent()).unwrap(),
            vec![
                TorrentManifestFile {
                    path: "Show/dir/a.mkv".to_string(),
                    size: 10,
                },
                TorrentManifestFile {
                    path: "Show/dir/b.mkv".to_string(),
                    size: 20,
                },
            ]
        );
    }

    #[test]
    fn hybrid_manifest_ignores_bep47_padding_files() {
        let torrent = hybrid_torrent_with_padding_file();
        assert_eq!(
            torrent_file_manifest(&torrent).unwrap(),
            vec![
                TorrentManifestFile {
                    path: "Show/dir/a.mkv".to_string(),
                    size: 10,
                },
                TorrentManifestFile {
                    path: "Show/dir/b.mkv".to_string(),
                    size: 20,
                },
            ]
        );
        let torrent_id = torrent_infohash(&torrent).unwrap();
        assert_eq!(torrent_id.len(), 40);
        assert_eq!(
            torrent_infohash_for(&torrent, &torrent_id).unwrap(),
            torrent_id
        );
    }

    #[test]
    fn rejects_invalid_v2_file_tree_shapes() {
        let properties = bencoded_dictionary(vec![(b"length", bencoded_integer(12))]);
        let root_tree = bencoded_dictionary(vec![(b"".as_slice(), properties.clone())]);
        let root_info = bencoded_dictionary(vec![
            (b"file tree", root_tree),
            (b"meta version", bencoded_integer(2)),
            (b"name", bencoded_bytes(b"file.bin")),
        ]);
        let root_file = bencoded_dictionary(vec![(b"info", root_info)]);
        assert_eq!(
            torrent_file_manifest(&root_file),
            Err(TorrentMetadataError::InvalidFiles)
        );

        let nested_file = bencoded_dictionary(vec![(
            b"".as_slice(),
            bencoded_dictionary(vec![(b"length", bencoded_integer(1))]),
        )]);
        let invalid_node =
            bencoded_dictionary(vec![(b"".as_slice(), properties), (b"x", nested_file)]);
        let sibling_tree = bencoded_dictionary(vec![(b"file.bin", invalid_node)]);
        let sibling_info = bencoded_dictionary(vec![
            (b"file tree", sibling_tree),
            (b"meta version", bencoded_integer(2)),
            (b"name", bencoded_bytes(b"file.bin")),
        ]);
        let sibling_leaf = bencoded_dictionary(vec![(b"info", sibling_info)]);
        assert_eq!(
            torrent_file_manifest(&sibling_leaf),
            Err(TorrentMetadataError::InvalidFiles)
        );
    }
}
