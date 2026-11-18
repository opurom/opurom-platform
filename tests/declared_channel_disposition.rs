//! Disposition audit for declared-only framework channels.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! `channel_contract::FRAMEWORK_CHANNELS` declares all 24 official framework
//! channels, but only five are routed by the bridge. The remaining 19 must
//! have an explicit, source-pinned disposition: either an Opurom IR path
//! already carries the same mechanism, or the channel stays rejected with a
//! typed error and is recorded as an unimplemented gap. A channel may never
//! be silently declared and silently unrouted.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/system_channels.dart:76-79,98,
//!   201-204,210,293-296,322,342,345-348,370-373,413-416,439,450,478,501,
//!   514-517,566,576-579,599,616` — the declarations of the 19 channels.
//! - `packages/flutter/lib/src/services/platform_channel.dart:291-299` —
//!   `MethodChannel`'s default `StandardMethodCodec`.
//! - `packages/flutter/lib/src/services/raw_keyboard.dart:379,757` —
//!   `flutter/keyevent` is the low-level raw-key channel.
//! - `packages/flutter/lib/src/services/hardware_keyboard.dart:603-606` —
//!   `flutter/keyboard` synchronizes the hardware keyboard state.
//! - `packages/flutter/lib/src/services/semantics.dart` — accessibility
//!   events use the `flutter/accessibility` message channel.
//! - `packages/flutter/lib/src/services/system_sound.dart:43-45` —
//!   `SystemSound.play` is a command on `flutter/platform`, not a separate
//!   channel.
//! - Opurom paths: `src/input_bridge.rs` (pointer/gesture input),
//!   `src/ime.rs` (text input), `src/a11y.rs` (accessibility tree),
//!   `src/event_producers.rs` (host keyboard-visibility signals).
//!
//! No ABI/IR change: the dispositions are data in this test; the encoders
//! continue to reject each declared-only channel with a typed error.

use std::collections::BTreeSet;

use opurom_platform::channel_contract::{
    contract, decode_channel_envelope, decode_channel_message, decode_channel_method_call,
    encode_channel_envelope, encode_channel_message, encode_channel_method_call, ChannelCodec,
    ChannelContract, Coverage, FRAMEWORK_CHANNELS,
};
use opurom_platform::platform_channel::{ChannelKind, CodecError, MethodCall, Reply, Value};

/// Why a declared-only channel is not routed in Opurom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Disposition {
    /// The same mechanism is carried by an Opurom IR path.
    IrPath,
    /// Opurom has no corresponding host capability yet.
    Unimplemented,
}

/// One declared-only channel and its pinned disposition.
struct DispositionRow {
    name: &'static str,
    disposition: Disposition,
    /// The Opurom symbol that carries the mechanism, or an empty string.
    path: &'static str,
    pin: &'static str,
}

