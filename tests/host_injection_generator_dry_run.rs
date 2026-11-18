//! Host injection generator dry-run from the machine-readable contract.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! The PSV contract is the single input a host adapter generator is allowed
//! to read. This test implements a dry-run generator over that input and
//! checks that for Android, macOS and Windows it:
//! - never emits a capability absent from the contract;
//! - never skips a capability that the contract marks implementable;
//! - always marks the four plugin GAP capabilities `unimplemented`;
//! - records the exact source pin for every generated slot.
//!
//! Source pins:
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:366-386` — the
//!   generated dependency manifest has one map per platform key.
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:478-503` — each
//!   platform entry carries name, path, native_build and dependencies.
//! - `packages/flutter/lib/src/services/message_codecs.dart:467,600-617,
//!   628-663` — argument errors, method-call shape and error envelope.
//! - `packages/flutter/lib/src/services/binding.dart:448-457,523-528` —
//!   framework payload examples for the frozen GAP methods.
//!
//! No ABI/IR change: the dry-run produces text rows only.

use std::collections::BTreeSet;

const CONTRACT: &str = include_str!("fixtures/capability-contract.psv");

#[derive(Clone, Debug, PartialEq, Eq)]
struct ContractRow {
    capability: String,
    qualified: String,
    wire: String,
    argument: String,
    keys: Vec<String>,
    returns: String,
    error: String,
    gap: String,
    pin: String,
}

fn parse_contract() -> Vec<ContractRow> {
    CONTRACT
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let f: Vec<&str> = line.split('|').collect();
            assert_eq!(f.len(), 9, "a contract row has 9 fields: {line}");
            ContractRow {
                capability: f[0].to_owned(),
                qualified: f[1].to_owned(),
                wire: f[2].to_owned(),
                argument: f[3].to_owned(),
                keys: if f[4].is_empty() {
                    Vec::new()
                } else {
                    f[4].split(',').map(str::to_owned).collect()
                },
                returns: f[5].to_owned(),
                error: f[6].to_owned(),
                gap: f[7].to_owned(),
                pin: f[8].to_owned(),
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InjectionRow {
    platform: &'static str,
    capability: String,
    method: String,
    generated: bool,
    status: &'static str,
    error_code: String,
    pin: String,
}

impl InjectionRow {
    fn as_line(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            self.platform,
            self.capability,
            self.method,
            self.generated,
            self.status,
            self.error_code,
            self.pin
        )
    }
}

/// A dry-run generator: scans the PSV only. A capability with a local
/// dispatch row is generated; a capability whose contract row carries a
/// `no_*` GAP reason is emitted as a typed-unimplemented placeholder.
fn generate(platform: &'static str) -> Vec<InjectionRow> {
    parse_contract()
        .iter()
        .filter(|row| row.error == "invalid_argument" || row.error == "unimplemented")
        .map(|row| {
            let is_gap = row.gap.starts_with("no ");
            InjectionRow {
                platform,
                capability: row.capability.clone(),
                method: row.qualified.clone(),
                generated: !is_gap,
                status: if is_gap {
                    "unimplemented"
                } else {
                    "implementable"
                },
                error_code: if is_gap {
                    "unimplemented".to_owned()
                } else {
                    row.error.clone()
                },
                pin: row.pin.clone(),
            }
        })
        .collect()
}

#[test]
fn the_dry_run_is_deterministic_for_all_three_platforms() {
    for platform in ["android", "macos", "windows"] {
        let first = generate(platform);
        for _ in 0..3 {
            assert_eq!(generate(platform), first, "{platform} generator drifted");
        }
        let mut names: BTreeSet<&str> = first.iter().map(|row| row.method.as_str()).collect();
        assert_eq!(names.len(), first.len(), "{platform} duplicate row");
        // Every row has seven pipe-separated fields and a source pin.
        for row in &first {
            assert_eq!(row.as_line().split('|').count(), 7);
            assert!(!row.pin.is_empty());
        }
        names.clear();
    }
}

#[test]
fn no_capability_outside_the_contract_is_generated() {
    let contract = parse_contract();
    let allowed: BTreeSet<&str> = contract.iter().map(|row| row.qualified.as_str()).collect();
    let capabilities: BTreeSet<&str> = contract.iter().map(|row| row.capability.as_str()).collect();
    assert_eq!(
        capabilities.len(),
        14,
        "the contract covers 14 capabilities"
    );

    for platform in ["android", "macos", "windows"] {
        for row in generate(platform) {
            assert!(
                allowed.contains(row.method.as_str()),
                "{} generated an unlisted method {}",
                platform,
                row.method
            );
            assert!(
                capabilities.contains(row.capability.as_str()),
                "{} generated an unlisted capability {}",
                platform,
                row.capability
            );
        }
    }
}

#[test]
fn no_implementable_capability_is_skipped() {
    let contract = parse_contract();
    let implementable: BTreeSet<&str> = contract
        .iter()
        .filter(|row| row.error == "invalid_argument" && row.gap == "none")
        .map(|row| row.qualified.as_str())
        .collect();
    assert!(
        implementable.len() > 40,
        "the contract should carry the real dispatchable methods"
    );

    for platform in ["android", "macos", "windows"] {
        let rows = generate(platform);
        let generated: BTreeSet<&str> = rows
            .iter()
            .filter(|row| row.generated)
            .map(|row| row.method.as_str())
            .collect();
        for method in &implementable {
            assert!(
                generated.contains(method),
                "{platform} skipped implementable {method}"
            );
        }
    }
}

#[test]
fn gap_capabilities_stay_typed_unimplemented_on_every_platform() {
    let contract = parse_contract();
    let gaps: BTreeSet<&str> = contract
        .iter()
        .filter(|row| row.gap.starts_with("no "))
        .map(|row| row.capability.as_str())
        .collect();
    assert_eq!(
        gaps,
        BTreeSet::from(["device_info", "network", "permissions", "share"])
    );

    for platform in ["android", "macos", "windows"] {
        for row in generate(platform) {
            if gaps.contains(row.capability.as_str()) {
                assert!(
                    !row.generated,
                    "{} must not implement {}",
                    platform, row.method
                );
                assert_eq!(row.status, "unimplemented");
                assert_eq!(row.error_code, "unimplemented");
            }
        }
    }
}

#[test]
fn the_generator_preserves_frozen_shapes_and_pins() {
    let contract = parse_contract();
    let share = contract
        .iter()
        .find(|row| row.qualified == "share.Share.invoke")
        .expect("share row");
    assert_eq!(share.argument, "string");
    assert!(share.keys.is_empty());
    assert_eq!(share.returns, "null");
    assert!(share.pin.contains("editable_text.dart"));

    let permissions = contract
        .iter()
        .find(|row| row.qualified == "permissions.System.exitApplication")
        .expect("permissions row");
    assert_eq!(permissions.argument, "map");
    assert_eq!(permissions.keys, vec!["type", "exitCode"]);
    assert_eq!(permissions.returns, "map");
    assert!(permissions.pin.contains("binding.dart"));

    // The dry-run output records the same pins for those capabilities.
    for platform in ["android", "macos", "windows"] {
        for row in generate(platform)
            .into_iter()
            .filter(|row| row.capability == "share" || row.capability == "permissions")
        {
            assert!(row.pin.contains("dart"), "{} pin too weak", row.method);
        }
    }
}
