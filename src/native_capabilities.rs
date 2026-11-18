//! Unified high-frequency native capability contract.
//!
//! Flutter baseline: 3.50-candidate, engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`. Plugin snapshot:
//! `flutter-plugins` at `557d3284ac9dda32a1106bb75be8a23bdccd2f96`.
//!
//! This module is the *contract and method table* for the high-frequency
//! native capabilities in module 5 of the Opurom parity directive. It follows
//! the architecture decision in `opurom-spec/PLATFORM_CHANNEL_PARITY.md`:
//! capability calls are compile-time typed methods over the existing
//! in-process adapter, not a runtime plugin registry. The wire values reuse
//! [`crate::platform_channel::Value`] and [`crate::platform_channel::Reply`]
//! so the same encoding is available if a call later crosses a process.
//!
//! Only capabilities with a pinned local Flutter source are given concrete
//! method names here. Capabilities whose official plugin source is not in
//! `flutter-plugins` are listed in [`Capability::unavailable_reason`] and
//! return `unimplemented` rather than a made-up API.
//!
//! Source pins:
//! - `flutter-plugins/packages/shared_preferences/shared_preferences/lib/shared_preferences.dart:17-158`
//!   (key prefix, typed getters/setters, remove/clear/reload).
//! - `flutter-plugins/packages/path_provider/path_provider/lib/path_provider.dart:54-136`
//!   (directory getters and their nullability).
//! - `flutter-plugins/packages/path_provider/path_provider_platform_interface/lib/src/method_channel_path_provider.dart:15-52`
//!   (channel name `plugins.flutter.io/path_provider`).
//! - `flutter-plugins/packages/url_launcher/url_launcher_platform_interface/lib/src/url_launcher_platform.dart:56-87`
//!   (`canLaunch`/`launch`/`launchUrl`/`closeWebView` contract and mode derivation).
//! - `flutter-plugins/packages/url_launcher/url_launcher_platform_interface/lib/method_channel_url_launcher.dart:12-52`
//!   (channel name and argument map).
//! - `flutter-plugins/packages/image_picker/image_picker_platform_interface/lib/src/method_channel/method_channel_image_picker.dart:12,75-152`
//!   (`pickImage`/`pickMultiImage`/`pickVideo`/`retrieve` and argument maps).

use std::collections::BTreeMap;

use crate::platform_channel::{error_codes, MethodCall, Reply, Value};

