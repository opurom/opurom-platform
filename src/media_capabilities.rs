//! Media capability contract: camera capture and video playback.
//!
//! Flutter baseline: 3.50-candidate, plugin snapshot `flutter-plugins` at
//! `557d3284ac9dda32a1106bb75be8a23bdccd2f96`.
//!
//! The two official plugins with pinned local source define the method names
//! and argument keys frozen here. A host injects typed handlers through the
//! compile-time capability table; a host without one answers `unimplemented`
//! instead of a fabricated path or player id.
//!
//! Source pins:
//! - `camera/camera_platform_interface/lib/src/method_channel/method_channel_camera.dart:17`
//!   — channel name `plugins.flutter.io/camera`.
//! - `:94-160` — `create` (`cameraName`/`resolutionPreset`/`enableAudio` →
//!   `cameraId`), `initialize` (`cameraId`/`imageFormatGroup`), `dispose`.
//! - `:204-291` — `lockCaptureOrientation`, `takePicture`,
//!   `startVideoRecording` (`maxVideoDuration` ms, `enableStream`),
//!   `stopVideoRecording`, `pauseVideoRecording`, `resumeVideoRecording`.
//! - `:354` — `setFlashMode` (`cameraId`/`mode`).
//! - `video_player/video_player_platform_interface/lib/video_player_platform_interface.dart:42-102`
//!   — `init`/`dispose`/`create`/`setLooping`/`play`/`pause`/`setVolume`/
//!   `seekTo`/`setPlaybackSpeed`/`getPosition`/`setMixWithOthers`.
//! - `video_player/video_player_android/android/src/main/java/io/flutter/plugins/videoplayer/VideoPlayerPlugin.java:114-210`
//!   — native method names (`position` is the native name of `getPosition`).

use crate::framework_channels::unimplemented;
use crate::native_capabilities::Capability;
use crate::platform_channel::{error_codes, MethodCall, Reply, Value};

/// Camera method names on `plugins.flutter.io/camera`.
pub mod camera_methods {
    pub const CHANNEL: &str = "plugins.flutter.io/camera";
    pub const CREATE: &str = "create";
    pub const INITIALIZE: &str = "initialize";
    pub const DISPOSE: &str = "dispose";
    pub const LOCK_ORIENTATION: &str = "lockCaptureOrientation";
    pub const UNLOCK_ORIENTATION: &str = "unlockCaptureOrientation";
    pub const TAKE_PICTURE: &str = "takePicture";
    pub const START_VIDEO: &str = "startVideoRecording";
    pub const STOP_VIDEO: &str = "stopVideoRecording";
    pub const PAUSE_VIDEO: &str = "pauseVideoRecording";
    pub const RESUME_VIDEO: &str = "resumeVideoRecording";
    pub const SET_FLASH: &str = "setFlashMode";

    /// Argument keys (`method_channel_camera.dart:94-354`).
    pub const ARG_CAMERA_ID: &str = "cameraId";
    pub const ARG_CAMERA_NAME: &str = "cameraName";
    pub const ARG_RESOLUTION_PRESET: &str = "resolutionPreset";
    pub const ARG_ENABLE_AUDIO: &str = "enableAudio";
    pub const ARG_IMAGE_FORMAT_GROUP: &str = "imageFormatGroup";
    pub const ARG_ORIENTATION: &str = "orientation";
    pub const ARG_MAX_VIDEO_DURATION: &str = "maxVideoDuration";
    pub const ARG_ENABLE_STREAM: &str = "enableStream";
    pub const ARG_MODE: &str = "mode";

    pub fn all() -> &'static [&'static str] {
        &[
            CREATE,
            INITIALIZE,
            DISPOSE,
            LOCK_ORIENTATION,
            UNLOCK_ORIENTATION,
            TAKE_PICTURE,
            START_VIDEO,
            STOP_VIDEO,
            PAUSE_VIDEO,
            RESUME_VIDEO,
            SET_FLASH,
        ]
    }
}