const DISPOSITIONS: &[DispositionRow] = &[
    DispositionRow {
        name: "flutter/navigation",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:76-79",
    },
    DispositionRow {
        name: "flutter/backgesture",
        disposition: Disposition::IrPath,
        path: "input_bridge.rs::InputRouter",
        pin: "system_channels.dart:98; services/back_gesture.dart",
    },
    DispositionRow {
        name: "flutter/status_bar",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:201-204",
    },
    DispositionRow {
        name: "flutter/processtext",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:210",
    },
    DispositionRow {
        name: "flutter/textinput",
        disposition: Disposition::IrPath,
        path: "ime.rs::ImeStatus",
        pin: "system_channels.dart:293-296; src/ime.rs",
    },
    DispositionRow {
        name: "flutter/scribe",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:322",
    },
    DispositionRow {
        name: "flutter/spellcheck",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:342",
    },
    DispositionRow {
        name: "flutter/undomanager",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:345-348",
    },
    DispositionRow {
        name: "flutter/keyevent",
        disposition: Disposition::IrPath,
        path: "input_bridge.rs::InputRouter",
        pin: "system_channels.dart:370-373; services/raw_keyboard.dart:379,757",
    },
    DispositionRow {
        name: "flutter/accessibility",
        disposition: Disposition::IrPath,
        path: "a11y.rs::map_a11y_tree",
        pin: "system_channels.dart:413-416; src/a11y.rs",
    },
    DispositionRow {
        name: "flutter/skia",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:439",
    },
    DispositionRow {
        name: "flutter/mousecursor",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:450",
    },
    DispositionRow {
        name: "flutter/restoration",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:478",
    },
    DispositionRow {
        name: "flutter/deferredcomponent",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:501",
    },
    DispositionRow {
        name: "flutter/localization",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:514-517",
    },
    DispositionRow {
        name: "flutter/menu",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:566",
    },
    DispositionRow {
        name: "flutter/contextmenu",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:576-579",
    },
    DispositionRow {
        name: "flutter/keyboard",
        disposition: Disposition::IrPath,
        path: "event_producers.rs::keyboard_visibility",
        pin: "system_channels.dart:599; services/hardware_keyboard.dart:603-606",
    },
    DispositionRow {
        name: "flutter/sensitivecontent",
        disposition: Disposition::Unimplemented,
        path: "",
        pin: "system_channels.dart:616",
    },
];

#[test]
fn the_disposition_table_covers_every_declared_only_channel() {
    let declared: BTreeSet<&str> = FRAMEWORK_CHANNELS
        .iter()
        .filter(|entry| entry.coverage == Coverage::Declared)
        .map(|entry| entry.name)
        .collect();
    let rows: BTreeSet<&str> = DISPOSITIONS.iter().map(|row| row.name).collect();
    assert_eq!(declared, rows, "a declared-only channel has no disposition");
    assert_eq!(DISPOSITIONS.len(), 19);

    let mut seen = BTreeSet::new();
    for row in DISPOSITIONS {
        assert!(seen.insert(row.name), "{} appears twice", row.name);
        assert!(!row.pin.is_empty(), "{} has no source pin", row.name);
        match row.disposition {
            Disposition::IrPath => assert!(
                !row.path.is_empty(),
                "{} claims an IR path without naming it",
                row.name
            ),
            Disposition::Unimplemented => assert!(
                row.path.is_empty(),
                "{} is unimplemented but names a path",
                row.name
            ),
        }
    }
}

#[test]
fn the_ir_paths_named_by_the_disposition_table_exist() {
    // Read the Opurom source text instead of asserting symbols through Rust
    // imports: the table is a source-level audit, not a call graph.
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checks = 0;
    for row in DISPOSITIONS {
        if row.disposition != Disposition::IrPath {
            continue;
        }
        let (file, symbol) = row.path.split_once("::").expect("file::symbol");
        let source = std::fs::read_to_string(root.join(file)).expect("read IR module");
        assert!(
            source.contains(symbol),
            "{} has no symbol {} in {}",
            row.name,
            symbol,
            file
        );
        checks += 1;
    }
    assert_eq!(checks, 5, "the known IR-backed channels changed");
}

#[test]
fn every_declared_only_channel_reports_a_typed_rejection() {
    let call = MethodCall {
        method: "anything".to_owned(),
        arguments: Value::Null,
    };
    for row in DISPOSITIONS {
        let entry = contract(row.name).expect("declared channel");
        assert_eq!(entry.coverage, Coverage::Declared, "{}", row.name);
        let code = match entry.kind {
            ChannelKind::BasicMessage => {
                encode_channel_message(row.name, &Value::Null).unwrap_err()
            }
            ChannelKind::Method { .. } | ChannelKind::Event => {
                encode_channel_method_call(row.name, &call).unwrap_err()
            }
        };
        assert_eq!(
            code,
            CodecError::Unsupported("channel has no bridge route"),
            "{} must not be routable",
            row.name
        );
    }
}

