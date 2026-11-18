//! Host backends for the unified native capability contract.
//!
//! Flutter baseline: 3.50-candidate, engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`. Source pins are repeated at
//! each backend. Opurom keeps the same method names, argument keys, key
//! prefix and typed values; the transport is the compile-time
//! [`crate::native_capabilities::CapabilityTable`] over the existing
//! in-process adapter, not a runtime plugin object tree.
//!
//! # Preferences
//!
//! `shared_preferences/lib/shared_preferences.dart:17,59-158` keeps a typed
//! cache and prefixes every platform key with `flutter.`.
//! `shared_preferences_platform_interface/lib/method_channel_shared_preferences.dart:11-38`
//! sends `set<Type>` with `{key, value}` and `remove` with `{key}`, and
//! `getAll` returns the typed platform map. This backend stores each typed
//! value as canonical JSON under the prefixed key in the existing
//! [`crate::storage::Storage`] object directory, so the on-disk layout stays
//! deterministic and host-independent.
//!
//! # Paths
//!
//! `path_provider_platform_interface/lib/src/method_channel_path_provider.dart:15-78`
//! defines the seven method names and the `{type}` argument for external
//! storage. Platform roots come from the official implementations:
//! - Linux `path_provider_linux/lib/src/path_provider_linux.dart:41-76`:
//!   `TMPDIR` or `/tmp`, `xdg.dataHome/<id>`, XDG `DOCUMENTS`/`DOWNLOAD`.
//! - macOS `path_provider_foundation/darwin/Classes/PathProviderPlugin.swift:25-65`:
//!   `NSSearchPathDirectory` user-domain paths with a bundle-id subdirectory.
//! - Windows `path_provider_windows/lib/src/path_provider_windows_real.dart:83-136`:
//!   `GetTempPath`, `RoamingAppData/<id>`, `Documents`, `Downloads`.
//!
//! Opurom injects those roots once at host startup. The resolver is a pure
//! path join so the same injected roots always produce the same answer on
//! every target, and a platform that does not support a directory returns a
//! typed `unimplemented` instead of a guessed path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value as JsonValue};

use crate::framework_channels::{json_to_value, unimplemented, value_to_json};
use crate::native_capabilities::{path_methods, preferences_methods};
use crate::platform_channel::{error_codes, MethodCall, Reply, Value};
use crate::storage::Storage;

fn invalid_argument(message: impl Into<String>) -> Reply {
    Reply::PlatformError {
        code: error_codes::INVALID_ARGUMENT.to_owned(),
        message: Some(message.into()),
        details: Value::Null,
    }
}

