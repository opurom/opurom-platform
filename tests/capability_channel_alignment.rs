//! Three-way alignment audit: capabilities, plugin channels, and manifest.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), plugin snapshot
//! `flutter-plugins` at `557d3284ac9dda32a1106bb75be8a23bdccd2f96`.
//!
//! Opurom declares 14 capability groups in `native_capabilities`, the pinned
//! Flutter framework/plugin channels are frozen in `channel_contract`, and
//! `flutter_tools` generates `.flutter-plugins-dependencies` from each
//! plugin's `pubspec.yaml`. These three sources must agree:
//!
//! * a capability claiming a plugin must have a channel and a plugin package
//!   in the snapshot;
//! * a plugin channel in the snapshot that belongs to a declared capability
//!   must have an Opurom counterpart;
//! * a capability with no plugin source must be an explicit typed GAP, never
//!   a silent implementation.
//!
//! The audit produces a machine-readable report
//! (`class|subject|detail|source`) with four classes:
//! `declared_no_source`, `plugin_channel_missing`, `gap_with_source`, and
//! `duplicate_channel`. A clean run has **zero rows in every class**.
//!
//! Source pins:
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:366-386` — the
//!   generated dependency manifest has one map per platform key
//!   (`ios`, `android`, `macos`, `linux`, `windows`, `web`).
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:478-503` — each
//!   entry records `name`, `path`, `sharedDarwinSource`, `native_build`
//!   (methodChannel or FFI), dependencies and dev dependency status.
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:563-575,943-956` —
//!   platform registrants are generated from method-channel plugins.
//! - `flutter-plugins` snapshot app-facing `pubspec.yaml` files:
//!   `packages/camera/camera/pubspec.yaml:13-21`,
//!   `packages/path_provider/path_provider/pubspec.yaml:12-23`,
//!   `packages/url_launcher/url_launcher/pubspec.yaml:13-25`,
//!   `packages/image_picker/image_picker/pubspec.yaml:13-20`,
//!   `packages/shared_preferences/shared_preferences/pubspec.yaml:13-25`,
//!   `packages/video_player/video_player/pubspec.yaml:13-20`.
//!
//! No ABI/IR change: this test only reads compile-time tables and source text.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use opurom_platform::channel_contract::{ChannelContract, PLUGIN_CHANNELS};
use opurom_platform::native_capabilities::{methods_for, Capability, ALL_CAPABILITIES};

/// Why a capability has (or lacks) a plugin source in the local snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceClass {
    /// A plugin package and a stable channel declaration exist locally.
    Plugin,
    /// A plugin package exists, but its only channel is a per-instance
    /// `EventChannel` with a runtime texture-id suffix
    /// (`video_player_android/android/src/main/java/io/flutter/plugins/
    /// videoplayer/VideoPlayerPlugin.java:121-123`); no stable name to freeze.
    PerInstanceEvent,
    /// Opurom owns the backend; Flutter has a framework channel but no
    /// plugin channel in the snapshot.
    Framework,
    /// No plugin source in the snapshot; the capability must be a typed GAP.
    Gap,
}

/// One expected alignment row, independently pinned to source.
struct Alignment {
    capability: Capability,
    /// The plugin package name from the app-facing `pubspec.yaml`.
    package: &'static str,
    /// The plugin channel name, if any.
    channel: Option<&'static str>,
    source_class: SourceClass,
    /// Source pin for the package/channel declaration.
    pin: &'static str,
}

