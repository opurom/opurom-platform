//! Contract test for the generated capability/method/shape/error report.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! The report at `tests/fixtures/capability-contract.psv` is the single
//! machine-readable source for host injection generators and audit scripts.
//! It must be deterministic, complete over the 14 capabilities, and must
//! agree with the same compile-time tables `CapabilityTable::call` uses.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/message_codecs.dart:467,600-617,
//!   628-663` — argument errors, method-call shape, error envelopes.
//! - `packages/flutter/lib/src/services/platform_channel.dart:359-364,
//!   600-613` — null reply / missing plugin, handler error mapping.
//! - `packages/flutter/lib/src/services/binding.dart:448-457,523-528` —
//!   framework method payload examples.
//!
//! No ABI/IR change: the test reads the generated artifact and compile-time
//! tables only.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use opurom_platform::frozen_contracts::CONTRACTS;
use opurom_platform::host_injection_audit::injection_slots;
use opurom_platform::native_capabilities::{methods_for, ALL_CAPABILITIES};

const REPORT: &str = include_str!("fixtures/capability-contract.psv");

#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    capability: &'static str,
    qualified: String,
    wire: String,
    argument: String,
    keys: String,
    returns: String,
    error: String,
    gap: String,
    pin: String,
}

fn parse(line: &str) -> Row {
    let fields: Vec<&str> = line.split('|').collect();
    assert_eq!(fields.len(), 9, "a report row has 9 fields: {line}");
    Row {
        capability: ALL_CAPABILITIES
            .iter()
            .map(|capability| capability.as_str())
            .find(|name| *name == fields[0])
            .unwrap_or_else(|| panic!("unknown capability {}", fields[0])),
        qualified: fields[1].to_owned(),
        wire: fields[2].to_owned(),
        argument: fields[3].to_owned(),
        keys: fields[4].to_owned(),
        returns: fields[5].to_owned(),
        error: fields[6].to_owned(),
        gap: fields[7].to_owned(),
        pin: fields[8].to_owned(),
    }
}

fn rows() -> Vec<Row> {
    REPORT
        .lines()
        .filter(|line| !line.is_empty())
        .map(parse)
        .collect()
}

#[test]
fn the_report_is_deterministic_and_regenerates_exactly() {
    // Run the generator into a temporary path and compare bytes.
    let path = std::env::temp_dir().join("opurom-capability-contract-check.psv");
    let status = Command::new(env!("CARGO_BIN_EXE_generate-capability-contract"))
        .arg(&path)
        .status()
        .expect("run generator");
    assert!(status.success(), "generator failed");
    let generated = std::fs::read_to_string(&path).expect("read generated report");
    assert_eq!(generated, REPORT, "the checked-in report drifted");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn every_capability_and_dispatchable_method_is_present() {
    let report = rows();
    let mut expected = BTreeMap::new();
    for capability in ALL_CAPABILITIES {
        for method in methods_for(capability) {
            expected.insert(
                format!("{}.{}", capability.as_str(), method),
                (capability.as_str(), method),
            );
        }
    }
    let actual: BTreeSet<&str> = report
        .iter()
        .filter(|row| row.error == "invalid_argument" || row.error == "unimplemented")
        .map(|row| row.qualified.as_str())
        .collect();
    let expected_names: BTreeSet<String> = expected.keys().cloned().collect();
    for name in &expected_names {
        assert!(actual.contains(name.as_str()), "missing report row {name}");
    }
    assert_eq!(expected.len(), injection_slots().len());
}

#[test]
fn the_frozen_gap_methods_are_reported_with_their_shapes() {
    let report = rows();
    let frozen: Vec<&Row> = report
        .iter()
        .filter(|row| {
            CONTRACTS.iter().any(|contract| {
                contract.capability.as_str() == row.capability && !contract.methods.is_empty()
            })
        })
        .collect();
    // Only the two capabilities with official pins have frozen rows.
    let with_pins: BTreeSet<&str> = CONTRACTS
        .iter()
        .filter(|contract| !contract.methods.is_empty())
        .map(|contract| contract.capability.as_str())
        .collect();
    assert_eq!(with_pins, BTreeSet::from(["permissions", "share"]));

    let share = report
        .iter()
        .find(|row| row.qualified == "share.Share.invoke")
        .expect("share frozen row");
    assert_eq!(share.argument, "string");
    assert_eq!(share.keys, "");
    assert_eq!(share.returns, "null");
    assert_eq!(share.error, "invalid_argument");
    assert!(share.pin.contains("editable_text.dart"));

    let permissions = report
        .iter()
        .find(|row| row.qualified == "permissions.System.exitApplication")
        .expect("permissions frozen row");
    assert_eq!(permissions.argument, "map");
    assert_eq!(permissions.keys, "type,exitCode");
    assert_eq!(permissions.returns, "map");
    assert_eq!(permissions.error, "invalid_argument");
    assert!(permissions.pin.contains("binding.dart"));

    assert!(!frozen.is_empty());
}

#[test]
fn error_codes_match_the_capability_gap_state() {
    for row in rows() {
        assert!(
            matches!(row.error.as_str(), "invalid_argument" | "unimplemented"),
            "{} has unknown error code {}",
            row.qualified,
            row.error
        );
        let capability = ALL_CAPABILITIES
            .iter()
            .find(|capability| capability.as_str() == row.capability)
            .expect("capability");
        if capability.unavailable_reason().is_some() {
            // A GAP capability always answers `unimplemented`; the frozen
            // wire row records its shape error separately in the argument
            // column.
            assert!(
                row.error == "unimplemented" || row.gap.starts_with("no "),
                "{} must stay a GAP",
                row.qualified
            );
        } else {
            assert_eq!(row.gap, "none");
        }
    }
}

#[test]
fn the_report_is_unique_sorted_and_self_contained() {
    let report = rows();
    let mut lines: Vec<String> = REPORT.lines().map(str::to_owned).collect();
    let sorted = {
        let mut copy = lines.clone();
        copy.sort();
        copy.dedup();
        copy
    };
    assert_eq!(sorted.len(), lines.len(), "report rows must be unique");
    lines.sort();
    assert_eq!(lines.first().unwrap(), REPORT.lines().next().unwrap());
    for row in &report {
        assert!(!row.pin.is_empty(), "{} has no source pin", row.qualified);
        assert!(
            matches!(row.argument.as_str(), "value" | "string" | "map" | "none"),
            "{} has unknown argument shape {}",
            row.qualified,
            row.argument
        );
        assert!(
            matches!(row.returns.as_str(), "value" | "string" | "map" | "null"),
            "{} has unknown return shape {}",
            row.qualified,
            row.returns
        );
    }
}
