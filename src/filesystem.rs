//! The filesystem capability: a recursive audio-library scanner plus the
//! metadata reader the music player needs. This module is host-independent —
//! it reads real files and parses real containers, so it runs and is tested
//! on every platform rather than behind a per-OS adapter.
//!
//! `scan_library` walks a root directory, keeps files with a supported audio
//! extension and at a bounded depth, and returns one [`Track`] per file with
//! its parsed tags. The walk is deterministic: entries are sorted by path, so
//! the same tree always yields the same vector.
//!
//! Metadata readers parse two real formats without a codec dependency: an ID3v2
//! (2.3/2.4) tag in an MP3 and a FLAC `VORBIS_COMMENT` block in a FLAC stream.
//! A file whose tags cannot be parsed falls back to its file stem as the title
//! and empty artist/album, never to an error, so a scan never fails on one bad
//! file. WAV audio has no standard tag block here and reports stem-only tags.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// Audio containers the scanner keeps. The extension is matched
/// case-insensitively.
pub const AUDIO_EXTENSIONS: [&str; 4] = ["mp3", "flac", "wav", "m4a"];

/// The default recursion depth cap. A library root is expected to be shallow;
/// the cap keeps a symlink-free but deeply nested tree from an unbounded walk.
pub const DEFAULT_MAX_DEPTH: usize = 8;

/// One scanned audio file and its best-known metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Duration in whole seconds when the container reports it, else 0.
    pub duration_secs: u32,
    /// Codec label derived from the extension, lowercase (`mp3`, `flac`, ...).
    pub format: String,
}

/// Typed scan failures. A missing or unreadable root fails the scan; a single
/// unreadable file is skipped, never fatal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanError {
    MissingRoot(String),
    NotDirectory(String),
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::MissingRoot(path) => write!(f, "scan root does not exist: {path}"),
            ScanError::NotDirectory(path) => write!(f, "scan root is not a directory: {path}"),
        }
    }
}

impl std::error::Error for ScanError {}

/// Recursively scan `root` for audio files up to `max_depth` directories deep,
/// returning tracks sorted by path. Unreadable subdirectories and files are
/// skipped; only a bad root is an error.
pub fn scan_library(root: &Path, max_depth: usize) -> Result<Vec<Track>, ScanError> {
    if !root.exists() {
        return Err(ScanError::MissingRoot(root.display().to_string()));
    }
    if !root.is_dir() {
        return Err(ScanError::NotDirectory(root.display().to_string()));
    }
    let mut paths = Vec::new();
    collect(root, 0, max_depth, &mut paths);
    paths.sort();
    Ok(paths.into_iter().map(|path| read_track(&path)).collect())
}

fn collect(dir: &Path, depth: usize, max_depth: usize, out: &mut Vec<PathBuf>) {
    if depth > max_depth {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect(&path, depth + 1, max_depth, out);
        } else if kind.is_file() && is_audio(&path) {
            out.push(path);
        }
    }
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let lower = ext.to_ascii_lowercase();
            AUDIO_EXTENSIONS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

