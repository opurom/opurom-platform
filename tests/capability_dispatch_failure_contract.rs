//! Integration test: frozen `invalid_argument` semantics on the
//! `CapabilityTable::call` dispatch path.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! `CapabilityTable::call` is the single entry point through which Opurom
//! dispatches capability methods. A malformed call to a capability with a
//! frozen official wire shape must be rejected there with the same typed
//! error as a malformed Dart `MethodCall`, while an unimplemented capability
//! must still answer `unimplemented` for a well-shaped call.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/message_codecs.dart:467` —
//!   unsupported values raise `ArgumentError.value(value)`.
//! - `packages/flutter/lib/src/services/message_codecs.dart:608-616` —
//!   a method call must decode to a string method + value arguments with no
//!   trailing bytes.
//! - `packages/flutter/lib/src/services/message_codecs.dart:628-663` —
//!   standard error envelopes carry `(code, message, details[, stacktrace])`.
//! - `packages/flutter/lib/src/services/platform_channel.dart:359-364` —
//!   a null reply is `MissingPluginException`, not an `unimplemented` error.
//! - `packages/flutter/lib/src/services/platform_channel.dart:600-613` —
//!   handler exceptions become error envelopes.
//! - `packages/flutter/lib/src/services/binding.dart:448-457,523-528` —
//!   framework method payload shape examples (`AppLifecycleState.*`,
//!   `System.exitApplication`).
//!
//! No ABI/IR change: all assertions use the existing `MethodCall`, `Reply`
//! and `CapabilityTable` types.

use std::collections::BTreeMap;

use opurom_platform::native_capabilities::{Capability, CapabilityHandler, CapabilityTable};
use opurom_platform::platform_channel::{
    decode_envelope, encode_envelope, error_codes, MethodCall, Reply, Value,
};

fn qualified(method: &str, arguments: Value) -> MethodCall {
    MethodCall {
        method: format!("permissions.{method}"),
        arguments,
    }
}

fn share_call(arguments: Value) -> MethodCall {
    MethodCall {
        method: "share.Share.invoke".to_owned(),
        arguments,
    }
}

fn noop(_state: *mut u8, _call: &MethodCall) -> Reply {
    Reply::Success(Value::Null)
}

#[test]
fn a_malformed_frozen_call_is_rejected_before_dispatch() {
    let mut calls = Vec::new();
    unsafe fn collect(state: *mut u8, call: &MethodCall) -> Reply {
        // SAFETY: state points to a live Vec owned by the test.
        unsafe { &mut *(state as *mut Vec<MethodCall>) }.push(call.clone());
        Reply::Success(Value::Bool(true))
    }
    let table = CapabilityTable::new().with(CapabilityHandler {
        capability: Capability::Share,
        function: collect,
        state: &mut calls as *mut Vec<MethodCall> as *mut u8,
    });

    let reply = unsafe { table.call(&share_call(Value::I32(1))) };
    assert_eq!(
        reply,
        Reply::PlatformError {
            code: error_codes::INVALID_ARGUMENT.to_owned(),
            message: Some("Share.invoke: frozen method takes a bare string".to_owned()),
            details: Value::Null,
        }
    );
    assert!(
        calls.is_empty(),
        "a malformed call must never reach the handler"
    );
}

#[test]
fn a_malformed_map_is_rejected_with_the_shared_error_template() {
    let table = CapabilityTable::new().with(CapabilityHandler {
        capability: Capability::Permissions,
        function: noop,
        state: std::ptr::null_mut(),
    });
    let cases = [
        Value::Null,
        Value::String("cancelable".to_owned()),
        Value::Map(BTreeMap::from([(
            Value::String("type".to_owned()),
            Value::String("required".to_owned()),
        )])),
    ];
    for arguments in cases {
        let reply = unsafe { table.call(&qualified("System.exitApplication", arguments)) };
        let Reply::PlatformError {
            code,
            message,
            details,
        } = reply
        else {
            panic!("expected a typed error: {reply:?}");
        };
        assert_eq!(code, error_codes::INVALID_ARGUMENT);
        assert!(
            message
                .as_deref()
                .unwrap_or("")
                .starts_with("System.exitApplication: "),
            "{message:?}"
        );
        assert_eq!(details, Value::Null);
    }
}