fn arg_string(arguments: &Value, name: &str) -> Option<String> {
    let Value::Map(map) = arguments else {
        return None;
    };
    match map.get(&Value::String(name.to_owned())) {
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn arg_value(arguments: &Value, name: &str) -> Option<Value> {
    let Value::Map(map) = arguments else {
        return None;
    };
    map.get(&Value::String(name.to_owned())).cloned()
}

fn typed_json(value_type: &str, value: &Value) -> Result<JsonValue, CodecFailure> {
    let plain = value_to_json(value).map_err(|error| CodecFailure(error.to_string()))?;
    Ok(json!({ "type": value_type, "value": plain }))
}

#[derive(Debug)]
struct CodecFailure(String);

impl From<CodecFailure> for Reply {
    fn from(error: CodecFailure) -> Self {
        Reply::PlatformError {
            code: error_codes::ERROR.to_owned(),
            message: Some(error.0),
            details: Value::Null,
        }
    }
}

fn read_typed(raw: &str) -> Result<(String, Value), CodecFailure> {
    let parsed: JsonValue =
        serde_json::from_str(raw).map_err(|error| CodecFailure(error.to_string()))?;
    let JsonValue::Object(mut object) = parsed else {
        return Err(CodecFailure(
            "preference entry is not a typed object".into(),
        ));
    };
    let Some(JsonValue::String(value_type)) = object.remove("type") else {
        return Err(CodecFailure("preference entry has no type".into()));
    };
    let Some(value) = object.remove("value") else {
        return Err(CodecFailure("preference entry has no value".into()));
    };
    Ok((
        value_type,
        json_to_value(&value).map_err(|e| CodecFailure(e.to_string()))?,
    ))
}

/// Typed preference backend over the existing deterministic file store.
/// Handles the `Capability::Preferences` method table; each public getter
/// takes an unprefixed key and adds Flutter's `flutter.` prefix internally
/// (`shared_preferences.dart:124,131`), so callers use the Dart-facing API.
#[derive(Clone, Debug)]
pub struct PreferencesBackend {
    storage: Storage,
}

impl PreferencesBackend {
    /// Open (creating if needed) a preference store rooted at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, crate::storage::StorageError> {
        Ok(Self {
            storage: Storage::open(root)?,
        })
    }

    /// The underlying deterministic store, for host diagnostics.
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// Call one method from [`preferences_methods::all`]. Unknown methods get
    /// a typed `unimplemented` reply; malformed arguments get
    /// `invalid_argument`.
    pub fn call(&mut self, call: &MethodCall) -> Reply {
        let method = call.method.as_str();
        if !preferences_methods::all().contains(&method) {
            return unimplemented(method);
        }
        if method == preferences_methods::GET_KEYS {
            return self.keys();
        }
        if method == preferences_methods::CLEAR {
            return self.clear();
        }
        if method == preferences_methods::COMMIT || method == preferences_methods::RELOAD {
            // `commit` is a documented no-op (`shared_preferences.dart:144`);
            // `reload` has no external native writer in the in-process
            // adapter, so the store already holds the latest value.
            return Reply::Success(Value::Bool(true));
        }
        let Some(key) = arg_string(&call.arguments, "key") else {
            return invalid_argument("preference method requires a string key");
        };
        let prefixed = crate::native_capabilities::prefixed_key(&key);
        match method {
            preferences_methods::GET
            | preferences_methods::GET_BOOL
            | preferences_methods::GET_INT
            | preferences_methods::GET_DOUBLE
            | preferences_methods::GET_STRING
            | preferences_methods::GET_STRING_LIST => self.get(method, &prefixed),
            preferences_methods::SET_BOOL
            | preferences_methods::SET_INT
            | preferences_methods::SET_DOUBLE
            | preferences_methods::SET_STRING
            | preferences_methods::SET_STRING_LIST => {
                let Some(value) = arg_value(&call.arguments, "value") else {
                    return invalid_argument("preference setter requires a value");
                };
                self.set(method, &prefixed, &value)
            }
            preferences_methods::REMOVE => self.remove(&prefixed),
            _ => unimplemented(method),
        }
    }

    fn keys(&self) -> Reply {
        match self.storage.list() {
            Ok(keys) => Reply::Success(Value::List(
                keys.iter()
                    .filter_map(|key| crate::native_capabilities::unprefixed_key(key))
                    .map(|key| Value::String(key.to_owned()))
                    .collect(),
            )),
            Err(error) => Reply::PlatformError {
                code: error_codes::ERROR.to_owned(),
                message: Some(error.to_string()),
                details: Value::Null,
            },
        }
    }

    fn clear(&mut self) -> Reply {
        match self.storage.clear() {
            Ok(()) => Reply::Success(Value::Bool(true)),
            Err(error) => Reply::PlatformError {
                code: error_codes::ERROR.to_owned(),
                message: Some(error.to_string()),
                details: Value::Null,
            },
        }
    }

    fn get(&self, method: &str, prefixed: &str) -> Reply {
        let raw = match self.storage.get_object(prefixed) {
            Ok(raw) => raw,
            Err(error) => {
                return Reply::PlatformError {
                    code: error_codes::ERROR.to_owned(),
                    message: Some(error.to_string()),
                    details: Value::Null,
                }
            }
        };
        let Some(raw) = raw else {
            return Reply::Success(Value::Null);
        };
        let (stored_type, value) = match read_typed(&raw) {
            Ok(parsed) => parsed,
            Err(error) => return error.into(),
        };
        let expected = value_type_for(method);
        if stored_type != expected {
            // Dart's `getBool`/`getString` casts throw a TypeError for a
            // different stored type. The codec-neutral reply keeps that
            // distinction as a typed error instead of returning null.
            return Reply::PlatformError {
                code: error_codes::ERROR.to_owned(),
                message: Some(format!(
                    "preference {prefixed} stores {stored_type}, not {expected}"
                )),
                details: Value::Null,
            };
        }
        Reply::Success(value)
    }

    fn set(&mut self, method: &str, prefixed: &str, value: &Value) -> Reply {
        let value_type = value_type_for(method);
        if !value_matches_type(value_type, value) {
            return invalid_argument(format!("{method} requires a {value_type} value"));
        }
        let encoded = match typed_json(value_type, value) {
            Ok(encoded) => encoded,
            Err(error) => return error.into(),
        };
        match self.storage.set_object(prefixed, &encoded.to_string()) {
            Ok(()) => Reply::Success(Value::Bool(true)),
            Err(error) => Reply::PlatformError {
                code: error_codes::ERROR.to_owned(),
                message: Some(error.to_string()),
                details: Value::Null,
            },
        }
    }

    fn remove(&mut self, prefixed: &str) -> Reply {
        match self.storage.remove(prefixed) {
            Ok(()) => Reply::Success(Value::Bool(true)),
            Err(error) => Reply::PlatformError {
                code: error_codes::ERROR.to_owned(),
                message: Some(error.to_string()),
                details: Value::Null,
            },
        }
    }

    /// A [`crate::native_capabilities::CapabilityHandler`] for this backend.
    /// The returned state pointer points at `self`; callers must keep `self`
    /// alive and not move it while the handler table is in use.
    pub fn handler(&mut self) -> crate::native_capabilities::CapabilityHandler {
        crate::native_capabilities::CapabilityHandler {
            capability: crate::native_capabilities::Capability::Preferences,
            function: preferences_handler,
            state: self as *mut Self as *mut u8,
        }
    }
}

/// `CapabilityHandler` entry point. # Safety: `state` must point to a live
/// [`PreferencesBackend`] for the duration of this call.
pub unsafe fn preferences_handler(state: *mut u8, call: &MethodCall) -> Reply {
    // SAFETY: documented by the caller and by `PreferencesBackend::handler`.
    unsafe { &mut *(state as *mut PreferencesBackend) }.call(call)
}

fn value_type_for(method: &str) -> &'static str {
    match method {
        preferences_methods::GET_BOOL | preferences_methods::SET_BOOL => "Bool",
        preferences_methods::GET_INT | preferences_methods::SET_INT => "Int",
        preferences_methods::GET_DOUBLE | preferences_methods::SET_DOUBLE => "Double",
        preferences_methods::GET_STRING | preferences_methods::SET_STRING => "String",
        preferences_methods::GET_STRING_LIST | preferences_methods::SET_STRING_LIST => "StringList",
        _ => "Any",
    }
}

