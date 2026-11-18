//! Frozen API contracts for capabilities with no local plugin source.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Four capabilities in `native_capabilities` have no plugin source in the
//! local `flutter-plugins` snapshot: `device_info`, `network`,
//! `permissions` and `share`. They stay typed `unimplemented`. But a host
//! adapter that later implements them needs a contract to write against,
//! and inventing one after the fact would drift from Flutter. This module
//! freezes the wire shape of each capability from the official Flutter
//! sources — method names, argument keys, argument types and return
//! types — without implementing any of it.
//!
//! Three of the four have a direct framework pin:
//! - `share` is `Share.invoke` on `flutter/platform`, carrying the plain
//!   text selection (`widgets/editable_text.dart:3025-3037`,
//!   `widgets/selectable_region.dart:1595-1600`).
//! - `clipboard`-adjacent shapes prove the argument-key style.
//! - `System.exitApplication` on `flutter/platform` freezes the exit
//!   request payload the permissions surface shares its exit path with
//!   (`services/binding.dart:523-528`,
//!   `engine/src/flutter/lib/ui/platform_dispatcher.dart:2481-2498`).
//!
//! `device_info` and `network` and `permissions` have no framework or
//! plugin pin, so only their `unimplemented` status is frozen here; their
//! rows carry a `None` contract and the register's GAP reason is the only
//! statement the bridge makes about them. Nothing is invented.
//!
//! No ABI/IR change: the frozen contracts are pure data. Every handler
//! still answers `unimplemented` until a host injects one.

use crate::framework_channels::platform_methods;
use crate::native_capabilities::Capability;
use crate::platform_channel::Value;

/// The wire contract of one frozen capability surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrozenMethod {
    /// The method name on the wire.
    pub name: &'static str,
    /// All argument map keys, when the official call passes a map.
    /// Flutter sends a complete argument map, so validation must require
    /// every key the pinned call writes (`binding.dart:525-528`).
    pub argument_keys: &'static [&'static str],
    /// The official argument type.
    pub argument: ArgType,
    /// The official return type.
    pub returns: ReturnType,
}

/// The official argument type of a frozen method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArgType {
    /// No arguments at all.
    None,
    /// A bare `String` (`Share.invoke` passes the selected text directly).
    String,
    /// A `Map<String, Object?>` with the frozen key.
    Map,
}

impl ArgType {
    pub fn as_str(self) -> &'static str {
        match self {
            ArgType::None => "none",
            ArgType::String => "string",
            ArgType::Map => "map",
        }
    }
}

/// The official return type of a frozen method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReturnType {
    /// The platform answers null; the framework ignores the result.
    Null,
    /// The platform answers a `Map<String, Object?>` with the frozen key.
    Map,
}

impl ReturnType {
    pub fn as_str(self) -> &'static str {
        match self {
            ReturnType::Null => "null",
            ReturnType::Map => "map",
        }
    }
}

/// The frozen contract of one capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrozenContract {
    pub capability: Capability,
    /// The capability's GAP reason, from
    /// `native_capabilities::unavailable_reason`.
    pub gap_reason: &'static str,
    /// The frozen method table, empty when no official pin exists.
    pub methods: &'static [FrozenMethod],
    /// The official source anchor.
    pub pin: &'static str,
}

/// `Share.invoke` passes the plain text selection as a bare `String`
/// argument and returns null (`widgets/editable_text.dart:3025-3037`,
/// `widgets/selectable_region.dart:1595-1600`).
pub const SHARE_METHODS: &[FrozenMethod] = &[FrozenMethod {
    name: platform_methods::SHARE_INVOKE,
    argument_keys: &[],
    argument: ArgType::String,
    returns: ReturnType::Null,
}];

/// The framework exit path the permissions capability would ride on. The
/// payload is `{type: cancelable|required, exitCode: int}` and the reply is
/// `{response: cancel|exit}` (`services/binding.dart:523-528`).
pub const EXIT_METHODS: &[FrozenMethod] = &[FrozenMethod {
    name: platform_methods::SYSTEM_EXIT_APPLICATION,
    argument_keys: &["type", "exitCode"],
    argument: ArgType::Map,
    returns: ReturnType::Map,
}];

/// The frozen contracts, one per capability with a local GAP.
pub const CONTRACTS: &[FrozenContract] = &[
    FrozenContract {
        capability: Capability::Share,
        gap_reason: "no share plugin source in the local snapshot; Share.invoke is frozen on flutter/platform",
        methods: SHARE_METHODS,
        pin: "widgets/editable_text.dart:3025-3037; widgets/selectable_region.dart:1595-1600",
    },
    FrozenContract {
        capability: Capability::Permissions,
        gap_reason: "no permission plugin source in the local snapshot",
        methods: EXIT_METHODS,
        pin: "services/binding.dart:523-528; engine/src/flutter/lib/ui/platform_dispatcher.dart:2481-2498",
    },
    FrozenContract {
        capability: Capability::DeviceInfo,
        gap_reason: "no device_info plugin source in the local snapshot; local projections are not parity",
        methods: &[],
        pin: "no official pin; local projections are not parity",
    },
    FrozenContract {
        capability: Capability::Network,
        gap_reason: "no connectivity plugin source in the local snapshot",
        methods: &[],
        pin: "no official pin; local projections are not parity",
    },
];