/// Canonical capability names. This is the closed set of module-5 capability
/// groups; each maps to one or more Flutter plugins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    /// key-value persistent storage (`shared_preferences`).
    Preferences,
    /// standard filesystem directories (`path_provider`).
    PathProvider,
    /// open URLs / system browser (`url_launcher`).
    UrlLauncher,
    /// image capture/selection (`image_picker`).
    ImagePicker,
    /// file-backed key-value storage (`opurom-platform/src/storage.rs`);
    /// Flutter's `shared_preferences` covers the same primitive with a
    /// different persistence backend.
    FileStorage,
    /// system notification posting (`notification.rs`).
    Notifications,
    /// device information. GAP: no `device_info_plus` source in local
    /// `flutter-plugins`; only the platform's own display/geometry facts are
    /// exposed, see [`device_info_methods`].
    DeviceInfo,
    /// network reachability. GAP: no `connectivity_plus` source in local
    /// snapshot; answers `unimplemented` rather than inventing a state model.
    Network,
    /// runtime permission request/status. GAP: no permission plugin source;
    /// answers `unimplemented`.
    Permissions,
    /// system share sheet. GAP: no share plugin source; answers `unimplemented`.
    Share,
    /// clipboard read/write. Flutter framework source
    /// `services/clipboard.dart:46-72` is pinned in `flutter-golden`, but no
    /// host clipboard backend exists in `opurom-platform` yet; the capability
    /// is declared so the no-op contract is explicit.
    Clipboard,
    /// haptic feedback. Framework `services/haptic_feedback.dart:29-33`.
    Haptics,
    /// camera capture (`camera` plugin, pinned local source).
    Camera,
    /// video playback (`video_player` plugin, pinned local source).
    VideoPlayer,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Preferences => "preferences",
            Capability::PathProvider => "path_provider",
            Capability::UrlLauncher => "url_launcher",
            Capability::ImagePicker => "image_picker",
            Capability::FileStorage => "file_storage",
            Capability::Notifications => "notifications",
            Capability::DeviceInfo => "device_info",
            Capability::Network => "network",
            Capability::Permissions => "permissions",
            Capability::Share => "share",
            Capability::Clipboard => "clipboard",
            Capability::Haptics => "haptics",
            Capability::Camera => "camera",
            Capability::VideoPlayer => "video_player",
        }
    }

    pub fn from_str(name: &str) -> Option<Self> {
        Some(match name {
            "preferences" => Self::Preferences,
            "path_provider" => Self::PathProvider,
            "url_launcher" => Self::UrlLauncher,
            "image_picker" => Self::ImagePicker,
            "file_storage" => Self::FileStorage,
            "notifications" => Self::Notifications,
            "device_info" => Self::DeviceInfo,
            "network" => Self::Network,
            "permissions" => Self::Permissions,
            "share" => Self::Share,
            "clipboard" => Self::Clipboard,
            "haptics" => Self::Haptics,
            "camera" => Self::Camera,
            "video_player" => Self::VideoPlayer,
            _ => return None,
        })
    }

    /// Why this capability has no concrete implementation yet. `None` means
    /// the method table is defined by a pinned Flutter source.
    pub fn unavailable_reason(self) -> Option<&'static str> {
        match self {
            Capability::Network => Some("no connectivity plugin source in the local snapshot"),
            Capability::Permissions => Some("no permission plugin source in the local snapshot"),
            Capability::Share => Some(
                "no share plugin source in the local snapshot; Share.invoke is frozen on flutter/platform",
            ),
            Capability::DeviceInfo => Some(
                "no device_info plugin source in the local snapshot; local projections are not parity",
            ),
            _ => None,
        }
    }
}

/// All declared capabilities in stable order.
pub const ALL_CAPABILITIES: [Capability; 14] = [
    Capability::Preferences,
    Capability::PathProvider,
    Capability::UrlLauncher,
    Capability::ImagePicker,
    Capability::FileStorage,
    Capability::Notifications,
    Capability::DeviceInfo,
    Capability::Network,
    Capability::Permissions,
    Capability::Share,
    Capability::Clipboard,
    Capability::Haptics,
    Capability::Camera,
    Capability::VideoPlayer,
];

/// Method names for `preferences`, from `shared_preferences.dart:66-158`.
pub mod preferences_methods {
    pub const GET_KEYS: &str = "getKeys";
    pub const GET: &str = "get";
    pub const GET_BOOL: &str = "getBool";
    pub const GET_INT: &str = "getInt";
    pub const GET_DOUBLE: &str = "getDouble";
    pub const GET_STRING: &str = "getString";
    pub const GET_STRING_LIST: &str = "getStringList";
    pub const SET_BOOL: &str = "setBool";
    pub const SET_INT: &str = "setInt";
    pub const SET_DOUBLE: &str = "setDouble";
    pub const SET_STRING: &str = "setString";
    pub const SET_STRING_LIST: &str = "setStringList";
    pub const REMOVE: &str = "remove";
    pub const CLEAR: &str = "clear";
    pub const RELOAD: &str = "reload";
    pub const COMMIT: &str = "commit";

    /// Flutter prefixes every key with `flutter.` before handing it to the
    /// platform store (`shared_preferences.dart:17,124,131`).
    pub const KEY_PREFIX: &str = "flutter.";

    pub fn all() -> &'static [&'static str] {
        &[
            GET_KEYS,
            GET,
            GET_BOOL,
            GET_INT,
            GET_DOUBLE,
            GET_STRING,
            GET_STRING_LIST,
            SET_BOOL,
            SET_INT,
            SET_DOUBLE,
            SET_STRING,
            SET_STRING_LIST,
            REMOVE,
            CLEAR,
            RELOAD,
            COMMIT,
        ]
    }
}