fn value_matches_type(value_type: &str, value: &Value) -> bool {
    match value_type {
        "Bool" => matches!(value, Value::Bool(_)),
        "Int" => matches!(value, Value::I32(_) | Value::I64(_)),
        "Double" => matches!(value, Value::F64(_)),
        "String" => matches!(value, Value::String(_)),
        "StringList" => {
            matches!(value, Value::List(items) if items.iter().all(|v| matches!(v, Value::String(_))))
        }
        _ => false,
    }
}

/// The target platform of an injected path set. Mirrors the platform
/// availability checks in `method_channel_path_provider.dart:44-78`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathPlatform {
    Android,
    Ios,
    Macos,
    Linux,
    Windows,
}

impl PathPlatform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Android => "android",
            Self::Ios => "ios",
            Self::Macos => "macos",
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }
}

/// Deterministic path provider. Roots are injected by the host: `temporary`,
/// the user-directory root (`documents`, `downloads`, `library`) and the
/// data root (`application_support`, with `application_id` appended).
#[derive(Clone, Debug)]
pub struct PathResolver {
    platform: PathPlatform,
    temporary: PathBuf,
    user_root: PathBuf,
    data_root: PathBuf,
    application_id: String,
    /// XDG external storage paths on Android; empty elsewhere.
    external_storage_paths: Vec<PathBuf>,
}

impl PathResolver {
    pub fn new(
        platform: PathPlatform,
        temporary: impl Into<PathBuf>,
        user_root: impl Into<PathBuf>,
        data_root: impl Into<PathBuf>,
        application_id: impl Into<String>,
    ) -> Self {
        Self {
            platform,
            temporary: temporary.into(),
            user_root: user_root.into(),
            data_root: data_root.into(),
            application_id: application_id.into(),
            external_storage_paths: Vec::new(),
        }
    }

    pub fn with_external_storage(mut self, paths: Vec<PathBuf>) -> Self {
        self.external_storage_paths = paths;
        self
    }