#[test]
fn a_well_shaped_gap_call_stays_unimplemented() {
    // No handler: a well-shaped GAP call must still report `unimplemented`,
    // never `invalid_argument` and never a fake success.
    let empty = CapabilityTable::new();
    let valid_exit = Value::Map(BTreeMap::from([
        (
            Value::String("type".to_owned()),
            Value::String("cancelable".to_owned()),
        ),
        (Value::String("exitCode".to_owned()), Value::I32(0)),
    ]));
    let reply = unsafe { empty.call(&qualified("System.exitApplication", valid_exit.clone())) };
    assert_eq!(
        reply,
        Reply::PlatformError {
            code: error_codes::UNIMPLEMENTED.to_owned(),
            message: Some("no permission plugin source in the local snapshot".to_owned()),
            details: Value::Null,
        }
    );
    assert_ne!(reply, Reply::Success(Value::Null));

    // Even if a host has injected a handler, a frozen GAP capability stays
    // unimplemented: only a real native backend can change that.
    let table = CapabilityTable::new().with(CapabilityHandler {
        capability: Capability::Permissions,
        function: noop,
        state: std::ptr::null_mut(),
    });
    let reply = unsafe { table.call(&qualified("System.exitApplication", valid_exit)) };
    assert_eq!(
        reply,
        Reply::PlatformError {
            code: error_codes::UNIMPLEMENTED.to_owned(),
            message: Some("no permission plugin source in the local snapshot".to_owned()),
            details: Value::Null,
        }
    );
}

#[test]
fn a_well_shaped_non_gap_call_is_dispatched_normally() {
    let table = CapabilityTable::new().with(CapabilityHandler {
        capability: Capability::Preferences,
        function: noop,
        state: std::ptr::null_mut(),
    });
    // No frozen contract applies to preferences, so dispatch proceeds.
    let call = MethodCall {
        method: "preferences.getString".to_owned(),
        arguments: Value::String("key".to_owned()),
    };
    assert_eq!(
        unsafe { table.call(&call) },
        Reply::Success(Value::Null),
        "the new shape gate must not intercept unrelated capabilities"
    );
}

#[test]
fn the_typed_error_survives_the_standard_envelope_round_trip() {
    let table = CapabilityTable::new();
    let reply = unsafe { table.call(&share_call(Value::I32(1))) };
    let encoded = encode_envelope(&reply)
        .expect("error encodes")
        .expect("error has bytes");
    assert_eq!(encoded[0], 1, "the leading byte marks an error envelope");
    assert_eq!(
        decode_envelope(Some(&encoded)),
        Ok(reply),
        "code, message and details must survive the wire"
    );
}

#[test]
fn shape_gating_is_pure_replayable_and_state_free() {
    let table = CapabilityTable::new().with(CapabilityHandler {
        capability: Capability::Permissions,
        function: noop,
        state: std::ptr::null_mut(),
    });
    let bad = qualified("System.exitApplication", Value::Null);
    let first = unsafe { table.call(&bad) };
    for _ in 0..10 {
        assert_eq!(unsafe { table.call(&bad) }, first);
    }
    assert_eq!(table.len(), 1, "shape failures do not mutate the table");

    // An unknown method is still `unimplemented` rather than being
    // misclassified as an invalid argument of a frozen method.
    let unknown = qualified("notAFrozenMethod", Value::Null);
    let reply = unsafe { table.call(&unknown) };
    let Reply::PlatformError { code, .. } = reply else {
        panic!("unknown methods must be typed errors: {reply:?}");
    };
    assert_eq!(code, error_codes::UNIMPLEMENTED);
}