/// Method names for `path_provider`, from `path_provider.dart:54-136` and
/// `method_channel_path_provider.dart:32-52`.
pub mod path_methods {
    pub const TEMPORARY: &str = "getTemporaryDirectory";
    pub const SUPPORT: &str = "getApplicationSupportDirectory";
    pub const LIBRARY: &str = "getLibraryDirectory";
    pub const DOCUMENTS: &str = "getApplicationDocumentsDirectory";
    pub const EXTERNAL_STORAGE: &str = "getExternalStorageDirectory";
    pub const EXTERNAL_STORAGE_PATHS: &str = "getExternalStorageDirectories";
    pub const DOWNLOADS: &str = "getDownloadsDirectory";

    pub fn all() -> &'static [&'static str] {
        &[
            TEMPORARY,
            SUPPORT,
            LIBRARY,
            DOCUMENTS,
            EXTERNAL_STORAGE,
            EXTERNAL_STORAGE_PATHS,
            DOWNLOADS,
        ]
    }
}

/// Method names for `url_launcher`, from `url_launcher_platform.dart:45-96`
/// and `method_channel_url_launcher.dart:17-55`.
pub mod url_methods {
    pub const CAN_LAUNCH: &str = "canLaunch";
    pub const LAUNCH: &str = "launch";
    pub const LAUNCH_URL: &str = "launchUrl";
    pub const CLOSE_WEB_VIEW: &str = "closeWebView";

    /// Argument keys of `launch` (`method_channel_url_launcher.dart:36-51`).
    pub const ARG_URL: &str = "url";
    pub const ARG_USE_SAFARI_VC: &str = "useSafariVC";
    pub const ARG_USE_WEB_VIEW: &str = "useWebView";
    pub const ARG_ENABLE_JAVASCRIPT: &str = "enableJavaScript";
    pub const ARG_ENABLE_DOM_STORAGE: &str = "enableDomStorage";
    pub const ARG_UNIVERSAL_LINKS_ONLY: &str = "universalLinksOnly";
    pub const ARG_HEADERS: &str = "headers";

    /// `method_channel_url_launcher.dart:36-51` omits the web-only window
    /// name from the platform call; `types.dart:56-65` defines it.
    pub const ARG_WEB_ONLY_WINDOW_NAME: &str = "webOnlyWindowName";

    /// `PreferredLaunchMode` index order (`types.dart:11-26`).
    pub const MODE_PLATFORM_DEFAULT: u8 = 0;
    pub const MODE_IN_APP_WEB_VIEW: u8 = 1;
    pub const MODE_EXTERNAL_APPLICATION: u8 = 2;
    pub const MODE_EXTERNAL_NON_BROWSER_APPLICATION: u8 = 3;

    /// Web-view configuration defaults (`types.dart:34-36`).
    pub const DEFAULT_ENABLE_JAVASCRIPT: bool = true;
    pub const DEFAULT_ENABLE_DOM_STORAGE: bool = true;

    pub fn all() -> &'static [&'static str] {
        &[CAN_LAUNCH, LAUNCH, CLOSE_WEB_VIEW]
    }
}

/// Method names for `image_picker` from
/// `image_picker_platform_interface/lib/src/method_channel/method_channel_image_picker.dart:12,75,107,139,152`
/// and `types/types.dart:15-18`.
pub mod image_methods {
    pub const PICK_IMAGE: &str = "pickImage";
    pub const PICK_MULTI_IMAGE: &str = "pickMultiImage";
    pub const PICK_VIDEO: &str = "pickVideo";
    pub const RETRIEVE: &str = "retrieve";