/// Video player method names (`video_player_platform_interface.dart:42-102`).
pub mod video_methods {
    pub const INIT: &str = "init";
    pub const CREATE: &str = "create";
    pub const DISPOSE: &str = "dispose";
    pub const SET_LOOPING: &str = "setLooping";
    pub const PLAY: &str = "play";
    pub const PAUSE: &str = "pause";
    pub const SET_VOLUME: &str = "setVolume";
    pub const SEEK_TO: &str = "seekTo";
    pub const SET_PLAYBACK_SPEED: &str = "setPlaybackSpeed";
    pub const POSITION: &str = "position";
    pub const SET_MIX_WITH_OTHERS: &str = "setMixWithOthers";

    pub const ARG_TEXTURE_ID: &str = "textureId";
    pub const ARG_LOOPING: &str = "looping";
    pub const ARG_VOLUME: &str = "volume";
    pub const ARG_POSITION_MS: &str = "position";
    pub const ARG_SPEED: &str = "speed";
    pub const ARG_MIX: &str = "mixWithOthers";

    pub fn all() -> &'static [&'static str] {
        &[
            INIT,
            CREATE,
            DISPOSE,
            SET_LOOPING,
            PLAY,
            PAUSE,
            SET_VOLUME,
            SEEK_TO,
            SET_PLAYBACK_SPEED,
            POSITION,
            SET_MIX_WITH_OTHERS,
        ]
    }
}

fn invalid_argument(message: impl Into<String>) -> Reply {
    Reply::PlatformError {
        code: error_codes::INVALID_ARGUMENT.to_owned(),
        message: Some(message.into()),
        details: Value::Null,
    }
}

fn arg_value(arguments: &Value, name: &str) -> Option<Value> {
    let Value::Map(map) = arguments else {
        return None;
    };
    map.get(&Value::String(name.to_owned())).cloned()
}

/// Typed camera backend. The host injects a handler for the method table;
/// validation of the shared `cameraId` argument stays in this crate.
pub struct CameraBackend {
    handler: Box<dyn Fn(&MethodCall) -> Reply>,
}

impl std::fmt::Debug for CameraBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CameraBackend").finish_non_exhaustive()
    }
}

impl CameraBackend {
    pub fn new(handler: impl Fn(&MethodCall) -> Reply + 'static) -> Self {
        Self {
            handler: Box::new(handler),
        }
    }

    /// Call one method from [`camera_methods::all`].
    pub fn call(&self, call: &MethodCall) -> Reply {
        if !camera_methods::all().contains(&call.method.as_str()) {
            return unimplemented(&call.method);
        }
        if call.method == camera_methods::CREATE {
            return match arg_value(&call.arguments, camera_methods::ARG_CAMERA_NAME) {
                Some(Value::String(_)) => (self.handler)(call),
                _ => invalid_argument("create requires a cameraName"),
            };
        }
        match arg_value(&call.arguments, camera_methods::ARG_CAMERA_ID) {
            Some(Value::I32(_)) | Some(Value::I64(_)) => (self.handler)(call),
            _ => invalid_argument("camera method requires a cameraId"),
        }
    }

    /// A [`crate::native_capabilities::CapabilityHandler`] for this backend.
    /// # Safety: `state` must point to a live [`CameraBackend`].
    pub fn handler(&self) -> crate::native_capabilities::CapabilityHandler {
        crate::native_capabilities::CapabilityHandler {
            capability: Capability::Camera,
            function: camera_handler,
            state: self as *const Self as *mut u8,
        }
    }
}

/// `CapabilityHandler` entry point. # Safety: `state` must point to a live
/// [`CameraBackend`].
pub unsafe fn camera_handler(state: *mut u8, call: &MethodCall) -> Reply {
    // SAFETY: documented by the caller and by `CameraBackend::handler`.
    unsafe { &*(state as *const CameraBackend) }.call(call)
}

/// Typed video-player backend with the same validation shape.
pub struct VideoPlayerBackend {
    handler: Box<dyn Fn(&MethodCall) -> Reply>,
}

impl std::fmt::Debug for VideoPlayerBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoPlayerBackend").finish_non_exhaustive()
    }
}

impl VideoPlayerBackend {
    pub fn new(handler: impl Fn(&MethodCall) -> Reply + 'static) -> Self {
        Self {
            handler: Box::new(handler),
        }
    }

