//! Conformance tests for the unified native capability contract.

use opurom_platform::native_capabilities::{
    image_methods, path_methods, preferences_methods, qualified, string_map, unprefixed_key,
    url_methods, Capability, CapabilityHandler, CapabilityTable, ALL_CAPABILITIES,
};
use opurom_platform::platform_channel::{error_codes, MethodCall, Reply, Value};

unsafe fn preference_handler(_state: *mut u8, call: &MethodCall) -> Reply {
    match call.method.as_str() {
        preferences_methods::GET_STRING => {
            let key = match &call.arguments {
                Value::Map(map) => map
                    .get(&Value::String("key".to_owned()))
                    .cloned()
                    .unwrap_or(Value::Null),
                _ => Value::Null,
            };
            Reply::Success(key)
        }
        preferences_methods::SET_STRING => Reply::Success(Value::Bool(true)),
        _ => Reply::PlatformError {
            code: error_codes::UNIMPLEMENTED.to_owned(),
            message: Some("method not in table".to_owned()),
            details: Value::Null,
        },
    }
}

unsafe fn url_handler(_state: *mut u8, call: &MethodCall) -> Reply {
    match call.method.as_str() {
        url_methods::CAN_LAUNCH => Reply::Success(Value::Bool(true)),
        url_methods::LAUNCH => Reply::Success(Value::Bool(false)),
        _ => Reply::PlatformError {
            code: error_codes::UNIMPLEMENTED.to_owned(),
            message: Some("method not in table".to_owned()),
            details: Value::Null,
        },
    }
}

#[test]
fn all_capabilities_are_declared_in_stable_order() {
    assert_eq!(ALL_CAPABILITIES.len(), 14);
    assert_eq!(Capability::from_str("camera"), Some(Capability::Camera));
    assert_eq!(
        Capability::from_str("video_player"),
        Some(Capability::VideoPlayer)
    );
    assert_eq!(ALL_CAPABILITIES[0], Capability::Preferences);
    assert_eq!(
        Capability::from_str("preferences"),
        Some(Capability::Preferences)
    );
    assert_eq!(Capability::from_str("unknown"), None);
    assert!(Capability::Network.unavailable_reason().is_some());
    assert!(Capability::Permissions.unavailable_reason().is_some());
    assert!(Capability::Share.unavailable_reason().is_some());
    assert!(Capability::DeviceInfo.unavailable_reason().is_some());
}

#[test]
fn preference_key_prefix_matches_flutter() {
    // shared_preferences.dart:17,124,131,169-170
    assert_eq!(
        opurom_platform::native_capabilities::prefixed_key("theme"),
        "flutter.theme"
    );
    assert_eq!(unprefixed_key("flutter.theme"), Some("theme"));
    assert_eq!(unprefixed_key("other.theme"), None);
}

#[test]
fn method_tables_match_pinned_plugin_methods() {
    assert!(preferences_methods::all().contains(&"getString"));
    assert!(preferences_methods::all().contains(&"setStringList"));
    assert!(path_methods::all().contains(&"getApplicationDocumentsDirectory"));
    assert!(url_methods::all().contains(&"canLaunch"));
    assert!(image_methods::all().contains(&"pickImage"));
}

#[test]
fn capability_table_dispatches_qualified_calls() {
    let table = CapabilityTable::new()
        .with(CapabilityHandler {
            capability: Capability::Preferences,
            function: preference_handler,
            state: std::ptr::null_mut(),
        })
        .with(CapabilityHandler {
            capability: Capability::UrlLauncher,
            function: url_handler,
            state: std::ptr::null_mut(),
        });
    assert_eq!(table.len(), 2);

    let args = string_map(&[("key", "name")]);
    let reply = unsafe {
        table.call(&qualified(
            Capability::Preferences,
            preferences_methods::GET_STRING,
            args.clone(),
        ))
    };
    assert_eq!(reply, Reply::Success(Value::String("name".to_owned())));

    let reply = unsafe {
        table.call(&qualified(
            Capability::UrlLauncher,
            url_methods::CAN_LAUNCH,
            Value::Null,
        ))
    };
    assert_eq!(reply, Reply::Success(Value::Bool(true)));
}

#[test]
fn missing_capability_is_typed_unimplemented_not_fake_success() {
    let table = CapabilityTable::new();
    for capability in [
        Capability::Network,
        Capability::Permissions,
        Capability::Share,
        Capability::DeviceInfo,
    ] {
        let reply = unsafe { table.call(&qualified(capability, "someMethod", Value::Null)) };
        match reply {
            Reply::PlatformError { code, message, .. } => {
                assert_eq!(code, error_codes::UNIMPLEMENTED);
                let message = message.unwrap_or_default();
                assert!(message.contains("snapshot") || message.contains("not part"));
            }
            other => panic!("expected typed unimplemented, got {other:?}"),
        }
    }
}

#[test]
fn unknown_method_is_never_aliased_to_success() {
    let table = CapabilityTable::new().with(CapabilityHandler {
        capability: Capability::Preferences,
        function: preference_handler,
        state: std::ptr::null_mut(),
    });
    let reply = unsafe {
        table.call(&qualified(
            Capability::Preferences,
            "notAFlutterMethod",
            Value::Null,
        ))
    };
    assert!(matches!(reply, Reply::PlatformError { .. }));
}

#[test]
fn qualified_and_plain_method_names_resolve_to_same_call() {
    let table = CapabilityTable::new().with(CapabilityHandler {
        capability: Capability::Preferences,
        function: preference_handler,
        state: std::ptr::null_mut(),
    });
    let args = string_map(&[("key", "plain")]);
    let plain = unsafe {
        table.call(&MethodCall {
            method: "getString".to_owned(),
            arguments: args.clone(),
        })
    };
    let qualified = unsafe {
        table.call(&qualified(
            Capability::Preferences,
            preferences_methods::GET_STRING,
            args,
        ))
    };
    assert_eq!(plain, qualified);
}