    /// `ImageSource.camera`/`.gallery` index order
    /// (`types/image_source.dart:9-16`). The platform wire uses the index,
    /// but the typed contract keeps the canonical names for callers.
    pub const SOURCE_CAMERA: &str = "camera";
    pub const SOURCE_GALLERY: &str = "gallery";
    pub const SOURCE_INDEX_CAMERA: u8 = 0;
    pub const SOURCE_INDEX_GALLERY: u8 = 1;
    /// `types/camera_device.dart:9-15`.
    pub const CAMERA_REAR: &str = "rear";
    pub const CAMERA_FRONT: &str = "front";
    pub const CAMERA_INDEX_REAR: u8 = 0;
    pub const CAMERA_INDEX_FRONT: u8 = 1;

    /// Argument keys from `method_channel_image_picker.dart:108-115,139-143,76-82`.
    pub const ARG_SOURCE: &str = "source";
    pub const ARG_MAX_WIDTH: &str = "maxWidth";
    pub const ARG_MAX_HEIGHT: &str = "maxHeight";
    pub const ARG_IMAGE_QUALITY: &str = "imageQuality";
    pub const ARG_CAMERA_DEVICE: &str = "cameraDevice";
    pub const ARG_REQUEST_FULL_METADATA: &str = "requestFullMetadata";
    pub const ARG_MAX_DURATION: &str = "maxDuration";
    /// `types/types.dart:15-18`.
    pub const TYPE_IMAGE: &str = "image";
    pub const TYPE_VIDEO: &str = "video";

    pub fn all() -> &'static [&'static str] {
        &[PICK_IMAGE, PICK_MULTI_IMAGE, PICK_VIDEO, RETRIEVE]
    }
}

/// Device facts that are actually derivable from `opurom-platform` without
/// a missing plugin. Each is a projection of an existing typed adapter API;
/// no device model / OS version is invented.
pub mod device_info_methods {
    pub const DISPLAY_GEOMETRY: &str = "displayGeometry";
    pub const DPI_SCALE: &str = "dpiScale";
    pub const GRAPHICS_BACKEND: &str = "graphicsBackend";
    pub const VIEWPORT: &str = "viewport";

    pub fn all() -> &'static [&'static str] {
        &[DISPLAY_GEOMETRY, DPI_SCALE, GRAPHICS_BACKEND, VIEWPORT]
    }
}

fn common_no_op_methods() -> &'static [&'static str] {
    &[]
}

/// Methods with a pinned Flutter source. `DeviceInfo` deliberately lists only
/// the projections this crate can compute; it is not a claim of full
/// `device_info_plus` parity.
pub fn methods_for(capability: Capability) -> &'static [&'static str] {
    match capability {
        Capability::Preferences => preferences_methods::all(),
        Capability::PathProvider => path_methods::all(),
        Capability::UrlLauncher => url_methods::all(),
        Capability::ImagePicker => image_methods::all(),
        Capability::DeviceInfo => device_info_methods::all(),
        Capability::Camera => crate::media_capabilities::camera_methods::all(),
        Capability::VideoPlayer => crate::media_capabilities::video_methods::all(),
        Capability::Clipboard => &["getData", "hasStrings", "setData"],
        Capability::Haptics => &[
            "vibrate",
            "lightImpact",
            "mediumImpact",
            "heavyImpact",
            "selectionClick",
        ],
        Capability::Notifications => &["post", "cancel"],
        Capability::FileStorage => &["read", "write", "remove", "keys"],
        Capability::Network | Capability::Permissions | Capability::Share => common_no_op_methods(),
    }
}

/// A compile-time row in the unified capability table. The state pointer and
/// handler follow the same lifetime rule as
/// [`crate::platform_channel::ChannelTable`].
#[derive(Clone, Copy)]
pub struct CapabilityHandler {
    pub capability: Capability,
    pub function: unsafe fn(*mut u8, &MethodCall) -> Reply,
    pub state: *mut u8,
}

impl std::fmt::Debug for CapabilityHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapabilityHandler")
            .field("capability", &self.capability)
            .field("state", &self.state)
            .finish()
    }
}