    /// Call one method from [`path_methods::all`].
    pub fn call(&self, call: &MethodCall) -> Reply {
        if !path_methods::all().contains(&call.method.as_str()) {
            return unimplemented(&call.method);
        }
        match call.method.as_str() {
            path_methods::TEMPORARY => self.path(&self.temporary),
            path_methods::SUPPORT => self.path(&self.data_root.join(&self.application_id)),
            path_methods::LIBRARY if self.apple() => self.path(&self.user_root.join("Library")),
            path_methods::LIBRARY => unimplemented(path_methods::LIBRARY),
            path_methods::DOCUMENTS => self.path(&self.user_root.join("Documents")),
            path_methods::DOWNLOADS if self.has_downloads() => {
                self.path(&self.user_root.join("Downloads"))
            }
            path_methods::DOWNLOADS => unimplemented(path_methods::DOWNLOADS),
            path_methods::EXTERNAL_STORAGE if self.platform == PathPlatform::Android => {
                self.path(&self.user_root.join("storage"))
            }
            path_methods::EXTERNAL_STORAGE => unimplemented(path_methods::EXTERNAL_STORAGE),
            path_methods::EXTERNAL_STORAGE_PATHS if self.platform == PathPlatform::Android => {
                Reply::Success(Value::List(
                    self.external_storage_paths
                        .iter()
                        .map(|path| Value::String(path.to_string_lossy().into_owned()))
                        .collect(),
                ))
            }
            path_methods::EXTERNAL_STORAGE_PATHS => {
                unimplemented(path_methods::EXTERNAL_STORAGE_PATHS)
            }
            _ => unimplemented(&call.method),
        }
    }

    fn apple(&self) -> bool {
        matches!(self.platform, PathPlatform::Ios | PathPlatform::Macos)
    }

    fn has_downloads(&self) -> bool {
        matches!(
            self.platform,
            PathPlatform::Macos | PathPlatform::Linux | PathPlatform::Windows
        )
    }

    fn path(&self, path: &Path) -> Reply {
        if self.application_id.is_empty() {
            return invalid_argument("application id is empty");
        }
        Reply::Success(Value::String(path.to_string_lossy().into_owned()))
    }

    /// A [`crate::native_capabilities::CapabilityHandler`] for this resolver.
    /// # Safety: `state` must point to a live [`PathResolver`] for the call.
    pub fn handler(&mut self) -> crate::native_capabilities::CapabilityHandler {
        crate::native_capabilities::CapabilityHandler {
            capability: crate::native_capabilities::Capability::PathProvider,
            function: path_handler,
            state: self as *mut Self as *mut u8,
        }
    }
}

