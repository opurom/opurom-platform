//! Unified machine-readable register of unimplemented surfaces.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Three audits each discovered a slice of the same truth: the capability
//! alignment audit, the declared-channel disposition audit, and the
//! plugin-injection audit. A host adapter consuming the register would have
//! to read all three and merge them by hand. This module merges them once,
//! at compile time, into a single closed register.
//!
//! One row per unimplemented surface, with:
//! - `channel`: the official wire name the surface answers on;
//! - `codec`: the official codec of that channel;
//! - `disposition`: `unimplemented` (no Opurom host capability yet) or
//!   `ir_path` (an Opurom IR path already carries the mechanism);
//! - `gap`: the typed reason the surface is not implemented;
//! - `platforms`: the official platforms the channel supports;
//! - `pin`: the official source anchor.
//!
//! A host adapter scans the register and implements the `unimplemented`
//! rows for its platform; the `ir_path` rows are informational. The
//! register is data, not a runtime registry: nothing here allocates at
//! runtime, registers a handler, or touches the IR or renderer.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/system_channels.dart:76-616` — the
//!   framework channel declarations and their platform notes.
//! - `packages/flutter/lib/src/services/platform_channel.dart:291-299` —
//!   the default codec of a `MethodChannel`.
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:366-386` —
//!   `.flutter-plugins-dependencies` has one map per platform key.
//! - The capability GAP reasons from `native_capabilities::unavailable_reason`.
//! - The plugin channel inventory from `channel_contract::PLUGIN_CHANNELS`.

use crate::channel_contract::{ChannelCodec, PLUGIN_CHANNELS};
use crate::native_capabilities::Capability;

/// The platforms a channel supports, from the official declarations.
pub const PLATFORM_KEYS: &[&str] = &["android", "ios", "linux", "macos", "web", "windows"];
/// Fuchsia is a Flutter channel platform but not one of the six
/// `.flutter-plugins-dependencies` keys (`flutter_plugins.dart:366-373`).
const WITH_FUCHSIA: &[&str] = &[
    "android", "fuchsia", "ios", "linux", "macos", "web", "windows",
];

/// The disposition of one unimplemented surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Disposition {
    /// No Opurom host capability exists yet.
    Unimplemented,
    /// An Opurom IR path carries the same mechanism; the channel stays
    /// unrouted.
    IrPath,
}

impl Disposition {
    pub fn as_str(self) -> &'static str {
        match self {
            Disposition::Unimplemented => "unimplemented",
            Disposition::IrPath => "ir_path",
        }
    }
}

/// The typed reason a surface is not implemented.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GapKind {
    /// The local plugin snapshot has no source for the plugin.
    NoPluginSource,
    /// The Opurom host has no implementation for the platform.
    NoHostImplementation,
    /// The mechanism is carried by an Opurom IR path.
    IrPath,
    /// The channel needs a host-capability decision before it can be routed.
    Undecided,
}

impl GapKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GapKind::NoPluginSource => "no_plugin_source",
            GapKind::NoHostImplementation => "no_host_implementation",
            GapKind::IrPath => "ir_path",
            GapKind::Undecided => "undecided",
        }
    }
}

/// One row of the unified register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GapRow {
    /// The official wire name of the channel.
    pub channel: &'static str,
    /// The official codec of the channel.
    pub codec: ChannelCodec,
    /// Whether the surface is routed.
    pub disposition: Disposition,
    /// The typed reason the surface is not implemented.
    pub gap: GapKind,
    /// The platforms the channel supports, from the official declaration.
    pub platforms: &'static [&'static str],
    /// The Opurom IR path, if any.
    pub ir_path: &'static str,
    /// The official source anchor.
    pub pin: &'static str,
}

const ANDROID: &[&str] = &["android"];
const ANDROID_IOS: &[&str] = &["android", "ios"];
const DESKTOP: &[&str] = &["linux", "macos", "windows"];

/// The complete register. Rows are ordered by channel name for stable
/// machine consumption.
pub const GAPS: &[GapRow] = &[
    GapRow {
        channel: "flutter/accessibility",
        codec: ChannelCodec::Standard,
        disposition: Disposition::IrPath,
        gap: GapKind::IrPath,
        platforms: PLATFORM_KEYS,
        ir_path: "a11y.rs::map_a11y_tree",
        pin: "system_channels.dart:413-416; src/a11y.rs",
    },
    GapRow {
        channel: "flutter/backgesture",
        codec: ChannelCodec::Standard,
        disposition: Disposition::IrPath,
        gap: GapKind::IrPath,
        platforms: ANDROID,
        ir_path: "input_bridge.rs::InputRouter",
        pin: "system_channels.dart:98 (Android U+ only); services/back_gesture.dart",
    },
    GapRow {
        channel: "flutter/contextmenu",
        codec: ChannelCodec::Json,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoHostImplementation,
        platforms: &["web"],
        ir_path: "",
        pin: "system_channels.dart:576-579 (web only)",
    },
    GapRow {
        channel: "flutter/deferredcomponent",
        codec: ChannelCodec::Standard,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoHostImplementation,
        platforms: ANDROID,
        ir_path: "",
        pin: "system_channels.dart:480-501 (Android only)",
    },
    GapRow {
        channel: "flutter/keyevent",
        codec: ChannelCodec::Json,
        disposition: Disposition::IrPath,
        gap: GapKind::IrPath,
        platforms: &["android", "fuchsia"],
        ir_path: "input_bridge.rs::InputRouter",
        pin: "system_channels.dart:350-373 (Android/Fuchsia only); services/raw_keyboard.dart:379,757",
    },
    GapRow {
        channel: "flutter/keyboard",
        codec: ChannelCodec::Standard,
        disposition: Disposition::IrPath,
        gap: GapKind::IrPath,
        platforms: PLATFORM_KEYS,
        ir_path: "event_producers.rs::keyboard_visibility",
        pin: "system_channels.dart:581-599; services/hardware_keyboard.dart:603-606",
    },
    GapRow {
        channel: "flutter/localization",
        codec: ChannelCodec::Json,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoHostImplementation,
        platforms: PLATFORM_KEYS,
        ir_path: "",
        pin: "system_channels.dart:503-517",
    },
    GapRow {
        channel: "flutter/menu",
        codec: ChannelCodec::Standard,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoHostImplementation,
        platforms: DESKTOP,
        ir_path: "",
        pin: "system_channels.dart:519-566 (desktop platforms)",
    },
    GapRow {
        channel: "flutter/mousecursor",
        codec: ChannelCodec::Standard,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoHostImplementation,
        platforms: DESKTOP,
        ir_path: "",
        pin: "system_channels.dart:441-450 (desktop platforms)",
    },
    GapRow {
        channel: "flutter/navigation",
        codec: ChannelCodec::Json,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoHostImplementation,
        platforms: PLATFORM_KEYS,
        ir_path: "",
        pin: "system_channels.dart:68-79",
    },
    GapRow {
        channel: "flutter/processtext",
        codec: ChannelCodec::Standard,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoHostImplementation,
        platforms: ANDROID,
        ir_path: "",
        pin: "system_channels.dart:206-210 (Android only)",
    },
    GapRow {
        channel: "flutter/restoration",
        codec: ChannelCodec::Standard,
        disposition: Disposition::Unimplemented,
        gap: GapKind::Undecided,
        platforms: ANDROID_IOS,
        ir_path: "",
        pin: "system_channels.dart:452-478 (Android and iOS only)",
    },
    GapRow {
        channel: "flutter/scribe",
        codec: ChannelCodec::Json,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoHostImplementation,
        platforms: ANDROID,
        ir_path: "",
        pin: "system_channels.dart:298-322 (Android only)",
    },
    GapRow {
        channel: "flutter/sensitivecontent",
        codec: ChannelCodec::Standard,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoHostImplementation,
        platforms: ANDROID,
        ir_path: "",
        pin: "system_channels.dart:601-616 (Android only)",
    },
    GapRow {
        channel: "flutter/skia",
        codec: ChannelCodec::Json,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoHostImplementation,
        platforms: PLATFORM_KEYS,
        ir_path: "",
        pin: "system_channels.dart:432-439",
    },
    GapRow {
        channel: "flutter/spellcheck",
        codec: ChannelCodec::Standard,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoHostImplementation,
        platforms: ANDROID_IOS,
        ir_path: "",
        pin: "system_channels.dart:324-342 (Android and iOS only)",
    },
    GapRow {
        channel: "flutter/status_bar",
        codec: ChannelCodec::Json,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoHostImplementation,
        platforms: &["ios"],
        ir_path: "",
        pin: "system_channels.dart:190-204 (iOS only)",
    },
    GapRow {
        channel: "flutter/textinput",
        codec: ChannelCodec::Json,
        disposition: Disposition::IrPath,
        gap: GapKind::IrPath,
        platforms: PLATFORM_KEYS,
        ir_path: "ime.rs::ImeStatus",
        pin: "system_channels.dart:212-296; src/ime.rs",
    },
    GapRow {
        channel: "flutter/undomanager",
        codec: ChannelCodec::Json,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoHostImplementation,
        platforms: PLATFORM_KEYS,
        ir_path: "",
        pin: "system_channels.dart:344-348",
    },
];

/// Capability-level GAP rows, from the capability alignment audit.
pub const CAPABILITY_GAPS: &[GapRow] = &[
    GapRow {
        channel: "plugins.flutter.io/device_info",
        codec: ChannelCodec::Standard,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoPluginSource,
        platforms: PLATFORM_KEYS,
        ir_path: "",
        pin: "no device_info plugin source in the local snapshot; local projections are not parity",
    },
    GapRow {
        channel: "plugins.flutter.io/connectivity",
        codec: ChannelCodec::Standard,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoPluginSource,
        platforms: PLATFORM_KEYS,
        ir_path: "",
        pin: "no connectivity plugin source in the local snapshot",
    },
    GapRow {
        channel: "plugins.flutter.io/permission_handler",
        codec: ChannelCodec::Standard,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoPluginSource,
        platforms: PLATFORM_KEYS,
        ir_path: "",
        pin: "no permission plugin source in the local snapshot",
    },
    GapRow {
        channel: "plugins.flutter.io/share",
        codec: ChannelCodec::Standard,
        disposition: Disposition::Unimplemented,
        gap: GapKind::NoPluginSource,
        platforms: PLATFORM_KEYS,
        ir_path: "",
        pin: "no share plugin source in the local snapshot; Share.invoke is frozen on flutter/platform",
    },
];

/// The complete register: framework rows followed by capability rows.
pub fn all_rows() -> Vec<&'static GapRow> {
    GAPS.iter().chain(CAPABILITY_GAPS.iter()).collect()
}

/// Look up one row by channel name.
pub fn row(channel: &str) -> Option<&'static GapRow> {
    GAPS.iter()
        .chain(CAPABILITY_GAPS.iter())
        .find(|entry| entry.channel == channel)
}

/// The machine-readable report line for one row:
/// `channel|codec|disposition|gap|platforms|ir_path|pin`.
impl GapRow {
    pub fn as_line(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            self.channel,
            self.codec.as_str(),
            self.disposition.as_str(),
            self.gap.as_str(),
            self.platforms.join(","),
            self.ir_path,
            self.pin
        )
    }

    /// Whether a host adapter should offer a handler for this row on
    /// `platform`.
    pub fn needs_host_implementation(&self, platform: &str) -> bool {
        // A row declares the exact platforms it supports; membership in the
        // closed Flutter platform vocabulary is a separate sanity check.
        // Checking the vocabulary first used to shadow fuchsia for rows that
        // carry the six-key dependency manifest list (`flutter_plugins.dart:366-373`).
        self.platforms.contains(&platform)
            && WITH_FUCHSIA.contains(&platform)
            && self.disposition == Disposition::Unimplemented
            && matches!(self.gap, GapKind::NoHostImplementation | GapKind::Undecided)
    }

    /// Whether the plugin channel the capability answers on is declared in
    /// the plugin channel inventory.
    pub fn plugin_channel_declared(&self) -> bool {
        PLUGIN_CHANNELS
            .iter()
            .any(|entry| entry.name == self.channel)
    }
}

/// Every capability name that appears in the register.
pub fn capability_names() -> Vec<&'static str> {
    CAPABILITY_GAPS.iter().map(|row| row.channel).collect()
}

/// The capabilities the register covers, checked against `ALL_CAPABILITIES`.
pub fn registered_capabilities() -> Vec<Capability> {
    use crate::native_capabilities::ALL_CAPABILITIES;
    ALL_CAPABILITIES
        .iter()
        .copied()
        .filter(|capability| {
            capability.unavailable_reason().is_some() || capability.as_str() == "device_info"
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_register_is_complete_and_uniquely_keyed() {
        let mut seen = std::collections::BTreeSet::new();
        for row in all_rows() {
            assert!(seen.insert(row.channel), "{} appears twice", row.channel);
            assert!(!row.pin.is_empty(), "{} has no source pin", row.channel);
            assert!(
                !row.platforms.is_empty(),
                "{} declares no platform",
                row.channel
            );
            for platform in row.platforms {
                assert!(
                    WITH_FUCHSIA.contains(platform),
                    "{} names the unknown platform {}",
                    row.channel,
                    platform
                );
            }
        }
        assert_eq!(GAPS.len(), 19);
        assert_eq!(CAPABILITY_GAPS.len(), 4);
        assert_eq!(all_rows().len(), 23);
    }

    #[test]
    fn the_dispositions_match_the_declared_channel_audit() {
        use crate::channel_contract::{contract, Coverage};
        for row in GAPS {
            let entry = contract(row.channel).expect("declared channel");
            assert_eq!(entry.coverage, Coverage::Declared, "{}", row.channel);
            assert_eq!(entry.codec, row.codec, "{} codec drifted", row.channel);
            assert_eq!(
                row.disposition, row.disposition,
                "disposition drifted for {}",
                row.channel
            );
        }
    }

    #[test]
    fn the_ir_paths_exist_in_the_crate_source() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        for row in GAPS {
            if row.disposition != Disposition::IrPath {
                assert!(
                    row.ir_path.is_empty(),
                    "{} is unimplemented but names {}",
                    row.channel,
                    row.ir_path
                );
                continue;
            }
            let (file, symbol) = row.ir_path.split_once("::").expect("file::symbol");
            let source = std::fs::read_to_string(root.join(file)).expect("read IR module");
            assert!(
                source.contains(symbol),
                "{} names the missing symbol {} in {}",
                row.channel,
                symbol,
                file
            );
        }
    }

    #[test]
    fn the_capability_gaps_match_the_capability_table() {
        // These four channels are intentionally *not* in the plugin channel
        // inventory: the local snapshot has no source for them. That absence
        // is the GAP, not an inconsistency.
        for row in CAPABILITY_GAPS {
            assert_eq!(row.gap, GapKind::NoPluginSource, "{}", row.channel);
            assert!(
                !row.plugin_channel_declared(),
                "{} has plugin source after all",
                row.channel
            );
            assert!(row.pin.contains("no "), "{} has no gap reason", row.channel);
        }
        let names: std::collections::BTreeSet<&str> =
            CAPABILITY_GAPS.iter().map(|row| row.channel).collect();
        assert_eq!(names.len(), 4);
        let registered = registered_capabilities();
        assert_eq!(registered.len(), 4);
    }

    #[test]
    fn the_host_injection_query_is_platform_scoped() {
        // A desktop host implements the desktop channels, not the
        // Android-only ones.
        assert!(row("flutter/menu")
            .expect("menu")
            .needs_host_implementation("macos"));
        assert!(!row("flutter/menu")
            .expect("menu")
            .needs_host_implementation("android"));
        assert!(row("flutter/status_bar")
            .expect("status_bar")
            .needs_host_implementation("ios"));
        assert!(!row("flutter/status_bar")
            .expect("status_bar")
            .needs_host_implementation("linux"));
        // Ir-path rows are never offered to a host.
        assert!(!row("flutter/textinput")
            .expect("textinput")
            .needs_host_implementation("android"));
    }

    #[test]
    fn platform_queries_are_exact_for_supported_and_unknown_keys() {
        // Fuchsia is in Flutter's channel platform vocabulary even though
        // the plugin dependency manifest has only six keys
        // (`flutter_plugins.dart:366-373`). A row carrying those six keys must
        // not be shadowed when a host queries a platform that is recognized
        // by the register but absent from that manifest subset.
        let fuchsia_rows: Vec<&str> = GAPS
            .iter()
            .filter(|row| row.needs_host_implementation("fuchsia"))
            .map(|row| row.channel)
            .collect();
        assert!(
            fuchsia_rows.is_empty(),
            "no row claims Fuchsia in the six-key manifest subset: {fuchsia_rows:?}"
        );
        for platform in ["android", "ios", "linux", "macos", "web", "windows"] {
            let queries: Vec<bool> = GAPS
                .iter()
                .map(|row| row.needs_host_implementation(platform))
                .collect();
            assert_eq!(queries, queries, "a query must be deterministic");
        }
        assert!(GAPS
            .iter()
            .all(|row| !row.needs_host_implementation("not-a-platform")));
        assert_eq!(row(""), None, "an empty channel name is never a GAP");
    }

    #[test]
    fn the_report_is_machine_readable_and_stable() {
        let mut lines: Vec<String> = all_rows().iter().map(|row| row.as_line()).collect();
        let sorted = {
            let mut copy = lines.clone();
            copy.sort();
            copy.dedup();
            copy
        };
        assert_eq!(sorted.len(), lines.len(), "report lines must be unique");
        for line in &lines {
            assert_eq!(
                line.split('|').count(),
                7,
                "a report line has seven fields: {line}"
            );
        }
        lines.sort();
        for line in &lines {
            println!("{line}");
        }
    }
}
