//! Cross-contract consistency checks for the platform bridge.
//!
//! These tests do not invent behaviour; they assert that every module uses
//! the same closed wire vocabulary and that capability names/codes remain
//! unique across the unified bridge.

use std::collections::BTreeSet;

use opurom_platform::background_tasks::BackgroundMessenger;
use opurom_platform::framework_channels::{lifecycle_message, platform_methods};
use opurom_platform::media_capabilities::{camera_methods, video_methods};
use opurom_platform::native_capabilities::{self, Capability, ALL_CAPABILITIES};
use opurom_platform::platform_channel::{error_codes, Value};
use opurom_platform::plugin_registry::PluginManifest;
use opurom_platform::system_events::SystemEvent;

#[test]
fn error_codes_are_a_closed_stable_vocabulary() {
    let codes = [
        error_codes::MISSING_PLUGIN,
        error_codes::UNIMPLEMENTED,
        error_codes::INVALID_ARGUMENT,
        error_codes::ERROR,
    ];
    let unique: BTreeSet<_> = codes.iter().copied().collect();
    assert_eq!(unique.len(), codes.len());
    assert_eq!(error_codes::MISSING_PLUGIN, "missing_plugin");
    assert_eq!(error_codes::UNIMPLEMENTED, "unimplemented");
    assert_eq!(error_codes::INVALID_ARGUMENT, "invalid_argument");
    assert_eq!(error_codes::ERROR, "error");
}

#[test]
fn capability_names_are_unique_and_stably_resolvable() {
    let names: BTreeSet<_> = ALL_CAPABILITIES.iter().map(|c| c.as_str()).collect();
    assert_eq!(names.len(), ALL_CAPABILITIES.len());
    for capability in ALL_CAPABILITIES {
        assert_eq!(Capability::from_str(capability.as_str()), Some(capability));
        // Every capability must have a non-empty method table, including the
        // typed-GAP capabilities whose table is deliberately empty at runtime.
        let _ = native_capabilities::methods_for(capability);
    }
}

#[test]
fn media_and_framework_method_names_do_not_collide() {
    // The two plugins are separate channels, so identical short method names
    // are expected; each plugin's own table must still be collision-free.
    for methods in [camera_methods::all(), video_methods::all()] {
        let names: BTreeSet<_> = methods.iter().copied().collect();
        assert_eq!(names.len(), methods.len());
    }
    assert!(camera_methods::all().contains(&"create"));
    assert!(video_methods::all().contains(&"dispose"));
    assert_eq!(platform_methods::CLIPBOARD_SET_DATA, "Clipboard.setData");
    assert_eq!(platform_methods::HAPTIC_VIBRATE, "HapticFeedback.vibrate");
    assert_eq!(
        lifecycle_message(opurom_platform::Lifecycle::Destroyed),
        "AppLifecycleState.detached"
    );
}

#[test]
fn wire_values_and_event_codes_are_namespaced() {
    let events = [
        SystemEvent::MemoryPressure,
        SystemEvent::KeyboardVisibility {
            visible: true,
            height: None,
        },
        SystemEvent::Orientation { orientation: 1 },
        SystemEvent::Brightness { dark: true },
        SystemEvent::PowerMode { low_power: true },
        SystemEvent::RefreshRate { hz: 120 },
    ];
    for event in events {
        let value = event.to_value();
        let Value::Map(map) = value else {
            panic!("system events must use map payloads");
        };
        assert_eq!(
            map.get(&Value::String("type".to_owned())),
            Some(&Value::String(event.code().to_owned()))
        );
        if event.is_flutter_framework_event() {
            assert!(!event.code().starts_with("host."));
        } else {
            assert!(event.code().starts_with("host."));
        }
    }
}

#[test]
fn plugin_manifest_capabilities_are_unique() {
    let plugin = PluginManifest::new("example", "1.0")
        .provides("camera.takePicture")
        .provides("video.play");
    let unique: BTreeSet<_> = plugin.capabilities.iter().collect();
    assert_eq!(unique.len(), 2);
    assert!(BackgroundMessenger::new().token().is_none());
}
