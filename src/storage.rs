//! The storage capability: a small key-value store and structured-object files
//! under one application-data directory. It is host-independent (plain file IO),
//! so it runs and is tested on every platform; the Android adapter mirrors it
//! over SharedPreferences plus the app's private files dir.
//!
//! Determinism: keys are sorted before listing, values are JSON with sorted map
//! order, and no wall clock enters a stored value, so the same operations always
//! produce the same on-disk bytes and the same `list` order.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::crypto::CryptoError;

/// Typed storage failures; an operation never silently succeeds on bad IO.
#[derive(Debug)]
pub enum StorageError {
    Io(String),
    InvalidKey(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::Io(message) => write!(f, "storage io error: {message}"),
            StorageError::InvalidKey(key) => write!(f, "invalid storage key: {key:?}"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<CryptoError> for StorageError {
    fn from(error: CryptoError) -> Self {
        StorageError::Io(error.to_string())
    }
}

/// A key-value and object store rooted at one directory. Layout:
/// `kv.json` holds the sorted string map; `objects/<key>.json` holds each
/// structured value. Keys are restricted to `[A-Za-z0-9._-]` so a key can never
/// escape the directory.
#[derive(Clone, Debug)]
pub struct Storage {
    root: PathBuf,
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl Storage {
    /// Open (creating if needed) a store rooted at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Storage, StorageError> {
        let root = root.into();
        fs::create_dir_all(root.join("objects"))
            .map_err(|e| StorageError::Io(e.to_string()))?;
        Ok(Storage { root })
    }

    fn kv_path(&self) -> PathBuf {
        self.root.join("kv.json")
    }

    fn object_path(&self, key: &str) -> PathBuf {
        self.root.join("objects").join(format!("{key}.json"))
    }

    fn read_kv(&self) -> Result<BTreeMap<String, String>, StorageError> {
        let path = self.kv_path();
        if !path.is_file() {
            return Ok(BTreeMap::new());
        }
        let text = fs::read_to_string(&path).map_err(|e| StorageError::Io(e.to_string()))?;
        if text.trim().is_empty() {
            return Ok(BTreeMap::new());
        }
        parse_string_map(&text).ok_or_else(|| StorageError::Io("kv.json is not a string map".into()))
    }

    fn write_kv(&self, map: &BTreeMap<String, String>) -> Result<(), StorageError> {
        let text = encode_string_map(map);
        fs::write(self.kv_path(), text).map_err(|e| StorageError::Io(e.to_string()))
    }

    /// Set a key-value entry (string value).
    pub fn set(&self, key: &str, value: &str) -> Result<(), StorageError> {
        if !valid_key(key) {
            return Err(StorageError::InvalidKey(key.to_string()));
        }
        let mut map = self.read_kv()?;
        map.insert(key.to_string(), value.to_string());
        self.write_kv(&map)
    }

    /// Get a key-value entry, or `None` when absent.
    pub fn get(&self, key: &str) -> Result<Option<String>, StorageError> {
        if !valid_key(key) {
            return Err(StorageError::InvalidKey(key.to_string()));
        }
        Ok(self.read_kv()?.get(key).cloned())
    }

    /// Remove a key-value entry and any matching object. Absent keys are a no-op.
    pub fn remove(&self, key: &str) -> Result<(), StorageError> {
        if !valid_key(key) {
            return Err(StorageError::InvalidKey(key.to_string()));
        }
        let mut map = self.read_kv()?;
        let changed = map.remove(key).is_some();
        if changed {
            self.write_kv(&map)?;
        }
        let object = self.object_path(key);
        if object.is_file() {
            fs::remove_file(object).map_err(|e| StorageError::Io(e.to_string()))?;
        }
        Ok(())
    }

    /// All keys in stable sorted order, from both the kv map and the object dir.
    pub fn list(&self) -> Result<Vec<String>, StorageError> {
        let mut keys: Vec<String> = self.read_kv()?.into_keys().collect();
        let objects = self.root.join("objects");
        if let Ok(entries) = fs::read_dir(&objects) {
            for entry in entries.flatten() {
                if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) {
                    if entry.path().extension().and_then(|e| e.to_str()) == Some("json")
                        && valid_key(stem)
                        && !keys.iter().any(|k| k == stem)
                    {
                        keys.push(stem.to_string());
                    }
                }
            }
        }
        keys.sort();
        keys.dedup();
        Ok(keys)
    }

    /// Clear every key-value entry and drop all objects.
    pub fn clear(&self) -> Result<(), StorageError> {
        self.write_kv(&BTreeMap::new())?;
        let objects = self.root.join("objects");
        if let Ok(entries) = fs::read_dir(&objects) {
            for entry in entries.flatten() {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }

    /// Store a structured value as canonical JSON (object keys sorted).
    pub fn set_object(&self, key: &str, json: &str) -> Result<(), StorageError> {
        if !valid_key(key) {
            return Err(StorageError::InvalidKey(key.to_string()));
        }
        let canonical = canonical_json(json)
            .ok_or_else(|| StorageError::Io("object value is not valid JSON".into()))?;
        fs::write(self.object_path(key), canonical).map_err(|e| StorageError::Io(e.to_string()))
    }

    /// Read a structured value previously stored with [`Self::set_object`].
    pub fn get_object(&self, key: &str) -> Result<Option<String>, StorageError> {
        if !valid_key(key) {
            return Err(StorageError::InvalidKey(key.to_string()));
        }
        let path = self.object_path(key);
        if !path.is_file() {
            return Ok(None);
        }
        fs::read_to_string(path).map(Some).map_err(|e| StorageError::Io(e.to_string()))
    }

    /// The directory this store writes to.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Encode a string map as JSON with sorted keys and escaped values. A tiny
/// JSON emitter keeps the deterministic byte layout without a serde dependency.
fn encode_string_map(map: &BTreeMap<String, String>) -> String {
    let mut out = String::from("{");
    for (i, (k, v)) in map.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json_string(k));
        out.push(':');
        out.push_str(&json_string(v));
    }
    out.push('}');
    out
}

/// Parse a flat `{"k":"v",...}` JSON object of strings. Returns `None` on any
/// shape it cannot represent (nested values, non-strings).
fn parse_string_map(text: &str) -> Option<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    let bytes = text.as_bytes();
    let mut i = skip_ws(bytes, 0);
    if bytes.get(i) != Some(&b'{') {
        return None;
    }
    i += 1;
    loop {
        i = skip_ws(bytes, i);
        match bytes.get(i) {
            Some(b'}') => return Some(map),
            Some(b'"') => {}
            _ => return None,
        }
        let (key, next) = parse_json_string(bytes, i)?;
        i = skip_ws(bytes, next);
        if bytes.get(i) != Some(&b':') {
            return None;
        }
        i = skip_ws(bytes, i + 1);
        let (value, next) = parse_json_string(bytes, i)?;
        i = skip_ws(bytes, next);
        map.insert(key, value);
        match bytes.get(i) {
            Some(b',') => i += 1,
            Some(b'}') => return Some(map),
            _ => return None,
        }
    }
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while matches!(bytes.get(i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        i += 1;
    }
    i
}

fn parse_json_string(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut i = start + 1;
    while let Some(&byte) = bytes.get(i) {
        match byte {
            b'"' => return Some((out, i + 1)),
            b'\\' => {
                let escaped = *bytes.get(i + 1)?;
                match escaped {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    _ => return None,
                }
                i += 2;
            }
            _ => {
                out.push(byte as char);
                i += 1;
            }
        }
    }
    None
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Canonicalize an arbitrary JSON value: object keys sorted, no insignificant
/// whitespace. Returns `None` when the input is not parseable JSON. A minimal
/// recursive parser is enough for the structured objects the primitives store.
fn canonical_json(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let (value, end) = parse_value(bytes, skip_ws(bytes, 0))?;
    if skip_ws(bytes, end) != bytes.len() {
        return None;
    }
    Some(value)
}

fn parse_value(bytes: &[u8], i: usize) -> Option<(String, usize)> {
    match bytes.get(i)? {
        b'{' => parse_object(bytes, i),
        b'[' => parse_array(bytes, i),
        b'"' => {
            let (s, next) = parse_json_string(bytes, i)?;
            Some((json_string(&s), next))
        }
        _ => parse_scalar(bytes, i),
    }
}

fn parse_object(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut i = skip_ws(bytes, start + 1);
    if bytes.get(i) == Some(&b'}') {
        return Some(("{}".into(), i + 1));
    }
    loop {
        i = skip_ws(bytes, i);
        let (key, next) = parse_json_string(bytes, i)?;
        i = skip_ws(bytes, next);
        if bytes.get(i) != Some(&b':') {
            return None;
        }
        let (value, next) = parse_value(bytes, skip_ws(bytes, i + 1))?;
        entries.push((key, value));
        i = skip_ws(bytes, next);
        match bytes.get(i) {
            Some(b',') => i += 1,
            Some(b'}') => break,
            _ => return None,
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let body: Vec<String> = entries
        .into_iter()
        .map(|(k, v)| format!("{}:{}", json_string(&k), v))
        .collect();
    Some((format!("{{{}}}", body.join(",")), i + 1))
}

fn parse_array(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    let mut items: Vec<String> = Vec::new();
    let mut i = skip_ws(bytes, start + 1);
    if bytes.get(i) == Some(&b']') {
        return Some(("[]".into(), i + 1));
    }
    loop {
        let (value, next) = parse_value(bytes, skip_ws(bytes, i))?;
        items.push(value);
        i = skip_ws(bytes, next);
        match bytes.get(i) {
            Some(b',') => i += 1,
            Some(b']') => break,
            _ => return None,
        }
    }
    Some((format!("[{}]", items.join(",")), i + 1))
}

fn parse_scalar(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    let mut i = start;
    while matches!(bytes.get(i), Some(b'-' | b'+' | b'.' | b'e' | b'E') | Some(b'0'..=b'9')
        | Some(b't') | Some(b'f') | Some(b'n') | Some(b'a'..=b'z'))
    {
        i += 1;
    }
    if i == start {
        return None;
    }
    let token = std::str::from_utf8(&bytes[start..i]).ok()?;
    match token {
        "true" | "false" | "null" => Some((token.to_string(), i)),
        _ => token.parse::<f64>().ok().map(|_| (token.to_string(), i)),
    }
}