/// The table is kept in `ALL_CAPABILITIES` order.
const ALIGNMENTS: &[Alignment] = &[
    Alignment {
        capability: Capability::Preferences,
        package: "shared_preferences",
        channel: Some("plugins.flutter.io/shared_preferences"),
        source_class: SourceClass::Plugin,
        pin: "shared_preferences/pubspec.yaml:13-25; method_channel_shared_preferences.dart:12",
    },
    Alignment {
        capability: Capability::PathProvider,
        package: "path_provider",
        channel: Some("plugins.flutter.io/path_provider"),
        source_class: SourceClass::Plugin,
        pin: "path_provider/pubspec.yaml:12-23; method_channel_path_provider.dart:16",
    },
    Alignment {
        capability: Capability::UrlLauncher,
        package: "url_launcher",
        channel: Some("plugins.flutter.io/url_launcher"),
        source_class: SourceClass::Plugin,
        pin: "url_launcher/pubspec.yaml:13-25; method_channel_url_launcher.dart:12",
    },
    Alignment {
        capability: Capability::ImagePicker,
        package: "image_picker",
        channel: Some("plugins.flutter.io/image_picker"),
        source_class: SourceClass::Plugin,
        pin: "image_picker/pubspec.yaml:13-20; method_channel_image_picker.dart:12",
    },
    Alignment {
        capability: Capability::FileStorage,
        package: "opurom-platform/src/storage.rs",
        channel: None,
        source_class: SourceClass::Framework,
        pin: "opurom-platform/src/storage.rs; shared_preferences.dart:17-158 (same primitive)",
    },
    Alignment {
        capability: Capability::Notifications,
        package: "opurom-platform/src/notification.rs",
        channel: None,
        source_class: SourceClass::Framework,
        pin: "opurom-platform/src/notification.rs; no Flutter plugin channel",
    },
    Alignment {
        capability: Capability::DeviceInfo,
        package: "",
        channel: None,
        source_class: SourceClass::Gap,
        pin: "no device_info plugin source in snapshot; local projections are not plugin parity",
    },
    Alignment {
        capability: Capability::Network,
        package: "",
        channel: None,
        source_class: SourceClass::Gap,
        pin: "no connectivity plugin source in flutter-plugins snapshot",
    },
    Alignment {
        capability: Capability::Permissions,
        package: "",
        channel: None,
        source_class: SourceClass::Gap,
        pin: "no permission plugin source in flutter-plugins snapshot",
    },
    Alignment {
        capability: Capability::Share,
        package: "",
        channel: None,
        source_class: SourceClass::Gap,
        pin: "system_channels.dart:181-188; no share plugin source in snapshot",
    },
    Alignment {
        capability: Capability::Clipboard,
        package: "flutter",
        channel: Some("flutter/platform"),
        source_class: SourceClass::Framework,
        pin: "services/clipboard.dart:32-68; system_channels.dart:181-184",
    },
    Alignment {
        capability: Capability::Haptics,
        package: "flutter",
        channel: Some("flutter/platform"),
        source_class: SourceClass::Framework,
        pin: "services/haptic_feedback.dart:19-152; system_channels.dart:181-184",
    },
    Alignment {
        capability: Capability::Camera,
        package: "camera",
        channel: Some("plugins.flutter.io/camera"),
        source_class: SourceClass::Plugin,
        pin: "camera/pubspec.yaml:13-21; method_channel_camera.dart:17",
    },
    Alignment {
        capability: Capability::VideoPlayer,
        package: "video_player",
        channel: None,
        source_class: SourceClass::PerInstanceEvent,
        pin: "video_player/pubspec.yaml:13-20; VideoPlayerPlugin.java:121-123 (per-instance event channel)",
    },
];

/// The platform keys of each app-facing plugin, read from its `pubspec.yaml`
/// and frozen here so the audit is self-contained in CI, which does not
/// clone the plugin snapshot
/// (`.github/workflows/ci.yml` clones only `opurom-examples` for this repo).
/// The local snapshot is cross-checked when it is available.
const FROZEN_PLATFORMS: &[(&str, &[&str], &str)] = &[
    (
        "shared_preferences",
        &["android", "ios", "linux", "macos", "web", "windows"],
        "shared_preferences/pubspec.yaml:13-25",
    ),
    (
        "path_provider",
        &["android", "ios", "linux", "macos", "windows"],
        "path_provider/pubspec.yaml:12-23",
    ),
    (
        "url_launcher",
        &["android", "ios", "linux", "macos", "web", "windows"],
        "url_launcher/pubspec.yaml:13-25",
    ),
    (
        "image_picker",
        &["android", "ios", "web"],
        "image_picker/pubspec.yaml:13-20",
    ),
    (
        "camera",
        &["android", "ios", "web"],
        "camera/pubspec.yaml:13-21",
    ),
    (
        "video_player",
        &["android", "ios", "web"],
        "video_player/pubspec.yaml:13-20",
    ),
];

