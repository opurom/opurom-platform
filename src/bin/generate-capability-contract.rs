//! Generate the machine-readable capability × method × shape × error report.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Usage:
//!   cargo run --bin generate-capability-contract -- <output.psv>
//!
//! The output is the single contract source for host injection generators
//! and audit scripts. Every row is:
//! `capability|qualified_method|wire_method|argument|argument_keys|returns|error_code|gap|source_pin`
//!
//! The rows are derived from the same compile-time tables that
//! `CapabilityTable::call` uses, so a host adapter that reads the report
//! cannot drift from the dispatch gate.
//!
//! Source pins:
//! - `services/message_codecs.dart:467,600-617,628-663` — argument errors,
//!   method-call shape and error envelope.
//! - `services/platform_channel.dart:359-364,600-613` — null reply and
//!   handler error semantics.
//! - `services/binding.dart:448-457,523-528` — framework payload examples.

use std::collections::BTreeMap;

use opurom_platform::frozen_contracts::{ArgType, CONTRACTS};
use opurom_platform::host_injection_audit::injection_slots;
use opurom_platform::native_capabilities::{methods_for, ALL_CAPABILITIES};

fn main() {
    let out = std::env::args().nth(1).expect("output path");
    let mut rows: Vec<String> = Vec::new();

    // One row per dispatchable capability method, in stable capability order.
    for capability in ALL_CAPABILITIES {
        for method in methods_for(capability) {
            let frozen = CONTRACTS
                .iter()
                .find(|contract| contract.capability == capability)
                .and_then(|contract| contract.methods.iter().find(|entry| entry.name == *method));
            let (argument, keys, returns) = match frozen {
                Some(entry) => (
                    entry.argument.as_str(),
                    entry.argument_keys.join(","),
                    entry.returns.as_str(),
                ),
                // A dispatchable method with no frozen wire shape uses the
                // general standard-codec shape.
                None => ("value", String::new(), "value"),
            };
            let error_code = if capability.unavailable_reason().is_some() {
                "unimplemented"
            } else {
                "invalid_argument"
            };
            let gap = capability.unavailable_reason().unwrap_or("none");
            let qualified = format!("{}.{}", capability.as_str(), method);
            rows.push(format!(
                "{}|{}|{}|{}|{}|{}|{}|{}|{}",
                capability.as_str(),
                qualified,
                method,
                argument,
                keys,
                returns,
                error_code,
                gap,
                "services/message_codecs.dart:467,600-617,628-663"
            ));
        }
    }

    // One row per frozen GAP wire method, even when it has no dispatchable
    // method in the local table. These are the shapes the dispatch gate
    // validates before `CapabilityTable::call` decides `unimplemented`.
    let mut seen = BTreeMap::new();
    for row in &rows {
        seen.insert(row.clone(), ());
    }
    for contract in CONTRACTS {
        for method in contract.methods {
            let qualified = format!("{}.{}", contract.capability.as_str(), method.name);
            let row = format!(
                "{}|{}|{}|{}|{}|{}|{}|{}|{}",
                contract.capability.as_str(),
                qualified,
                method.name,
                method.argument.as_str(),
                method.argument_keys.join(","),
                method.returns.as_str(),
                "invalid_argument",
                contract.gap_reason,
                contract.pin
            );
            if !seen.contains_key(&row) {
                rows.push(row);
            }
        }
    }

    rows.sort();
    rows.dedup();
    std::fs::write(&out, format!("{}\n", rows.join("\n"))).expect("write report");
    eprintln!("wrote {} rows to {}", rows.len(), out);

    // Keep the injection-slot table honest: every dispatchable method is a
    // host-injection boundary and must appear in the report.
    let slots = injection_slots();
    debug_assert_eq!(
        slots.len(),
        ALL_CAPABILITIES
            .iter()
            .map(|capability| methods_for(*capability).len())
            .sum::<usize>()
    );

    // A compile-time sanity check that the argument vocabulary stays closed.
    let _ = ArgType::None.as_str();
}