#[test]
fn neither_codec_nor_envelope_is_reachable_on_a_declared_only_channel() {
    for row in DISPOSITIONS {
        let entry = contract(row.name).expect("declared channel");
        if entry.kind == ChannelKind::BasicMessage {
            assert_eq!(
                decode_channel_message(row.name, None),
                Err(CodecError::Unsupported("channel has no bridge route")),
                "{}",
                row.name
            );
        } else {
            assert_eq!(
                decode_channel_method_call(row.name, b"irrelevant"),
                Err(CodecError::Unsupported("channel has no bridge route")),
                "{}",
                row.name
            );
            assert_eq!(
                encode_channel_envelope(row.name, &Reply::Success(Value::Null)),
                Err(CodecError::Unsupported("channel has no bridge route")),
                "{}",
                row.name
            );
            assert_eq!(
                decode_channel_envelope(row.name, Some(b"irrelevant")),
                Err(CodecError::Unsupported("channel has no bridge route")),
                "{}",
                row.name
            );
        }
    }
}

#[test]
fn the_disposition_report_is_machine_readable() {
    let mut lines = Vec::new();
    for row in DISPOSITIONS {
        let entry = contract(row.name).expect("declared channel");
        let disposition = match row.disposition {
            Disposition::IrPath => "ir_path",
            Disposition::Unimplemented => "unimplemented",
        };
        lines.push(format!(
            "{}|{}|{}|{}|{}",
            row.name,
            entry.codec.as_str(),
            disposition,
            row.path,
            row.pin
        ));
    }
    lines.sort();
    assert_eq!(lines.len(), 19);
    for line in &lines {
        assert_eq!(
            line.split('|').count(),
            5,
            "a report line has five fields: {line}"
        );
    }

    let ir_paths: Vec<&str> = lines
        .iter()
        .filter(|line| line.contains("|ir_path|"))
        .map(String::as_str)
        .collect();
    assert_eq!(ir_paths.len(), 5);
    // The codec column is read from the implementation contract, so a drift
    // in any declared channel's codec surfaces here.
    let key_event = lines
        .iter()
        .find(|line| line.starts_with("flutter/keyevent|"))
        .expect("keyevent row");
    assert!(key_event.starts_with("flutter/keyevent|json|"));
    let accessibility = lines
        .iter()
        .find(|line| line.starts_with("flutter/accessibility|"))
        .expect("accessibility row");
    assert!(accessibility.starts_with("flutter/accessibility|standard|"));
}

#[test]
fn the_data_classification_catches_missing_and_duplicate_rows() {
    let names: BTreeSet<&str> = DISPOSITIONS.iter().map(|row| row.name).collect();
    assert!(!names.contains("flutter/system"));
    assert!(!names.contains("flutter/platform"));
    assert!(!names.contains("flutter/platform_views"));

    let mut declared: Vec<&ChannelContract> = FRAMEWORK_CHANNELS
        .iter()
        .filter(|entry| entry.coverage == Coverage::Declared)
        .collect();
    for row in DISPOSITIONS {
        let before = declared.len();
        declared.retain(|entry| entry.name != row.name);
        assert_eq!(declared.len(), before - 1, "{} missing", row.name);
    }
    assert!(
        declared.is_empty(),
        "{} declared channel(s) left unclassified",
        declared.len()
    );
}

#[test]
fn codec_columns_stay_in_the_closed_official_set() {
    let allowed = [
        ChannelCodec::Standard,
        ChannelCodec::Json,
        ChannelCodec::String,
    ];
    for row in DISPOSITIONS {
        let codec = contract(row.name).expect("declared channel").codec;
        assert!(
            allowed.contains(&codec),
            "{} has codec {}",
            row.name,
            codec.as_str()
        );
    }
    // The audited set contains no StringCodec channel; it is reserved for
    // the bridged lifecycle channel.
    assert!(DISPOSITIONS
        .iter()
        .all(|row| contract(row.name).expect("declared channel").codec != ChannelCodec::String));
}