fn format_of(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Read one file into a `Track`. Tag parse failure degrades to the file stem,
/// never an error, so the scan is robust to one corrupt file.
pub fn read_track(path: &Path) -> Track {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let format = format_of(path);
    let mut track = Track {
        path: path.to_path_buf(),
        title: stem,
        artist: String::new(),
        album: String::new(),
        duration_secs: 0,
        format: format.clone(),
    };
    let Ok(bytes) = fs::read(path) else {
        return track;
    };
    let tags = match format.as_str() {
        "mp3" => read_id3(&bytes),
        "flac" => read_flac(&bytes),
        _ => None,
    };
    if let Some(tags) = tags {
        apply_tags(&mut track, tags);
    }
    track
}

#[derive(Default)]
struct Tags {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    duration_secs: Option<u32>,
}

fn apply_tags(track: &mut Track, tags: Tags) {
    if let Some(title) = tags.title.filter(|t| !t.trim().is_empty()) {
        track.title = title;
    }
    if let Some(artist) = tags.artist {
        track.artist = artist;
    }
    if let Some(album) = tags.album {
        track.album = album;
    }
    if let Some(duration) = tags.duration_secs {
        track.duration_secs = duration;
    }
}

/// Parse an ID3v2.3/2.4 tag at the start of an MP3. Returns `None` when the
/// leading bytes are not a `ID3` tag or the tag is truncated.
fn read_id3(bytes: &[u8]) -> Option<Tags> {
    if bytes.len() < 10 || &bytes[0..3] != b"ID3" {
        return None;
    }
    let version = bytes[3];
    if version < 3 || version > 4 {
        return None;
    }
    let tag_size = syncsafe(&bytes[6..10]);
    let body = bytes.get(10..10 + tag_size)?;
    let mut tags = Tags::default();
    let mut pos = 0;
    while pos + 10 <= body.len() {
        let frame_id = &body[pos..pos + 4];
        if frame_id[0] == 0 {
            break;
        }
        // ID3v2.4 uses syncsafe frame sizes; 2.3 uses a plain big-endian size.
        let size = if version == 4 {
            syncsafe(&body[pos + 4..pos + 8])
        } else {
            u32::from_be_bytes([body[pos + 4], body[pos + 5], body[pos + 6], body[pos + 7]])
                as usize
        };
        let content_start = pos + 10;
        let content_end = content_start.checked_add(size)?;
        let content = body.get(content_start..content_end)?;
        match frame_id {
            b"TIT2" => tags.title = decode_text(content),
            b"TPE1" => tags.artist = decode_text(content),
            b"TALB" => tags.album = decode_text(content),
            b"TLEN" => {
                if let Some(text) = decode_text(content) {
                    if let Ok(ms) = text.trim().parse::<u64>() {
                        tags.duration_secs = Some((ms / 1000) as u32);
                    }
                }
            }
            _ => {}
        }
        pos = content_end;
    }
    Some(tags)
}

fn syncsafe(bytes: &[u8]) -> usize {
    ((bytes[0] as usize & 0x7f) << 21)
        | ((bytes[1] as usize & 0x7f) << 14)
        | ((bytes[2] as usize & 0x7f) << 7)
        | (bytes[3] as usize & 0x7f)
}

/// Decode an ID3 text frame. Encoding byte: 0=ISO-8859-1, 3=UTF-8. A UTF-16
/// frame (1/2) is decoded as its low byte per character, a documented shortcut.
fn decode_text(content: &[u8]) -> Option<String> {
    let (&encoding, rest) = content.split_first()?;
    let text = match encoding {
        0 => rest.iter().map(|&b| b as char).collect::<String>(),
        3 => String::from_utf8_lossy(rest).into_owned(),
        1 | 2 => {
            // shortcut: UTF-16 frames decode as their even bytes, upgrade to a
            // real UTF-16 decoder if a library ships BOM-marked UTF-16 tags.
            rest.chunks(2).map(|c| c[0] as char).collect::<String>()
        }
        _ => return None,
    };
    let text = text.trim_end_matches('\u{0}').trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Parse a FLAC stream: check the `fLaC` marker, walk metadata blocks to the
/// `VORBIS_COMMENT` (type 4) block, and read `TITLE`/`ARTIST`/`ALBUM`. The
/// `STREAMINFO` (type 0) block supplies the duration.
fn read_flac(bytes: &[u8]) -> Option<Tags> {
    if bytes.len() < 4 || &bytes[0..4] != b"fLaC" {
        return None;
    }
    let mut tags = Tags::default();
    let mut pos = 4;
    loop {
        if pos + 4 > bytes.len() {
            break;
        }
        let header = bytes[pos];
        let is_last = header & 0x80 != 0;
        let block_type = header & 0x7f;
        let len = ((bytes[pos + 1] as usize) << 16)
            | ((bytes[pos + 2] as usize) << 8)
            | (bytes[pos + 3] as usize);
        let start = pos + 4;
        let end = start.checked_add(len)?;
        let block = bytes.get(start..end)?;
        match block_type {
            0 => tags.duration_secs = streaminfo_duration(block),
            4 => parse_vorbis_comment(block, &mut tags),
            _ => {}
        }
        if is_last {
            break;
        }
        pos = end;
    }
    Some(tags)
}

fn streaminfo_duration(block: &[u8]) -> Option<u32> {
    // STREAMINFO: 34 bytes; sample rate (20 bits) at byte 10, total samples
    // (36 bits) starting at byte 13. Duration = samples / rate.
    if block.len() < 18 {
        return None;
    }
    let sample_rate =
        ((block[10] as u32) << 12) | ((block[11] as u32) << 4) | ((block[12] as u32) >> 4);
    let total_samples = (((block[13] as u64) & 0x0f) << 32)
        | ((block[14] as u64) << 24)
        | ((block[15] as u64) << 16)
        | ((block[16] as u64) << 8)
        | (block[17] as u64);
    if sample_rate == 0 {
        return None;
    }
    Some((total_samples / sample_rate as u64) as u32)
}

/// Parse a Vorbis comment block: a little-endian vendor length, the vendor
/// string, a comment count, then `KEY=value` strings. Only the first `TITLE`,
/// `ARTIST` and `ALBUM` (case-insensitive keys) are kept.
fn parse_vorbis_comment(block: &[u8], tags: &mut Tags) {
    let mut pos = 0;
    let Some(vendor_len) = read_u32_le(block, &mut pos) else {
        return;
    };
    pos += vendor_len;
    let Some(count) = read_u32_le(block, &mut pos) else {
        return;
    };
    for _ in 0..count {
        let Some(len) = read_u32_le(block, &mut pos) else {
            return;
        };
        let Some(entry) = block.get(pos..pos + len) else {
            return;
        };
        pos += len;
        let Ok(text) = std::str::from_utf8(entry) else {
            continue;
        };
        let Some((key, value)) = text.split_once('=') else {
            continue;
        };
        let value = value.to_string();
        match key.to_ascii_uppercase().as_str() {
            "TITLE" if tags.title.is_none() => tags.title = Some(value),
            "ARTIST" if tags.artist.is_none() => tags.artist = Some(value),
            "ALBUM" if tags.album.is_none() => tags.album = Some(value),
            _ => {}
        }
    }
}

fn read_u32_le(block: &[u8], pos: &mut usize) -> Option<usize> {
    let bytes = block.get(*pos..*pos + 4)?;
    let value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    *pos += 4;
    Some(value)
}

/// The filesystem capability reports whether this build can scan a real tree.
/// It always can — the scanner is host-independent — so this is a constant
/// `true` carrier that keeps the capability describable in the registry.
pub const FILESYSTEM_AVAILABLE: bool = true;