    /// Call one method from [`video_methods::all`].
    pub fn call(&self, call: &MethodCall) -> Reply {
        if !video_methods::all().contains(&call.method.as_str()) {
            return unimplemented(&call.method);
        }
        if matches!(
            call.method.as_str(),
            video_methods::INIT | video_methods::SET_MIX_WITH_OTHERS
        ) {
            return (self.handler)(call);
        }
        match arg_value(&call.arguments, video_methods::ARG_TEXTURE_ID) {
            Some(Value::I32(_)) | Some(Value::I64(_)) => (self.handler)(call),
            _ => invalid_argument("video method requires a textureId"),
        }
    }

    /// A [`crate::native_capabilities::CapabilityHandler`] for this backend.
    /// # Safety: `state` must point to a live [`VideoPlayerBackend`].
    pub fn handler(&self) -> crate::native_capabilities::CapabilityHandler {
        crate::native_capabilities::CapabilityHandler {
            capability: Capability::VideoPlayer,
            function: video_handler,
            state: self as *const Self as *mut u8,
        }
    }
}

/// `CapabilityHandler` entry point. # Safety: `state` must point to a live
/// [`VideoPlayerBackend`].
pub unsafe fn video_handler(state: *mut u8, call: &MethodCall) -> Reply {
    // SAFETY: documented by the caller and by `VideoPlayerBackend::handler`.
    unsafe { &*(state as *const VideoPlayerBackend) }.call(call)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_capabilities::{qualified, CapabilityTable};
    use std::collections::BTreeMap;

    fn args(entries: &[(&str, Value)]) -> Value {
        Value::Map(
            entries
                .iter()
                .map(|(k, v)| (Value::String((*k).to_owned()), v.clone()))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    #[test]
    fn camera_methods_follow_the_pinned_channel() {
        let camera = CameraBackend::new(|call| {
            Reply::Success(Value::String(format!("camera:{}", call.method)))
        });
        assert_eq!(camera_methods::CHANNEL, "plugins.flutter.io/camera");
        assert_eq!(camera_methods::all().len(), 11);
        assert_eq!(
            camera.call(&MethodCall {
                method: camera_methods::TAKE_PICTURE.to_owned(),
                arguments: args(&[(camera_methods::ARG_CAMERA_ID, Value::I32(0))]),
            }),
            Reply::Success(Value::String("camera:takePicture".into()))
        );
        assert_eq!(
            camera.call(&MethodCall {
                method: camera_methods::CREATE.to_owned(),
                arguments: args(&[(
                    camera_methods::ARG_CAMERA_NAME,
                    Value::String("rear".into())
                )]),
            }),
            Reply::Success(Value::String("camera:create".into()))
        );
        assert_eq!(
            camera.call(&MethodCall {
                method: camera_methods::TAKE_PICTURE.to_owned(),
                arguments: Value::Null,
            }),
            invalid_argument("camera method requires a cameraId")
        );
        let table = CapabilityTable::new().with(camera.handler());
        assert_eq!(
            unsafe {
                table.call(&qualified(
                    Capability::Camera,
                    camera_methods::DISPOSE,
                    args(&[(camera_methods::ARG_CAMERA_ID, Value::I32(1))]),
                ))
            },
            Reply::Success(Value::String("camera:dispose".into()))
        );
    }

    #[test]
    fn video_methods_follow_the_pinned_api() {
        let video = VideoPlayerBackend::new(|call| {
            Reply::Success(Value::String(format!("video:{}", call.method)))
        });
        assert_eq!(video_methods::all().len(), 11);
        assert_eq!(
            video.call(&MethodCall {
                method: video_methods::POSITION.to_owned(),
                arguments: args(&[(video_methods::ARG_TEXTURE_ID, Value::I32(3))]),
            }),
            Reply::Success(Value::String("video:position".into()))
        );
        assert_eq!(
            video.call(&MethodCall {
                method: video_methods::INIT.to_owned(),
                arguments: Value::Null,
            }),
            Reply::Success(Value::String("video:init".into()))
        );
        assert_eq!(
            video.call(&MethodCall {
                method: video_methods::PLAY.to_owned(),
                arguments: Value::Null,
            }),
            invalid_argument("video method requires a textureId")
        );
        let table = CapabilityTable::new().with(video.handler());
        assert_eq!(
            unsafe {
                table.call(&qualified(
                    Capability::VideoPlayer,
                    video_methods::SET_LOOPING,
                    args(&[(video_methods::ARG_TEXTURE_ID, Value::I32(2))]),
                ))
            },
            Reply::Success(Value::String("video:setLooping".into()))
        );
    }
}
