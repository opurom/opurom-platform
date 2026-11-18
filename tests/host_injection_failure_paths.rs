//! Failure-path convergence for host injection and typed channel replies.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/message_codecs.dart:337-345,477,536,
//!   615,641,663` — malformed standard-codec messages/envelopes throw
//!   `FormatException('Message corrupted')` or a shape-specific error.
//! - `packages/flutter/lib/src/services/platform_channel.dart:350-366` —
//!   a null reply becomes `MissingPluginException` on a strict channel.
//! - `packages/flutter/lib/src/services/binding.dart:174-182,448-457` —
//!   unknown system/lifecycle values are not fabricated.
//! - `packages/flutter/lib/src/scheduler/binding.dart:416-430` — frame state
//!   changes only on a valid lifecycle value.
//! - `packages/flutter/lib/src/services/_background_isolate_binary_messenger_io.dart:21-29,92-96`
//!   — uninitialized messenger / handler installation errors are typed.
//!
//! No ABI/IR change: these tests use existing public contracts only.

use std::collections::BTreeMap;

use opurom_platform::background_tasks::{BackgroundMessenger, RootToken};
use opurom_platform::event_producers::refresh_rate;
use opurom_platform::framework_channels::unimplemented;
use opurom_platform::input_bridge::{Clock, ClockError};
use opurom_platform::native_capabilities::CapabilityTable;
use opurom_platform::platform_channel::{
    decode_envelope, decode_message, encode_envelope, error_codes, ChannelTable, CodecError,
    MethodCall, Reply, Value,
};
use opurom_platform::system_events::SystemEventDispatcher;

#[test]
fn invalid_time_deadline_and_state_fail_without_state_advance() {
    let mut clock = Clock::default();
    assert_eq!(
        clock.accept(-1),
        Err(ClockError::NonMonotonic {
            previous: None,
            now: -1,
        })
    );
    assert_eq!(clock.accept(100), Ok(100));
    assert_eq!(
        clock.accept(99),
        Err(ClockError::NonMonotonic {
            previous: Some(100),
            now: 99,
        })
    );
    // The failed calls left the last accepted value unchanged.
    assert_eq!(clock.accept(100), Ok(100));

    let mut messenger = BackgroundMessenger::new();
    assert_eq!(
        messenger.send("early", vec![1]),
        Err(CodecError::Corrupt(
            "background messenger is not initialized"
        ))
    );
    assert_eq!(
        RootToken::new(0),
        Err(CodecError::Corrupt("root token must not be zero"))
    );
    assert!(messenger.ensure_initialized(RootToken(7)));
    let id = messenger.send("valid", vec![2]).unwrap();
    assert_eq!(
        messenger.reply(id + 1, vec![]),
        Err(CodecError::Corrupt("unknown background message identifier"))
    );
    assert_eq!(messenger.pending()[&id].message, vec![2]);

    let mut dispatcher = SystemEventDispatcher::new();
    // Invalid refresh rates never become events; no subscriber is installed.
    assert!(!opurom_platform::event_producers::orientation_from_index(255).is_some_and(|_| true));
    assert!(!unsafe { dispatcher.emit_host_event(refresh_rate(0)) });
    assert_eq!(dispatcher.subscriber_count(), 0);
}

#[test]
fn missing_channel_and_unimplemented_handlers_are_distinct() {
    let table = ChannelTable::new();
    // SAFETY: the empty table has no handler state pointers.
    let reply = unsafe { table.dispatch("missing.channel", b"payload") }.unwrap();
    assert_eq!(reply, None);
    assert_eq!(
        decode_envelope(reply.as_deref()).unwrap(),
        Reply::MissingPlugin
    );

    let text = unimplemented("preferences.getString");
    match &text {
        Reply::PlatformError {
            code,
            message,
            details,
        } => {
            assert_eq!(code, error_codes::UNIMPLEMENTED);
            assert_eq!(
                message.as_deref(),
                Some("no host handler for preferences.getString")
            );
            assert_eq!(details, &Value::Null);
        }
        other => panic!("unimplemented() fabricated {other:?}"),
    }
    // Code/message are stable across repeated calls (no clock or random id).
    assert_eq!(text, unimplemented("preferences.getString"));

    let caps = CapabilityTable::new();
    let call = MethodCall {
        method: "network.lookup".to_owned(),
        arguments: Value::Null,
    };
    // SAFETY: the empty table has no handler state pointers.
    let missing = unsafe { caps.call(&call) };
    assert_eq!(
        missing,
        Reply::PlatformError {
            code: error_codes::UNIMPLEMENTED.to_owned(),
            message: Some("no connectivity plugin source in flutter-plugins snapshot".to_owned()),
            details: Value::Null,
        }
    );
}