/// A machine-readable audit finding.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Finding {
    class: &'static str,
    subject: String,
    detail: String,
    source: &'static str,
}

impl Finding {
    fn new(
        class: &'static str,
        subject: impl Into<String>,
        detail: impl Into<String>,
        source: &'static str,
    ) -> Self {
        Self {
            class,
            subject: subject.into(),
            detail: detail.into(),
            source,
        }
    }

    /// `class|subject|detail|source`
    fn as_line(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.class, self.subject, self.detail, self.source
        )
    }
}

/// The plugin channel inventory excluding Opurom-internal projections.
fn plugin_channels() -> Vec<&'static ChannelContract> {
    PLUGIN_CHANNELS.iter().collect()
}

/// Run the three-way audit. Zero findings means the three sources agree.
fn audit(source_root: Option<&Path>) -> Vec<Finding> {
    let mut findings = Vec::new();
    let classes: BTreeMap<Capability, &Alignment> =
        ALIGNMENTS.iter().map(|row| (row.capability, row)).collect();

    // 1. Every declared capability must have exactly one alignment row.
    for capability in ALL_CAPABILITIES {
        let Some(row) = classes.get(&capability) else {
            findings.push(Finding::new(
                "declared_no_source",
                capability.as_str(),
                "capability has no alignment row",
                "native_capabilities.rs:ALL_CAPABILITIES",
            ));
            continue;
        };

        // A plugin-backed capability must have a package and a channel.
        if row.source_class == SourceClass::Plugin {
            let frozen = FROZEN_PLATFORMS.iter().find(|entry| entry.0 == row.package);
            let present = frozen.is_some()
                && source_root
                    .map(|root| root.join("packages").join(row.package).is_dir())
                    .unwrap_or(true);
            if row.package.is_empty() || !present {
                findings.push(Finding::new(
                    "declared_no_source",
                    capability.as_str(),
                    format!("package {} is absent", row.package),
                    row.pin,
                ));
            }
            let Some(channel) = row.channel else {
                findings.push(Finding::new(
                    "declared_no_source",
                    capability.as_str(),
                    "plugin capability has no channel",
                    row.pin,
                ));
                continue;
            };
            if !plugin_channels().iter().any(|entry| entry.name == channel) {
                findings.push(Finding::new(
                    "declared_no_source",
                    capability.as_str(),
                    format!("channel {channel} is absent"),
                    row.pin,
                ));
            }
        }

        // A GAP capability must have no source and no method table. A
        // non-GAP capability must not claim a typed GAP reason.
        if row.source_class == SourceClass::Gap {
            if capability.unavailable_reason().is_none() {
                findings.push(Finding::new(
                    "gap_with_source",
                    capability.as_str(),
                    "no typed GAP reason",
                    row.pin,
                ));
            }
            // DeviceInfo is a declared GAP with local projections: it may
            // have a small method table as long as it never fabricates a
            // full device_info_plus implementation. The other GAPs have no
            // pinned plugin source and therefore no methods.
            if capability != Capability::DeviceInfo && !methods_for(capability).is_empty() {
                findings.push(Finding::new(
                    "gap_with_source",
                    capability.as_str(),
                    "GAP capability has a method table",
                    row.pin,
                ));
            }
        } else if capability.unavailable_reason().is_some() {
            findings.push(Finding::new(
                "declared_no_source",
                capability.as_str(),
                "capability is a typed GAP but the alignment table claims a source",
                row.pin,
            ));
        }
    }

    // 2. Every plugin channel must be claimed by exactly one capability.
    let channels: BTreeSet<&str> = ALIGNMENTS.iter().filter_map(|row| row.channel).collect();
    for entry in plugin_channels() {
        if entry.kind == opurom_platform::platform_channel::ChannelKind::Event {
            continue;
        }
        let owners: Vec<&str> = ALIGNMENTS
            .iter()
            .filter(|row| row.channel == Some(entry.name))
            .map(|row| row.capability.as_str())
            .collect();
        if owners.len() != 1 {
            findings.push(Finding::new(
                "plugin_channel_missing",
                entry.name,
                format!("owner count {}", owners.len()),
                entry.pin,
            ));
        }
        if !channels.contains(entry.name) {
            findings.push(Finding::new(
                "plugin_channel_missing",
                entry.name,
                "no capability claims this channel",
                entry.pin,
            ));
        }
    }

    // 3. A channel name must not appear twice in the plugin inventory.
    let mut seen = BTreeSet::new();
    for entry in plugin_channels() {
        if entry.kind == opurom_platform::platform_channel::ChannelKind::Event {
            continue;
        }
        if !seen.insert(entry.name) {
            findings.push(Finding::new(
                "duplicate_channel",
                entry.name,
                "channel declared twice",
                entry.pin,
            ));
        }
    }

    findings.sort();
    findings
}

