//! Cross-validation between the frozen API contracts and the unified GAP
//! register.
//!
//! Flutter baseline: 3.50.0 (`flutter-golden` HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), plugin snapshot
//! `557d3284ac9dda32a1106bb75be8a23bdccd2f96`.
//!
//! The frozen contracts (this round) and the unified GAP register (last
//! round) describe the same four capabilities from two directions:
//! - the register states that a capability is a GAP and why;
//! - the frozen contract states the wire shape a host must implement and
//!   from which official source the shape was read.
//!
//! Both must agree on the capability set, the GAP reason and the source
//! anchor, or a host adapter would consume contradictory instructions.
//!
//! Source pins:
//! - `packages/flutter/lib/src/widgets/editable_text.dart:3025-3037` and
//!   `packages/flutter/lib/src/widgets/selectable_region.dart:1595-1600` —
//!   `Share.invoke` carries the plain text selection.
//! - `packages/flutter/lib/src/services/binding.dart:523-528` — the
//!   `System.exitApplication` payload and reply.
//! - `engine/src/flutter/lib/ui/platform_dispatcher.dart:2481-2498` — the
//!   `AppExitType` values the payload's `type` key carries.
//! - `packages/flutter/lib/src/services/clipboard.dart:14-22` — the
//!   `ClipboardData` shape that fixes the argument-key style.
//!
//! No ABI/IR change: the cross-validation only reads compile-time tables.

use std::collections::BTreeSet;

use opurom_platform::frozen_contracts::{contract, ArgType, FrozenMethod, ReturnType, CONTRACTS};
use opurom_platform::gap_register::{row, CAPABILITY_GAPS};
use opurom_platform::native_capabilities::{methods_for, Capability, ALL_CAPABILITIES};
use opurom_platform::platform_channel::Value;

#[test]
fn the_frozen_contracts_and_the_register_agree_on_the_gap_set() {
    let frozen: BTreeSet<&str> = CONTRACTS
        .iter()
        .map(|entry| entry.capability.as_str())
        .collect();
    let registered: BTreeSet<&str> = CAPABILITY_GAPS
        .iter()
        .map(|entry| entry.channel.trim_start_matches("plugins.flutter.io/"))
        .collect();
    // The register uses plugin names (`connectivity`, `permission_handler`),
    // the frozen contracts use Opurom capability names. Normalize the two
    // namespaces before comparing them.
    let normalized: BTreeSet<&str> = registered
        .iter()
        .map(|name| match *name {
            "connectivity" => "network",
            "permission_handler" => "permissions",
            other => other,
        })
        .collect();
    assert_eq!(frozen, normalized);
}

#[test]
fn every_frozen_pin_agrees_with_the_register_reason() {
    for entry in CONTRACTS {
        let plugin_channel = match entry.capability {
            Capability::DeviceInfo => "plugins.flutter.io/device_info",
            Capability::Network => "plugins.flutter.io/connectivity",
            Capability::Permissions => "plugins.flutter.io/permission_handler",
            Capability::Share => "plugins.flutter.io/share",
            other => panic!("unexpected frozen capability {other:?}"),
        };
        let register_row = row(plugin_channel).expect("registered gap");
        assert_eq!(
            entry.gap_reason, register_row.pin,
            "{} gap reason drifted",
            plugin_channel
        );
        assert!(register_row.pin.starts_with("no "));
    }
}

#[test]
fn the_share_contract_is_officially_pinned_and_type_safe() {
    let entry = contract(Capability::Share).expect("share contract");
    assert!(entry.pin.contains("editable_text.dart"));
    assert!(entry.pin.contains("selectable_region.dart"));
    let method = &entry.methods[0];
    assert_eq!(method.name, "Share.invoke");
    assert_eq!(method.argument, ArgType::String);
    assert!(validate(method, &Value::String("text".to_owned())).is_ok());
    assert!(validate(method, &Value::I32(1)).is_err());
}

#[test]
fn the_permissions_contract_is_officially_pinned_and_type_safe() {
    let entry = contract(Capability::Permissions).expect("permissions contract");
    assert!(entry.pin.contains("binding.dart"));
    let method = &entry.methods[0];
    assert_eq!(method.name, "System.exitApplication");
    assert_eq!(method.argument, ArgType::Map);
    assert!(validate(method, &Value::Null).is_err());
}

#[test]
fn the_device_info_and_network_contracts_freeze_nothing() {
    for capability in [Capability::DeviceInfo, Capability::Network] {
        let entry = contract(capability).expect("frozen contract");
        assert!(
            entry.methods.is_empty(),
            "{} has no official pin but freezes methods",
            capability.as_str()
        );
        assert!(entry.pin.starts_with("no official pin"));
    }
}

#[test]
fn the_frozen_contracts_never_implement_anything() {
    // A frozen method is a wire shape, not a dispatched capability method:
    // the frozen names live in the framework namespace (`Share.invoke`,
    // `System.exitApplication`), not in the capability-qualified dispatch
    // table (`share.invoke`).
    for entry in CONTRACTS {
        for method in entry.methods {
            assert!(
                !methods_for(entry.capability).contains(&method.name),
                "{} leaks into the dispatch table",
                method.name
            );
            assert!(
                method.name.contains('.'),
                "{} is not a wire-qualified method",
                method.name
            );
        }
    }
    let all: BTreeSet<&str> = ALL_CAPABILITIES
        .iter()
        .map(|capability| capability.as_str())
        .collect();
    assert!(all.contains("share"));
    assert!(all.contains("network"));
}

#[test]
fn the_artifact_pins_and_the_frozen_pins_form_one_closed_source_set() {
    let artifact = include_str!("fixtures/gap-register.psv");
    let pins: BTreeSet<&str> = artifact
        .lines()
        .filter(|line| line.starts_with("plugins.flutter.io/"))
        .filter_map(|line| line.split('|').nth(6))
        .collect();
    for entry in CONTRACTS {
        assert!(
            pins.contains(entry.gap_reason),
            "{} pin is not in the artifact",
            entry.capability.as_str()
        );
    }
    assert_eq!(pins.len(), 4);
}

#[test]
fn the_return_type_column_is_part_of_the_contract() {
    let share = &contract(Capability::Share).unwrap().methods[0];
    assert_eq!(share.returns, ReturnType::Null);
    let permissions = &contract(Capability::Permissions).unwrap().methods[0];
    assert_eq!(permissions.returns, ReturnType::Map);
    // The frozen types are only data: no method call is ever constructed
    // from them.
    fn validate(method: &FrozenMethod, arguments: &Value) -> Result<(), &'static str> {
        opurom_platform::frozen_contracts::validate_arguments(method, arguments)
    }
    let _ = validate;
}

fn validate(method: &FrozenMethod, arguments: &Value) -> Result<(), &'static str> {
    opurom_platform::frozen_contracts::validate_arguments(method, arguments)
}
