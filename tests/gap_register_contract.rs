//! Integration contract for the unified host-injection GAP register.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! The register merges three previously separate audits into one consumer
//! contract for multi-platform host injection. This test verifies that the
//! merged artifact:
//! - covers all 19 declared-only framework channels plus the 4 capability
//!   GAPs from the local plugin snapshot;
//! - preserves each row's official channel name, codec, disposition,
//!   platform scope and source anchor;
//! - is deterministic (sorted, unique, stable bytes);
//! - answers a platform-scoped host query correctly.
//!
//! The generated artifact is `tests/fixtures/gap-register.psv` (pipe-separated
//! values). Host adapters consume it directly; no runtime registry or parser
//! is introduced into the library.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/system_channels.dart:68-616` —
//!   every framework channel declaration and platform note.
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:366-386` — the six
//!   generated dependency-manifest platform keys.
//! - `packages/flutter/lib/src/services/platform_channel.dart:291-299` —
//!   default `StandardMethodCodec` for a `MethodChannel`.

use std::collections::BTreeSet;

use opurom_platform::channel_contract::{
    contract, decode_channel_envelope, encode_channel_envelope, Coverage,
};
use opurom_platform::gap_register::{all_rows, row, Disposition, GapKind, GAPS, PLATFORM_KEYS};
use opurom_platform::platform_channel::{CodecError, Reply, Value};

const ARTIFACT: &str = include_str!("fixtures/gap-register.psv");

#[test]
fn the_register_fixture_is_exact_and_replayable() {
    let mut expected: Vec<String> = all_rows().iter().map(|row| row.as_line()).collect();
    expected.sort();
    let actual: Vec<&str> = ARTIFACT.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(expected, actual, "the machine-readable artifact drifted");

    let mut seen = BTreeSet::new();
    for line in &actual {
        assert_eq!(line.split('|').count(), 7, "bad line: {line}");
        assert!(seen.insert(*line), "duplicate line: {line}");
    }
}

#[test]
fn the_register_covers_every_declared_only_framework_channel() {
    let declared: BTreeSet<&str> = opurom_platform::channel_contract::FRAMEWORK_CHANNELS
        .iter()
        .filter(|entry| entry.coverage == Coverage::Declared)
        .map(|entry| entry.name)
        .collect();
    let registered: BTreeSet<&str> = GAPS.iter().map(|row| row.channel).collect();
    assert_eq!(declared, registered);
    assert_eq!(GAPS.len(), 19);
}

#[test]
fn each_row_names_an_official_channel_with_the_matching_codec() {
    for gap in all_rows() {
        assert!(!gap.codec.as_str().is_empty(), "{}", gap.channel);
        assert!(!gap.pin.is_empty(), "{}", gap.channel);
        if gap.channel.starts_with("flutter/") {
            let entry = contract(gap.channel).expect("official framework channel");
            assert_eq!(entry.codec, gap.codec, "{} codec drifted", gap.channel);
            assert_eq!(entry.coverage, Coverage::Declared, "{}", gap.channel);
        } else {
            assert!(
                gap.channel.starts_with("plugins.flutter.io/"),
                "{} is outside both official namespaces",
                gap.channel
            );
        }
    }
}

#[test]
fn the_register_distinguishes_ir_paths_from_real_gaps() {
    let ir_paths: Vec<&str> = GAPS
        .iter()
        .filter(|row| row.disposition == Disposition::IrPath)
        .map(|row| row.channel)
        .collect();
    assert_eq!(
        ir_paths,
        vec![
            "flutter/accessibility",
            "flutter/backgesture",
            "flutter/keyevent",
            "flutter/keyboard",
            "flutter/textinput",
        ]
    );
    for gap in GAPS {
        if gap.disposition == Disposition::IrPath {
            assert_eq!(gap.gap, GapKind::IrPath, "{}", gap.channel);
            assert!(!gap.ir_path.is_empty(), "{}", gap.channel);
        } else {
            assert_ne!(gap.gap, GapKind::IrPath, "{}", gap.channel);
        }
    }
}

#[test]
fn all_surfaces_stay_typed_rejected_on_the_wire() {
    for gap in GAPS {
        if gap.disposition != Disposition::Unimplemented {
            continue;
        }
        assert_eq!(
            encode_channel_envelope(gap.channel, &Reply::Success(Value::Null)),
            Err(CodecError::Unsupported("channel has no bridge route")),
            "{} must not be routable",
            gap.channel
        );
        assert_eq!(
            decode_channel_envelope(gap.channel, Some(b"irrelevant")),
            Err(CodecError::Unsupported("channel has no bridge route")),
            "{} must not be routable",
            gap.channel
        );
    }
}

#[test]
fn a_host_adapter_can_query_the_register_by_platform() {
    let android: Vec<&str> = all_rows()
        .iter()
        .filter(|gap| gap.needs_host_implementation("android"))
        .map(|gap| gap.channel)
        .collect();
    assert!(android.contains(&"flutter/navigation"));
    assert!(android.contains(&"flutter/processtext"));
    assert!(android.contains(&"flutter/sensitivecontent"));
    assert!(!android.contains(&"flutter/contextmenu"), "web-only");
    assert!(!android.contains(&"flutter/status_bar"), "iOS-only");
    assert!(!android.contains(&"flutter/textinput"), "IR path");

    let ios: Vec<&str> = all_rows()
        .iter()
        .filter(|gap| gap.needs_host_implementation("ios"))
        .map(|gap| gap.channel)
        .collect();
    assert!(ios.contains(&"flutter/status_bar"));
    assert!(ios.contains(&"flutter/spellcheck"));
    assert!(!ios.contains(&"flutter/processtext"), "Android-only");

    let macos: Vec<&str> = all_rows()
        .iter()
        .filter(|gap| gap.needs_host_implementation("macos"))
        .map(|gap| gap.channel)
        .collect();
    assert!(macos.contains(&"flutter/menu"));
    assert!(macos.contains(&"flutter/mousecursor"));
    assert!(!macos.contains(&"flutter/sensitivecontent"), "Android-only");
}

#[test]
fn the_register_has_no_unknown_platforms_or_empty_reasons() {
    let mut keys: Vec<&str> = PLATFORM_KEYS.to_vec();
    keys.push("fuchsia");
    keys.sort();
    keys.dedup();
    for gap in all_rows() {
        assert!(!gap.platforms.is_empty(), "{}", gap.channel);
        for platform in gap.platforms {
            assert!(
                keys.contains(platform),
                "{} names {}",
                gap.channel,
                platform
            );
        }
        assert!(
            !gap.gap.as_str().is_empty(),
            "{} has no typed reason",
            gap.channel
        );
    }
    let mut capability_gaps: Vec<&str> = all_rows()
        .iter()
        .filter(|gap| gap.gap == GapKind::NoPluginSource)
        .map(|gap| gap.channel)
        .collect();
    capability_gaps.sort();
    assert_eq!(
        capability_gaps,
        vec![
            "plugins.flutter.io/connectivity",
            "plugins.flutter.io/device_info",
            "plugins.flutter.io/permission_handler",
            "plugins.flutter.io/share",
        ]
    );
}

#[test]
fn row_lookup_is_exact_and_deterministic() {
    assert!(row("flutter/menu").is_some());
    assert!(row("plugins.flutter.io/share").is_some());
    assert!(
        row("flutter/platform").is_none(),
        "a bridged channel is no GAP"
    );
    assert!(row("flutter/unknown").is_none());
    assert_eq!(
        row("flutter/menu").map(|gap| gap.channel),
        Some("flutter/menu")
    );
}
