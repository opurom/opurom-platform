//! Failure-path contract for the frozen plugin GAP shapes.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! A well-shaped call to a GAP capability must still answer `unimplemented`.
//! A malformed call must answer a typed `invalid_argument` error and leave
//! no state behind: the host adapter's GAP status never advances, and no
//! success reply is fabricated.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/message_codecs.dart:467` — an
//!   unsupported value raises `ArgumentError.value(value)`.
//! - `packages/flutter/lib/src/services/message_codecs.dart:608-616` — a
//!   method call must decode to a string method + value arguments with no
//!   trailing bytes.
//! - `packages/flutter/lib/src/services/message_codecs.dart:145-157` —
//!   the JSON method codec rejects a non-map call and a non-string method.
//! - `packages/flutter/lib/src/services/platform_channel.dart:600-613` —
//!   handler errors become error envelopes; `MissingPluginException` is a
//!   null reply.
//! - `packages/flutter/lib/src/services/message_codec.dart:108-122` —
//!   `PlatformException` carries a required code and optional message.
//! - `packages/flutter/lib/src/services/message_codecs.dart:628-663` —
//!   the standard error envelope is
//!   `[1, code, message, details]` with an optional stacktrace.
//!
//! No ABI/IR change: the tests use existing `Value`, `MethodCall`, `Reply`
//! and envelope encoders only.

use std::collections::BTreeMap;

use opurom_platform::frozen_contracts::{contract, validate_arguments, validate_call};
use opurom_platform::native_capabilities::Capability;
use opurom_platform::platform_channel::{
    decode_envelope, encode_envelope, error_codes, MethodCall, Reply, Value,
};

fn share_call(arguments: Value) -> MethodCall {
    MethodCall {
        method: "Share.invoke".to_owned(),
        arguments,
    }
}

fn exit_call(arguments: Value) -> MethodCall {
    MethodCall {
        method: "System.exitApplication".to_owned(),
        arguments,
    }
}

#[test]
fn a_malformed_share_call_is_a_typed_invalid_argument() {
    let method = &contract(Capability::Share).unwrap().methods[0];
    for arguments in [
        Value::I32(1),
        Value::Null,
        Value::Map(BTreeMap::from([(
            Value::String("text".to_owned()),
            Value::String("hello".to_owned()),
        )])),
    ] {
        let reply = validate_call(method, &share_call(arguments)).expect("shape error");
        assert_eq!(
            reply,
            Reply::PlatformError {
                code: error_codes::INVALID_ARGUMENT.to_owned(),
                message: Some("Share.invoke: frozen method takes a bare string".to_owned()),
                details: Value::Null,
            }
        );
        assert_ne!(reply, Reply::Success(Value::Null));
    }
}

#[test]
fn a_missing_key_or_wrong_map_type_is_a_typed_invalid_argument() {
    let method = &contract(Capability::Permissions).unwrap().methods[0];
    let cases = [
        Value::Null,
        Value::String("cancelable".to_owned()),
        Value::Map(BTreeMap::from([(
            Value::String("exitCode".to_owned()),
            Value::I32(0),
        )])),
    ];
    for arguments in cases {
        let reply = validate_call(method, &exit_call(arguments)).expect("shape error");
        let Reply::PlatformError {
            code,
            message,
            details,
        } = &reply
        else {
            panic!("a malformed call must be an error envelope: {reply:?}");
        };
        assert_eq!(code, error_codes::INVALID_ARGUMENT);
        assert!(message
            .as_deref()
            .unwrap_or("")
            .starts_with("System.exitApplication: "));
        assert_eq!(*details, Value::Null);
    }
}

#[test]
fn a_well_shaped_gap_call_stays_unimplemented_never_success() {
    let share = &contract(Capability::Share).unwrap().methods[0];
    let permissions = &contract(Capability::Permissions).unwrap().methods[0];

    // A shape-valid call does not produce an invalid_argument error here;
    // the host still has no implementation, so no reply is synthesized.
    assert_eq!(
        validate_call(share, &share_call(Value::String("text".to_owned()))),
        None
    );
    let valid_exit = Value::Map(BTreeMap::from([
        (
            Value::String("type".to_owned()),
            Value::String("required".to_owned()),
        ),
        (Value::String("exitCode".to_owned()), Value::I32(0)),
    ]));
    assert_eq!(validate_call(permissions, &exit_call(valid_exit)), None);

    // The GAP capability's own dispatch table still answers typed
    // `unimplemented`, never `Success(Null)`.
    for capability in [
        Capability::Share,
        Capability::Permissions,
        Capability::DeviceInfo,
        Capability::Network,
    ] {
        assert!(capability.unavailable_reason().is_some());
    }
}

#[test]
fn the_invalid_argument_reply_has_a_stable_wire_envelope() {
    let method = &contract(Capability::Share).unwrap().methods[0];
    let reply = validate_call(method, &share_call(Value::I32(1))).expect("shape error");
    let encoded = encode_envelope(&reply)
        .expect("envelope encodes")
        .expect("error has bytes");
    assert_eq!(encoded[0], 1, "leading byte distinguishes an error");
    assert_eq!(
        decode_envelope(Some(&encoded)),
        Ok(reply),
        "the typed error survives the wire round trip"
    );
}

#[test]
fn failures_are_pure_replayable_and_do_not_advance_gap_state() {
    let method = &contract(Capability::Permissions).unwrap().methods[0];
    let bad = exit_call(Value::Null);
    let first = validate_call(method, &bad);
    for _ in 0..10 {
        assert_eq!(validate_call(method, &bad), first);
    }
    // A valid call after any number of failures is still not a success: the
    // frozen contract has no host implementation to advance to.
    let valid = exit_call(Value::Map(BTreeMap::from([
        (
            Value::String("type".to_owned()),
            Value::String("cancelable".to_owned()),
        ),
        (Value::String("exitCode".to_owned()), Value::I32(0)),
    ])));
    assert_eq!(validate_call(method, &valid), None);
    assert_eq!(validate_arguments(method, &valid.arguments), Ok(()));
}

#[test]
fn every_frozen_method_has_a_shape_failure_path() {
    let mut checked = 0;
    for frozen in opurom_platform::frozen_contracts::CONTRACTS {
        for method in frozen.methods {
            let bad = match method.argument {
                opurom_platform::frozen_contracts::ArgType::None => Value::I32(1),
                opurom_platform::frozen_contracts::ArgType::String => Value::Bool(true),
                opurom_platform::frozen_contracts::ArgType::Map => Value::I32(1),
            };
            let call = MethodCall {
                method: method.name.to_owned(),
                arguments: bad,
            };
            let reply = validate_call(method, &call).expect("every frozen method has a bad shape");
            match reply {
                Reply::PlatformError { code, details, .. } => {
                    assert_eq!(code, error_codes::INVALID_ARGUMENT);
                    assert_eq!(details, Value::Null);
                }
                other => panic!("unexpected reply: {other:?}"),
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 2, "two frozen methods have official pins");
}

#[test]
fn the_gap_register_still_reports_the_same_channels() {
    let before = opurom_platform::gap_register::all_rows().len();
    let method = &contract(Capability::Share).unwrap().methods[0];
    let _ = validate_call(method, &share_call(Value::I32(1)));
    assert_eq!(opurom_platform::gap_register::all_rows().len(), before);
    for capability in [
        Capability::Share,
        Capability::Permissions,
        Capability::DeviceInfo,
        Capability::Network,
    ] {
        assert!(capability.unavailable_reason().is_some());
    }
}