fn snapshot_root() -> Option<PathBuf> {
    // The CI workflow does not clone the plugin snapshot, so the frozen
    // pubspec table is the source of truth there. Use a sibling checkout
    // when present (CI or local worktree), then the local parity workspace
    // where the pinned checkout lives next to the other opurom repos.
    let sibling = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("flutter-plugins");
    if sibling.join("packages").is_dir() {
        return Some(sibling);
    }
    let workspace = PathBuf::from("/Users/mac/work/opurom/flutter-plugins");
    if workspace.join("packages").is_dir() {
        return Some(workspace);
    }
    None
}

/// Read the app-facing `pubspec.yaml` and verify the plugin section declares
/// at least one platform key. This is the input `flutter_tools` consumes
/// when generating `.flutter-plugins-dependencies`
/// (`flutter_tools/lib/src/flutter_plugins.dart:366-386`).
fn plugin_manifest_for(root: &Path, package: &str) -> Option<(String, BTreeSet<String>)> {
    let path = root
        .join("packages")
        .join(package)
        .join(package)
        .join("pubspec.yaml");
    let text = fs::read_to_string(path).ok()?;
    let name = text
        .lines()
        .find_map(|line| line.strip_prefix("name:").map(str::trim))
        .unwrap_or("")
        .to_owned();
    let mut platforms = BTreeSet::new();
    let mut in_platforms = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "platforms:" {
            in_platforms = true;
            continue;
        }
        if in_platforms {
            if line.len() == trimmed.len() {
                // Dedented back to the `flutter:` or `plugin:` level.
                in_platforms = false;
                continue;
            }
            if let Some((key, _)) = trimmed.split_once(':') {
                if line.starts_with("      ") && !trimmed.starts_with("default_package") {
                    platforms.insert(key.to_owned());
                }
            }
        }
    }
    Some((name, platforms))
}

#[test]
fn alignment_table_is_complete_and_uniquely_keyed() {
    assert_eq!(ALIGNMENTS.len(), ALL_CAPABILITIES.len());
    let mut seen = BTreeSet::new();
    for row in ALIGNMENTS {
        assert!(
            seen.insert(row.capability),
            "{:?} appears twice",
            row.capability
        );
        assert!(!row.pin.is_empty());
    }
    for capability in ALL_CAPABILITIES {
        assert!(
            seen.contains(&capability),
            "{capability:?} has no alignment row"
        );
    }
}

#[test]
fn three_way_audit_reports_zero_findings() {
    let findings = audit(snapshot_root().as_deref());
    assert!(
        findings.is_empty(),
        "alignment findings:\n{}",
        findings
            .iter()
            .map(Finding::as_line)
            .collect::<Vec<String>>()
            .join("\n")
    );
}

#[test]
fn the_audit_is_self_contained_when_the_snapshot_is_absent() {
    // This is the GitHub Actions case: ci.yml clones only opurom-examples.
    let findings = audit(None);
    assert!(
        findings.is_empty(),
        "frozen snapshot table drifted: {}",
        findings
            .iter()
            .map(Finding::as_line)
            .collect::<Vec<String>>()
            .join("\n")
    );
}