/// `CapabilityHandler` entry point for path methods.
/// # Safety: `state` must point to a live [`PathResolver`] for this call.
pub unsafe fn path_handler(state: *mut u8, call: &MethodCall) -> Reply {
    // SAFETY: documented by the caller and by `PathResolver::handler`.
    unsafe { &*(state as *const PathResolver) }.call(call)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_capabilities::{qualified, Capability, CapabilityTable};

    fn temp_dir(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "opurom-platform-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn call(backend: &mut PreferencesBackend, method: &str, args: Value) -> Reply {
        backend.call(&MethodCall {
            method: method.to_owned(),
            arguments: args,
        })
    }

    #[test]
    fn preferences_round_trip_every_typed_value() {
        let root = temp_dir("preferences");
        let mut backend = PreferencesBackend::open(&root).unwrap();
        let mut set = |key: &str, method: &str, value: Value| {
            let mut map = BTreeMap::new();
            map.insert(Value::String("key".into()), Value::String(key.into()));
            map.insert(Value::String("value".into()), value);
            call(&mut backend, method, Value::Map(map))
        };
        assert_eq!(
            set("flag", preferences_methods::SET_BOOL, Value::Bool(true)),
            Reply::Success(Value::Bool(true))
        );
        assert_eq!(
            set("count", preferences_methods::SET_INT, Value::I32(42)),
            Reply::Success(Value::Bool(true))
        );
        assert_eq!(
            set("ratio", preferences_methods::SET_DOUBLE, Value::F64(0.25)),
            Reply::Success(Value::Bool(true))
        );
        assert_eq!(
            set(
                "name",
                preferences_methods::SET_STRING,
                Value::String("Opurom".into())
            ),
            Reply::Success(Value::Bool(true))
        );
        assert_eq!(
            set(
                "items",
                preferences_methods::SET_STRING_LIST,
                Value::List(vec![Value::String("a".into()), Value::String("b".into())])
            ),
            Reply::Success(Value::Bool(true))
        );
        for (key, method, expected) in [
            ("flag", preferences_methods::GET_BOOL, Value::Bool(true)),
            ("count", preferences_methods::GET_INT, Value::I32(42)),
            ("ratio", preferences_methods::GET_DOUBLE, Value::F64(0.25)),
            (
                "name",
                preferences_methods::GET_STRING,
                Value::String("Opurom".into()),
            ),
            (
                "items",
                preferences_methods::GET_STRING_LIST,
                Value::List(vec![Value::String("a".into()), Value::String("b".into())]),
            ),
        ] {
            let mut args = BTreeMap::new();
            args.insert(Value::String("key".into()), Value::String(key.into()));
            assert_eq!(
                call(&mut backend, method, Value::Map(args)),
                Reply::Success(expected)
            );
        }
        assert_eq!(
            backend.keys(),
            Reply::Success(Value::List(vec![
                Value::String("count".into()),
                Value::String("flag".into()),
                Value::String("items".into()),
                Value::String("name".into()),
                Value::String("ratio".into()),
            ]))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn preference_type_mismatch_and_missing_key_are_not_fabricated() {
        let root = temp_dir("preference-mismatch");
        let mut backend = PreferencesBackend::open(&root).unwrap();
        let mut args = BTreeMap::new();
        args.insert(Value::String("key".into()), Value::String("x".into()));
        args.insert(Value::String("value".into()), Value::String("text".into()));
        assert_eq!(
            call(
                &mut backend,
                preferences_methods::SET_STRING,
                Value::Map(args.clone())
            ),
            Reply::Success(Value::Bool(true))
        );
        args.remove(&Value::String("value".into()));
        assert_eq!(
            call(
                &mut backend,
                preferences_methods::GET_BOOL,
                Value::Map(args.clone())
            ),
            Reply::PlatformError {
                code: error_codes::ERROR.to_owned(),
                message: Some("preference flutter.x stores String, not Bool".into()),
                details: Value::Null,
            }
        );
        args.insert(Value::String("key".into()), Value::String("absent".into()));
        assert_eq!(
            call(
                &mut backend,
                preferences_methods::GET_STRING,
                Value::Map(args)
            ),
            Reply::Success(Value::Null)
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn preferences_handler_drives_the_capability_table() {
        let root = temp_dir("preference-table");
        let mut backend = PreferencesBackend::open(&root).unwrap();
        let table = CapabilityTable::new().with(backend.handler());
        let mut args = BTreeMap::new();
        args.insert(Value::String("key".into()), Value::String("k".into()));
        args.insert(Value::String("value".into()), Value::I32(9));
        assert_eq!(
            unsafe {
                table.call(&qualified(
                    Capability::Preferences,
                    preferences_methods::SET_INT,
                    Value::Map(args.clone()),
                ))
            },
            Reply::Success(Value::Bool(true))
        );
        args.remove(&Value::String("value".into()));
        assert_eq!(
            unsafe {
                table.call(&qualified(
                    Capability::Preferences,
                    preferences_methods::GET_INT,
                    Value::Map(args),
                ))
            },
            Reply::Success(Value::I32(9))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn path_resolver_matches_platform_availability() {
        let resolver = PathResolver::new(
            PathPlatform::Linux,
            "/tmp",
            "/home/user",
            "/home/user/.local/share",
            "opurom",
        );
        let call = |method: &str| MethodCall {
            method: method.to_owned(),
            arguments: Value::Null,
        };
        assert_eq!(
            resolver.call(&call(path_methods::TEMPORARY)),
            Reply::Success(Value::String("/tmp".into()))
        );
        assert_eq!(
            resolver.call(&call(path_methods::SUPPORT)),
            Reply::Success(Value::String("/home/user/.local/share/opurom".into()))
        );
        assert_eq!(
            resolver.call(&call(path_methods::DOCUMENTS)),
            Reply::Success(Value::String("/home/user/Documents".into()))
        );
        assert_eq!(
            resolver.call(&call(path_methods::DOWNLOADS)),
            Reply::Success(Value::String("/home/user/Downloads".into()))
        );
        assert_eq!(
            resolver.call(&call(path_methods::LIBRARY)),
            unimplemented(path_methods::LIBRARY)
        );
        assert_eq!(
            resolver.call(&call(path_methods::EXTERNAL_STORAGE)),
            unimplemented(path_methods::EXTERNAL_STORAGE)
        );

        let mut mac = PathResolver::new(
            PathPlatform::Macos,
            "/var/folders/tmp",
            "/Users/user",
            "/Users/user/Library/Application Support",
            "dev.opurom.app",
        )
        .with_external_storage(vec![PathBuf::from("/Volumes/USB")]);
        assert_eq!(
            mac.call(&call(path_methods::LIBRARY)),
            Reply::Success(Value::String("/Users/user/Library".into()))
        );
        assert_eq!(
            mac.call(&call(path_methods::EXTERNAL_STORAGE_PATHS)),
            unimplemented(path_methods::EXTERNAL_STORAGE_PATHS)
        );
        let table = CapabilityTable::new().with(mac.handler());
        assert_eq!(
            unsafe {
                table.call(&qualified(
                    Capability::PathProvider,
                    path_methods::SUPPORT,
                    Value::Null,
                ))
            },
            Reply::Success(Value::String(
                "/Users/user/Library/Application Support/dev.opurom.app".into()
            ))
        );
    }
}
