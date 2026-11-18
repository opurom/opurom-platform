//! Alignment between the host-injection dry run and `RegistrationPlan`.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! The dry-run injection list and the registration plan are two views of
//! one truth: the PSV contract report. This test builds per-platform
//! `PluginManifest`s from the report, resolves a frozen `RegistrationPlan`,
//! and verifies that:
//! - every plan step comes from the report;
//! - dependency order is stable and deterministic on all three platforms;
//! - an injected failure is isolated without mutating the plan;
//! - a GAP capability never becomes a registration capability.
//!
//! Source pins:
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:243-245` —
//!   `sortByDependencies` places dependencies before dependents with
//!   deterministic tie-breaks.
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:366-386` — the
//!   generated manifest has one map per platform key.
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:478-503` — each
//!   platform entry records `native_build`.
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:528-551` —
//!   registrant generation is per-plugin and catches failures individually.
//! - `packages/flutter/lib/src/services/message_codecs.dart:467,600-617,
//!   628-663` — the frozen shape/error vocabulary of the PSV report.
//!
//! No ABI/IR change: the test only reads the artifact and uses the existing
//! pure `resolve_registration` / `execute_plan` functions.

use std::collections::{BTreeMap, BTreeSet};

use opurom_platform::plugin_executor::{
    execute_plan, RegistrationFn, RegistrationReport, StepOutcome,
};
use opurom_platform::plugin_registry::{resolve_registration, PluginManifest};

const CONTRACT: &str = include_str!("fixtures/capability-contract.psv");

/// Plugin package names for the capabilities that have a local plugin
/// source in the pinned snapshot. The names are the app-facing `pubspec`
/// names. GAP capabilities are deliberately absent.
const PLUGINS: &[(&str, &str, &[&str])] = &[
    ("camera", "camera", &["android", "ios"]),
    (
        "path_provider",
        "path_provider",
        &["android", "ios", "linux", "macos", "windows"],
    ),
    (
        "url_launcher",
        "url_launcher",
        &["android", "ios", "linux", "macos", "windows"],
    ),
    ("image_picker", "image_picker", &["android", "ios"]),
    (
        "shared_preferences",
        "shared_preferences",
        &["android", "ios", "linux", "macos", "windows"],
    ),
    ("video_player", "video_player", &["android", "ios"]),
];

fn contract_capabilities() -> BTreeSet<&'static str> {
    CONTRACT
        .lines()
        .filter_map(|line| line.split('|').next())
        .collect()
}

fn dry_run_capabilities(platform: &str) -> Vec<&'static str> {
    PLUGINS
        .iter()
        .filter(|(_, _, platforms)| platforms.contains(&platform))
        .map(|(name, _, _)| *name)
        .filter(|name| contract_capabilities().contains(name))
        .collect()
}

fn manifests(platform: &str) -> Vec<PluginManifest> {
    dry_run_capabilities(platform)
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let manifest = PluginManifest::new(name, "1.0.0").on_platform(platform);
            // A stable dependency edge exercises `sortByDependencies`
            // without inventing a plugin relationship: each plugin after the
            // first depends on the previous one in PSV order.
            if index > 0 {
                manifest.depends_on(dry_run_capabilities(platform)[index - 1])
            } else {
                manifest
            }
        })
        .collect()
}

fn ok() -> Result<(), &'static str> {
    Ok(())
}

fn fail() -> Result<(), &'static str> {
    Err("host surface unavailable")
}

fn execute(platform: &str, fail_name: Option<&str>) -> (RegistrationReport, Vec<String>) {
    let manifests = manifests(platform);
    let plan = resolve_registration(&manifests, Some(platform)).expect("frozen plan");
    let names = plan.names().iter().map(|name| (*name).to_owned()).collect();
    let fns: Vec<RegistrationFn> = dry_run_capabilities(platform)
        .into_iter()
        .map(|name| RegistrationFn {
            name,
            function: if Some(name) == fail_name { fail } else { ok },
        })
        .collect();
    let permissions = BTreeMap::new();
    (
        execute_plan(&plan, &fns, &manifests, &permissions, platform),
        names,
    )
}