/// The frozen contract of one capability.
pub fn contract(capability: Capability) -> Option<&'static FrozenContract> {
    CONTRACTS
        .iter()
        .find(|entry| entry.capability == capability)
}

/// Validate a call against a frozen method. The frozen contract is a wire
/// shape, not an implementation: this only checks that the arguments the
/// caller passes are the ones the official call carries. A host adapter
/// should call this before forwarding to the platform.
pub fn validate_arguments(method: &FrozenMethod, arguments: &Value) -> Result<(), &'static str> {
    match method.argument {
        ArgType::None => {
            if matches!(arguments, Value::Null) {
                Ok(())
            } else {
                Err("frozen method takes no arguments")
            }
        }
        ArgType::String => match arguments {
            Value::String(_) => Ok(()),
            _ => Err("frozen method takes a bare string"),
        },
        ArgType::Map => {
            let Value::Map(entries) = arguments else {
                return Err("frozen method takes a map");
            };
            if method.argument_keys.is_empty() {
                return Err("frozen method map has no keys");
            }
            for key in method.argument_keys {
                let probe = Value::String((*key).to_owned());
                if !entries.contains_key(&probe) {
                    return Err("frozen method map is missing a required key");
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn every_capability_gap_has_a_frozen_contract() {
        use crate::native_capabilities::ALL_CAPABILITIES;
        let gapped: Vec<Capability> = ALL_CAPABILITIES
            .iter()
            .copied()
            .filter(|capability| capability.unavailable_reason().is_some())
            .collect();
        assert_eq!(gapped.len(), 4);
        for capability in gapped {
            let entry = contract(capability).expect("frozen contract");
            assert_eq!(entry.capability, capability);
            // The frozen reason is the register's full wording; both start
            // with the capability's typed `unavailable_reason`.
            let typed = capability.unavailable_reason().expect("gap reason");
            assert!(
                entry.gap_reason.starts_with(typed),
                "{} frozen reason drifted from the typed GAP reason",
                capability.as_str()
            );
            assert!(!entry.pin.is_empty());
        }
    }

    #[test]
    fn share_keeps_the_official_wire_shape() {
        let entry = contract(Capability::Share).expect("share contract");
        assert_eq!(entry.methods.len(), 1);
        let method = &entry.methods[0];
        assert_eq!(method.name, "Share.invoke");
        assert_eq!(method.argument, ArgType::String);
        assert!(method.argument_keys.is_empty());
        assert_eq!(method.returns, ReturnType::Null);

        // The official call passes the selected text directly.
        assert_eq!(
            validate_arguments(method, &Value::String("hello".to_owned())),
            Ok(())
        );
        assert!(validate_arguments(method, &Value::I32(1)).is_err());
        assert!(validate_arguments(method, &Value::Null).is_err());
    }

    #[test]
    fn permissions_ride_on_the_framework_exit_shape() {
        let entry = contract(Capability::Permissions).expect("permissions contract");
        assert_eq!(entry.methods.len(), 1);
        let method = &entry.methods[0];
        assert_eq!(method.name, "System.exitApplication");
        assert_eq!(method.argument, ArgType::Map);
        assert_eq!(method.argument_keys, &["type", "exitCode"]);
        assert_eq!(method.returns, ReturnType::Map);

        let valid = Value::Map(BTreeMap::from([
            (
                Value::String("type".to_owned()),
                Value::String("required".to_owned()),
            ),
            (Value::String("exitCode".to_owned()), Value::I32(0)),
        ]));
        assert_eq!(validate_arguments(method, &valid), Ok(()));
        // Flutter writes both keys (`binding.dart:525-528`): dropping either
        // one is a malformed call, not a partial success.
        let missing_type = Value::Map(BTreeMap::from([(
            Value::String("exitCode".to_owned()),
            Value::I32(0),
        )]));
        let missing_exit_code = Value::Map(BTreeMap::from([(
            Value::String("type".to_owned()),
            Value::String("required".to_owned()),
        )]));
        assert_eq!(
            validate_arguments(method, &missing_type),
            Err("frozen method map is missing a required key")
        );
        assert_eq!(
            validate_arguments(method, &missing_exit_code),
            Err("frozen method map is missing a required key")
        );
        assert!(validate_arguments(method, &Value::Null).is_err());
    }

    #[test]
    fn unpinned_capabilities_freeze_only_their_gap_status() {
        for capability in [Capability::DeviceInfo, Capability::Network] {
            let entry = contract(capability).expect("frozen contract");
            assert!(
                entry.methods.is_empty(),
                "{} has no official pin, so it must freeze no methods",
                capability.as_str()
            );
            assert!(entry.pin.starts_with("no official pin"));
        }
    }

    #[test]
    fn the_contracts_are_the_only_gap_rows() {
        let names: BTreeSet<&str> = CONTRACTS
            .iter()
            .map(|entry| entry.capability.as_str())
            .collect();
        assert_eq!(
            names,
            BTreeSet::from(["share", "permissions", "device_info", "network"])
        );
    }
}