/// Immutable capability table. Missing capability/method pairs answer a typed
/// `unimplemented` error, never `Success(Null)` — matching Flutter's
/// `UnimplementedError` / `MissingPluginException` distinction
/// (`platform_channel.dart:350-366`, `url_launcher_platform.dart:45-48`).
#[derive(Default)]
pub struct CapabilityTable {
    handlers: Vec<CapabilityHandler>,
}

impl CapabilityTable {
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    pub fn with(mut self, handler: CapabilityHandler) -> Self {
        if let Some(slot) = self
            .handlers
            .iter_mut()
            .find(|h| h.capability == handler.capability)
        {
            *slot = handler;
        } else {
            self.handlers.push(handler);
        }
        self
    }

    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// Calls one capability method. The codec-neutral `Value` result is
    /// wrapped in a `Reply` so error semantics travel with the value.
    ///
    /// # Safety
    /// Each handler's `state` pointer must remain valid for this call.
    pub unsafe fn call(&self, call: &MethodCall) -> Reply {
        let capability = match Capability::from_str(&call.method.split('.').next().unwrap_or("")) {
            Some(capability) if call.method.contains('.') => capability,
            _ => {
                // The method name may be capability-qualified in the table
                // (e.g. "preferences.getString") or the caller may have set
                // Capability in a separate field; accept both forms.
                match self
                    .handlers
                    .iter()
                    .find(|h| methods_for(h.capability).contains(&call.method.as_str()))
                {
                    Some(h) => h.capability,
                    None => {
                        return Reply::PlatformError {
                            code: error_codes::UNIMPLEMENTED.to_owned(),
                            message: Some(format!("unknown capability method {}", call.method)),
                            details: Value::Null,
                        }
                    }
                }
            }
        };
        let Some(handler) = self.handlers.iter().find(|h| h.capability == capability) else {
            let reason = capability
                .unavailable_reason()
                .unwrap_or("no host handler registered");
            return Reply::PlatformError {
                code: error_codes::UNIMPLEMENTED.to_owned(),
                message: Some(reason.to_owned()),
                details: Value::Null,
            };
        };
        let method = call.method.rsplit('.').next().unwrap_or(&call.method);
        if !methods_for(capability).contains(&method) {
            return Reply::PlatformError {
                code: error_codes::UNIMPLEMENTED.to_owned(),
                message: Some(format!(
                    "method {method} is not part of {}",
                    capability.as_str()
                )),
                details: Value::Null,
            };
        }
        let inner = MethodCall {
            method: method.to_owned(),
            arguments: call.arguments.clone(),
        };
        unsafe { (handler.function)(handler.state, &inner) }
    }
}

/// Build a `MethodCall` for a capability method. Qualified names
/// (`"preferences.getString"`) disambiguate methods shared across plugins.
pub fn qualified(capability: Capability, method: &str, arguments: Value) -> MethodCall {
    MethodCall {
        method: format!("{}.{}", capability.as_str(), method),
        arguments,
    }
}

/// Prefix a key with Flutter's `flutter.` prefix before writing to the
/// platform store (`shared_preferences.dart:17,124,131`).
pub fn prefixed_key(key: &str) -> String {
    format!("{}{}", preferences_methods::KEY_PREFIX, key)
}

/// Strip the prefix when reading back into the preference cache
/// (`shared_preferences.dart:169-170,183-184`). `None` means the key belongs
/// to another store and must not surface to the caller.
pub fn unprefixed_key(key: &str) -> Option<&str> {
    key.strip_prefix(preferences_methods::KEY_PREFIX)
}

/// Helper for building a `Value::Map` from string keys, used by launch options
/// and picker arguments.
pub fn string_map(items: &[(&str, &str)]) -> Value {
    Value::Map(
        items
            .iter()
            .map(|(key, value)| {
                (
                    Value::String((*key).to_owned()),
                    Value::String((*value).to_owned()),
                )
            })
            .collect::<BTreeMap<_, _>>(),
    )
}
