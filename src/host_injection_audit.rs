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
//!
//! Multi-platform source pins (all Flutter 3.50-candidate at
//! `flutter-golden@ab2912035e160d9cad6306df307576da6128ebdb`):
//! - `services/platform_channel.dart:350-366` — a channel without a handler
//!   produces a null reply / `MissingPluginException` on strict channels.
//! - `services/clipboard.dart:36-67` and `services/haptic_feedback.dart:24-147`
//!   — framework-level `flutter/platform` methods with exact payload shapes.
//! - `flutter_tools/lib/src/flutter_plugins.dart:528-551` — platform
//!   implementations are injected one-by-one by generated registrants; the
//!   host may legitimately have no implementation for a channel.

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

/// Fold one bare haptic method name into the exact `flutter/platform`
/// wire method (`haptic_feedback.dart:24-147`).
fn map_haptic_framework_method(method: &str) -> Option<&'static str> {
    use crate::framework_channels::{haptic_method, haptic_type_name, platform_methods};
    if method == platform_methods::HAPTIC_VIBRATE {
        return Some(platform_methods::HAPTIC_VIBRATE);
    }
    let type_name = haptic_type_name(method)?;
    haptic_method(Some(type_name))
}

/// The exact `flutter/platform` wire method for a framework-defined
/// capability method, or `None` when the method belongs to a plugin channel
/// rather than the framework channel. Keeping this mapping as data makes the
/// host-injection boundary auditable without a runtime registry.
pub fn framework_channel_method(capability: Capability, method: &str) -> Option<&'static str> {
    use crate::framework_channels::platform_methods;
    use crate::native_capabilities::Capability as C;

    match (capability, method) {
        (C::Clipboard, "setData") => Some(platform_methods::CLIPBOARD_SET_DATA),
        (C::Clipboard, "getData") => Some(platform_methods::CLIPBOARD_GET_DATA),
        (C::Clipboard, "hasStrings") => Some(platform_methods::CLIPBOARD_HAS_STRINGS),
        (C::Haptics, _) => map_haptic_framework_method(method),
        _ => None,
    }
}

/// A typed host-injection row: for each declared capability method, name the
/// framework wire method (when one exists) and the reply a host must produce
/// when no implementation was injected. Deterministic and allocation-light:
/// only the typed missing reply owns strings, and it is built once per row.
#[derive(Clone, Debug, PartialEq)]
pub struct InjectionSlot {
    pub capability: Capability,
    pub method: &'static str,
    pub framework_method: Option<&'static str>,
    pub missing_reply: Reply,
}

/// Enumerate all injection slots in stable capability/method order.
pub fn injection_slots() -> Vec<InjectionSlot> {
    let mut slots = Vec::new();
    for capability in ALL_CAPABILITIES {
        for method in methods_for(capability) {
            slots.push(InjectionSlot {
                capability,
                method,
                framework_method: framework_channel_method(capability, method),
                missing_reply: missing_handler_reply(capability),
            });
        }
    }
    slots
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
    fn framework_capabilities_map_to_exact_flutter_platform_methods() {
        use crate::framework_channels::platform_methods;
        // Mapping diagnostics: bare vibrate and type name must both resolve.
        assert_eq!(
            crate::framework_channels::haptic_type_name("vibrate"),
            Some(platform_methods::HAPTIC_VIBRATE)
        );
        assert_eq!(
            crate::framework_channels::haptic_method(Some(platform_methods::HAPTIC_VIBRATE)),
            Some(platform_methods::HAPTIC_VIBRATE)
        );
        assert_eq!(
            framework_channel_method(Capability::Clipboard, "setData"),
            Some(platform_methods::CLIPBOARD_SET_DATA)
        );
        assert_eq!(
            framework_channel_method(Capability::Clipboard, "getData"),
            Some(platform_methods::CLIPBOARD_GET_DATA)
        );
        assert_eq!(
            framework_channel_method(Capability::Clipboard, "hasStrings"),
            Some(platform_methods::CLIPBOARD_HAS_STRINGS)
        );
        for haptic in [
            "vibrate",
            "lightImpact",
            "mediumImpact",
            "heavyImpact",
            "selectionClick",
        ] {
            assert_eq!(
                framework_channel_method(Capability::Haptics, haptic),
                Some(platform_methods::HAPTIC_VIBRATE),
                "{haptic} must fold into the one framework method"
            );
        }
        assert_eq!(framework_channel_method(Capability::Camera, "create"), None);
        assert_eq!(framework_channel_method(Capability::Network, "none"), None);
    }

    #[test]
    fn injection_slots_cover_every_capability_method_with_typed_gaps() {
        let slots = injection_slots();
        let expected: usize = ALL_CAPABILITIES
            .iter()
            .map(|capability| methods_for(*capability).len())
            .sum();
        assert_eq!(slots.len(), expected);
        for slot in slots {
            assert_eq!(slot.missing_reply, missing_handler_reply(slot.capability));
            assert_ne!(slot.missing_reply, Reply::Success(Value::Null));
            match &slot.missing_reply {
                Reply::PlatformError {
                    code,
                    message,
                    details,
                } => {
                    assert_eq!(code, error_codes::UNIMPLEMENTED);
                    assert_eq!(*details, Value::Null);
                    if matches!(
                        slot.capability,
                        Capability::Network
                            | Capability::Permissions
                            | Capability::Share
                            | Capability::DeviceInfo
                    ) {
                        assert!(message.as_deref().unwrap_or("").contains("snapshot"));
                    }
                }
                other => panic!("missing handler reply must be typed: {other:?}"),
            }
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