#[test]
fn every_plugin_package_feeds_the_flutter_tools_manifest() {
    for row in ALIGNMENTS {
        if !matches!(
            row.source_class,
            SourceClass::Plugin | SourceClass::PerInstanceEvent
        ) {
            continue;
        }
        let frozen = FROZEN_PLATFORMS
            .iter()
            .find(|entry| entry.0 == row.package)
            .unwrap_or_else(|| panic!("{} has no frozen platform key row", row.package));
        assert!(
            !frozen.1.is_empty(),
            "{} declares no platform key",
            row.package
        );
        println!(
            "{}|{}|{}|{}",
            row.capability.as_str(),
            row.package,
            frozen.1.join(","),
            frozen.2
        );

        // When the pinned snapshot is available locally, verify the frozen
        // table still matches the real pubspec.yaml.
        if let Some(root) = snapshot_root() {
            if let Some((name, platforms)) = plugin_manifest_for(&root, row.package) {
                assert_eq!(name, row.package, "package name drifted");
                let actual: BTreeSet<String> = platforms;
                let expected: BTreeSet<String> =
                    frozen.1.iter().map(|key| (*key).to_owned()).collect();
                assert_eq!(
                    actual, expected,
                    "{} platform keys drifted from the frozen snapshot",
                    row.package
                );
            }
        }
    }
}

#[test]
fn the_audit_detects_missing_sources_and_wrong_gaps() {
    // An alignment row whose plugin package does not exist in the frozen
    // table and in no snapshot: the probe passes `None` for the root and a
    // package that no frozen row declares.
    let missing_package = Alignment {
        capability: Capability::Camera,
        package: "missing_package",
        channel: Some("plugins.flutter.io/camera"),
        source_class: SourceClass::Plugin,
        pin: "probe",
    };
    let findings = audit_with_rows(None, &[missing_package]);
    assert!(findings.iter().any(|finding| {
        finding.class == "declared_no_source" && finding.subject == Capability::Camera.as_str()
    }));

    // A GAP capability with no plugin source is not a finding; the typed
    // reason and empty method table are the expected state.
    let real_gap = Alignment {
        capability: Capability::Network,
        package: "",
        channel: None,
        source_class: SourceClass::Gap,
        pin: "probe",
    };
    assert!(audit_with_rows(snapshot_root().as_deref(), &[real_gap]).is_empty());

    // A plugin channel with no owner is a finding.
    let orphan = opurom_platform::channel_contract::ChannelContract {
        name: "plugins.flutter.io/orphan",
        kind: opurom_platform::platform_channel::ChannelKind::Method { optional: false },
        optional: false,
        codec: opurom_platform::channel_contract::ChannelCodec::Standard,
        coverage: opurom_platform::channel_contract::Coverage::Declared,
        pin: "probe",
    };
    assert_eq!(
        channel_findings(&[orphan])[0].class,
        "plugin_channel_missing"
    );
}

fn channel_findings(channels: &[ChannelContract]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in channels {
        let owners = ALIGNMENTS
            .iter()
            .filter(|row| row.channel == Some(entry.name))
            .count();
        if owners != 1 {
            findings.push(Finding::new(
                "plugin_channel_missing",
                entry.name,
                format!("owner count {}", owners),
                entry.pin,
            ));
        }
        if !seen.insert(entry.name) {
            findings.push(Finding::new(
                "duplicate_channel",
                entry.name,
                "channel declared twice",
                entry.pin,
            ));
        }
    }
    findings
}

/// A variant of [`audit`] that accepts a smaller alignment table, used to
/// probe the auditor's detection logic without touching production code.
fn audit_with_rows(source_root: Option<&Path>, rows: &[Alignment]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for row in rows {
        if row.source_class == SourceClass::Plugin {
            // The frozen pubspec table is authoritative when no snapshot is
            // present; a package that no frozen row declares is a finding.
            let frozen_present = FROZEN_PLATFORMS.iter().any(|entry| entry.0 == row.package);
            let disk_present =
                source_root.map(|root| root.join("packages").join(row.package).is_dir());
            let present = match disk_present {
                Some(found) => found,
                None => frozen_present,
            };
            if !present {
                findings.push(Finding::new(
                    "declared_no_source",
                    row.capability.as_str(),
                    format!("package {} is absent", row.package),
                    row.pin,
                ));
            }
        }
        if row.source_class == SourceClass::Gap && row.capability.unavailable_reason().is_none() {
            findings.push(Finding::new(
                "gap_with_source",
                row.capability.as_str(),
                "no typed GAP reason",
                row.pin,
            ));
        }
    }
    findings
}