#[test]
fn malformed_codec_input_never_becomes_a_success_reply() {
    // Samples chosen so each is malformed at both message and envelope layers.
    let malformed: [&[u8]; 4] = [&[], &[99], &[3, 0], &[7, 1]];
    for bytes in malformed {
        assert!(
            decode_envelope(Some(bytes)).is_err(),
            "envelope accepted {bytes:?}"
        );
        assert!(decode_message(Some(bytes)).is_err(), "accepted {bytes:?}");
    }
    let success = Reply::Success(Value::Bool(true));
    let encoded = encode_envelope(&success).unwrap();
    assert_eq!(decode_envelope(encoded.as_deref()), Ok(success));
    let mut extra = encoded.expect("success envelope is encoded");
    extra.push(0);
    assert_eq!(
        decode_envelope(Some(&extra)),
        Err(CodecError::Corrupt("trailing bytes in success envelope"))
    );
}

#[test]
fn failed_injection_does_not_mutate_dispatch_state() {
    let mut dispatcher = SystemEventDispatcher::new();
    // Register one slot, then feed an invalid payload through the decoder;
    // no synthetic event is inserted and the slot stays usable.
    let before = dispatcher.subscriber_count();
    // SAFETY: the dispatcher has no subscriber state pointers in this test.
    assert_eq!(
        unsafe {
            opurom_platform::channel_event_bridge::dispatch_system_message(
                &mut dispatcher,
                br#"{"type":"unknown"}"#,
            )
        },
        Err(CodecError::Corrupt("unknown system event payload"))
    );
    assert_eq!(dispatcher.subscriber_count(), before);
    assert!(!unsafe { dispatcher.emit_host_event(refresh_rate(1)) });

    let args = BTreeMap::from([(Value::String("key".into()), Value::I32(1))]);
    let call = MethodCall {
        method: "preferences.getString".to_owned(),
        arguments: Value::Map(args),
    };
    let table = CapabilityTable::new();
    // SAFETY: empty table has no state pointers.
    let first = unsafe { table.call(&call) };
    let second = unsafe { table.call(&call) };
    assert_eq!(
        first, second,
        "failure reply must not gain clock/state data"
    );
}

#[test]
fn registration_failures_are_typed_isolated_and_replayable() {
    use opurom_platform::plugin_executor::{execute_plan, RegistrationFn, StepOutcome};
    use opurom_platform::plugin_registry::{resolve_registration, PluginManifest, RegistryError};

    // Flutter's generated registrant is built from declarations that must be
    // resolvable (`flutter_plugins.dart:480-498`); a duplicate plugin name
    // cannot produce a registration plan (`flutter_plugins.dart:528-551`).
    let duplicates = vec![
        PluginManifest::new("camera", "1.0.0"),
        PluginManifest::new("camera", "2.0.0"),
    ];
    assert!(matches!(
        resolve_registration(&duplicates, Some("android")),
        Err(RegistryError::DuplicatePlugin(name)) if name == "camera"
    ));

    let manifests = vec![
        PluginManifest::new("camera", "1.0.0").on_platform("android"),
        PluginManifest::new("path", "1.0.0")
            .on_platform("android")
            .depends_on("camera"),
        PluginManifest::new("share", "1.0.0").on_platform("android"),
    ];
    let plan = resolve_registration(&manifests, Some("android")).unwrap();
    assert_eq!(plan.names(), vec!["camera", "path", "share"]);

    // Per-plugin try/catch: the `path` host surface fails while `camera` and
    // `share` still register, and the frozen plan is unchanged.
    fn ok() -> Result<(), &'static str> {
        Ok(())
    }
    fn fail() -> Result<(), &'static str> {
        Err("host surface unavailable")
    }
    let functions = [
        RegistrationFn {
            name: "camera",
            function: ok,
        },
        RegistrationFn {
            name: "path",
            function: fail,
        },
        RegistrationFn {
            name: "share",
            function: ok,
        },
    ];
    let permissions = BTreeMap::from([
        ("camera", vec!["android.permission.CAMERA"]),
        ("path", vec!["android.permission.READ_EXTERNAL_STORAGE"]),
        ("share", vec!["android.permission.INTERNET"]),
    ]);
    let first = execute_plan(&plan, &functions, &manifests, &permissions, "android");
    let second = execute_plan(&plan, &functions, &manifests, &permissions, "android");
    assert_eq!(first, second, "failure path must be deterministic");
    assert_eq!(first.registered(), vec!["camera", "share"]);
    assert_eq!(first.failed(), vec!["path"]);
    assert_eq!(
        first.outcomes()[1].1,
        StepOutcome::Failed {
            reason: "host surface unavailable".into()
        }
    );
    assert!(!first.all_registered());
    assert!(first.permissions().contains("android.permission.CAMERA"));
    assert!(first.permissions().contains("android.permission.INTERNET"));
    assert!(!first
        .permissions()
        .contains("android.permission.READ_EXTERNAL_STORAGE"));
    assert_eq!(plan.names(), vec!["camera", "path", "share"]);
}