#[test]
fn every_plan_step_comes_from_the_contract_report() {
    let allowed = contract_capabilities();
    for platform in ["android", "macos", "windows"] {
        let manifests = manifests(platform);
        let plan = resolve_registration(&manifests, Some(platform)).expect("frozen plan");
        assert_eq!(plan.names(), dry_run_capabilities(platform));
        for step in plan.steps() {
            assert!(
                allowed.contains(step.name.as_str()),
                "{platform} planned an unlisted plugin {}",
                step.name
            );
            assert_eq!(step.version, "1.0.0");
            assert!(step.capabilities.is_empty());
        }
    }
}

#[test]
fn dependency_order_is_stable_and_replayable() {
    for platform in ["android", "macos", "windows"] {
        let first = manifests(platform);
        let plan = resolve_registration(&first, Some(platform)).expect("frozen plan");
        for _ in 0..10 {
            let replay = resolve_registration(&first, Some(platform)).expect("frozen plan");
            assert_eq!(replay, plan, "{platform} plan drifted");
        }
        // A linear dependency chain has no cycles.
        assert!(plan.cycle_breaks().is_empty());
        let names = plan.names();
        let expected = dry_run_capabilities(platform);
        // A linear dependency chain means the plan order is exactly the
        // dry-run order, and that order is stable across replays.
        assert_eq!(names, expected, "{platform} dependency order drifted");
    }
}

#[test]
fn platform_filtering_matches_the_dry_run_inventory() {
    let android: BTreeSet<&str> = dry_run_capabilities("android").into_iter().collect();
    let macos: BTreeSet<&str> = dry_run_capabilities("macos").into_iter().collect();
    let windows: BTreeSet<&str> = dry_run_capabilities("windows").into_iter().collect();
    assert_eq!(android.len(), 5);
    assert_eq!(macos.len(), 2);
    assert_eq!(windows.len(), 2);
    assert!(android.contains("camera"));
    assert!(
        !macos.contains("camera"),
        "camera has no macOS implementation"
    );
    assert!(!windows.contains("video_player"));
    assert!(macos.contains("path_provider"));
    assert!(windows.contains("url_launcher"));
}

#[test]
fn a_failed_registration_is_isolated_without_mutating_the_plan() {
    for platform in ["android", "macos", "windows"] {
        let (report, names) = execute(platform, None);
        assert!(report.all_registered(), "{platform} baseline must be green");
        assert_eq!(report.registered(), names);

        let victim = names[1].as_str();
        let (failed, replay_names) = execute(platform, Some(victim));
        assert_eq!(replay_names, names, "the frozen plan must not change");
        assert_eq!(failed.failed(), vec![victim]);
        assert_eq!(failed.registered().len(), names.len() - 1);
        // The failing step is recorded with its typed reason.
        assert!(failed.outcomes().iter().any(|(name, outcome)| {
            name == victim
                && matches!(outcome, StepOutcome::Failed { reason } if reason == "host surface unavailable")
        }));
    }
}

#[test]
fn gap_capabilities_are_never_registration_capabilities() {
    let gaps: BTreeSet<&str> = CONTRACT
        .lines()
        .filter(|line| {
            line.split('|')
                .nth(7)
                .map(|gap| gap.starts_with("no "))
                .unwrap_or(false)
        })
        .filter_map(|line| line.split('|').next())
        .collect();
    assert_eq!(
        gaps,
        BTreeSet::from(["device_info", "network", "permissions", "share"])
    );
    for platform in ["android", "macos", "windows"] {
        let manifests = manifests(platform);
        let plan = resolve_registration(&manifests, Some(platform)).expect("frozen plan");
        for step in plan.steps() {
            assert!(
                !gaps.contains(step.name.as_str()),
                "{platform} registered {}",
                step.name
            );
        }
        let (report, _) = execute(platform, None);
        assert!(
            report.permissions().is_empty(),
            "no permissions were declared"
        );
    }
}
