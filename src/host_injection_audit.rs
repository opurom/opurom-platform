//! Compile-time audit of host capability injection points.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Flutter plugin registrants insert each plugin's platform implementation
//! into the engine (`flutter_tools/lib/src/flutter_plugins.dart:528-551`); a
//! platform that does not implement a method leaves the channel unanswered
//! and `MethodChannel` raises `MissingPluginException`
//! (`services/platform_channel.dart:350-366`). Opurom follows the same rule:
//! [`crate::native_capabilities::CapabilityTable`] accepts typed handlers,
//! and a capability without one must return a typed `unimplemented` reply.
//!
//! This module is a pure audit: it validates that every declared capability
//! has a closed method table and that a missing handler never fabricates
//! success. It does not create handlers, timers or runtime registries.

use crate::native_capabilities::{methods_for, Capability, ALL_CAPABILITIES};
use crate::platform_channel::{error_codes, Reply, Value};

/// Audit result for one capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityAudit {
    pub capability: Capability,
    pub method_count: usize,
    pub handler_required: bool,
    pub gap_reason: Option<&'static str>,
}

/// Stable audit rows for all declared capabilities, in ABI order.
pub fn audit_all() -> Vec<CapabilityAudit> {
    ALL_CAPABILITIES
        .iter()
        .copied()
        .map(|capability| CapabilityAudit {
            capability,
            method_count: methods_for(capability).len(),
            handler_required: methods_for(capability).len() > 0
                && capability.unavailable_reason().is_none(),
            gap_reason: capability.unavailable_reason(),
        })
        .collect()
}

/// The reply a capability with no injected handler must return. It mirrors
/// Flutter's missing-plugin distinction instead of answering `Success(Null)`.
pub fn missing_handler_reply(capability: Capability) -> Reply {
    Reply::PlatformError {
        code: error_codes::UNIMPLEMENTED.to_owned(),
        message: Some(
            capability
                .unavailable_reason()
                .unwrap_or("no host handler registered")
                .to_owned(),
        ),
        details: Value::Null,
    }
}

/// Verify that a qualified call to an unregistered capability yields the
/// typed missing-handler reply. Deterministic: no state beyond the table.
pub fn assert_missing_handler(
    table: &crate::native_capabilities::CapabilityTable,
    capability: Capability,
) -> bool {
    let Some(method) = methods_for(capability).first() else {
        return false;
    };
    let call = crate::native_capabilities::qualified(capability, method, Value::Null);
    // SAFETY: an empty table holds no state pointers, so no handler runs.
    let reply = unsafe { table.call(&call) };
    reply == missing_handler_reply(capability)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_capabilities::CapabilityTable;

    #[test]
    fn every_declared_capability_has_an_audit_row() {
        let rows = audit_all();
        assert_eq!(rows.len(), ALL_CAPABILITIES.len());
        let mut seen = std::collections::BTreeSet::new();
        for row in rows {
            assert!(seen.insert(row.capability));
            assert!(row.method_count > 0 || row.gap_reason.is_some());
        }
    }

    #[test]
    fn missing_handlers_never_fabricate_success() {
        let table = CapabilityTable::new();
        for capability in ALL_CAPABILITIES {
            // A capability whose method table is intentionally empty (no
            // pinned plugin source) has no wire method to dispatch; it stays
            // unimplemented by construction.
            if !methods_for(capability).is_empty() {
                assert!(assert_missing_handler(&table, capability), "{capability:?}");
            }
            assert_ne!(
                missing_handler_reply(capability),
                Reply::Success(Value::Null)
            );
        }
    }

    #[test]
    fn method_call_shape_stays_codec_neutral() {
        let call = crate::native_capabilities::qualified(
            Capability::Preferences,
            "getString",
            Value::Null,
        );
        assert_eq!(call.method, "preferences.getString");
        assert_eq!(call.arguments, Value::Null);
    }
}
