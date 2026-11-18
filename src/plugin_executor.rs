//! Deterministic execution of a frozen plugin registration plan.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! `packages/flutter_tools/lib/src/flutter_plugins.dart:528-551` generates
//! one `plugins.add(...)` call per method-channel plugin in dependency order,
//! each inside its own `try/catch` so one failure never stops the others.
//! Opurom keeps that mechanism without a runtime plugin object tree:
//! [`crate::plugin_registry::resolve_registration`] produces a frozen
//! [`RegistrationPlan`], and this module executes it against a compile-time
//! table of typed registration functions. Every step records success or a
//! typed failure; the plan is never mutated.
//!
//! Source pins:
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:528-551` — per-plugin
//!   registration with isolated exception handling.
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:225-292` —
//!   dependency-sorted order (produced by `resolve_registration`).
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:380-420` —
//!   platform-scoped permission merging (produced by `merge_permissions`).

use std::collections::BTreeSet;

use crate::plugin_registry::{merge_permissions, PluginManifest, RegistrationPlan};

/// Result of one registration step. Flutter's generated registrant catches
/// every exception and continues; the typed error keeps the reason without
/// an exception crossing the ABI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepOutcome {
    Registered,
    Failed { reason: String },
}

/// The immutable result of executing a plan. Steps stay in plan order, and
/// the permission set is the union declared by the manifests that actually
/// registered (`flutter_plugins.dart:380-420`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationReport {
    outcomes: Vec<(String, StepOutcome)>,
    permissions: BTreeSet<String>,
}

impl RegistrationReport {
    pub fn outcomes(&self) -> &[(String, StepOutcome)] {
        &self.outcomes
    }

    pub fn permissions(&self) -> &BTreeSet<String> {
        &self.permissions
    }

    /// Names of steps that registered successfully, in plan order.
    pub fn registered(&self) -> Vec<&str> {
        self.outcomes
            .iter()
            .filter(|(_, outcome)| *outcome == StepOutcome::Registered)
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Names of steps that failed, in plan order.
    pub fn failed(&self) -> Vec<&str> {
        self.outcomes
            .iter()
            .filter(|(_, outcome)| matches!(outcome, StepOutcome::Failed { .. }))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    pub fn all_registered(&self) -> bool {
        self.failed().is_empty()
    }
}

/// One compile-time registration function. `name` must match the manifest
/// name exactly; the function returns `Ok(())` on success or a stable typed
/// reason on failure. No runtime object tree is created.
#[derive(Clone, Copy)]
pub struct RegistrationFn {
    pub name: &'static str,
    pub function: fn() -> Result<(), &'static str>,
}

impl std::fmt::Debug for RegistrationFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistrationFn")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Execute a frozen plan. Steps not present in `functions` fail with a typed
/// reason, mirroring Flutter's per-plugin try/catch when a plugin class is
/// missing. Execution is deterministic: the same plan and functions always
/// produce the same report.
pub fn execute_plan(
    plan: &RegistrationPlan,
    functions: &[RegistrationFn],
    manifests: &[PluginManifest],
    permissions: &std::collections::BTreeMap<&str, Vec<&str>>,
    target_platform: &str,
) -> RegistrationReport {
    let mut outcomes = Vec::with_capacity(plan.steps().len());
    let mut registered_names = BTreeSet::new();
    for step in plan.steps() {
        let function = functions.iter().find(|f| f.name == step.name);
        let outcome = match function {
            Some(f) => match (f.function)() {
                Ok(()) => StepOutcome::Registered,
                Err(reason) => StepOutcome::Failed {
                    reason: reason.to_owned(),
                },
            },
            None => StepOutcome::Failed {
                reason: "no registration function".to_owned(),
            },
        };
        if outcome == StepOutcome::Registered {
            registered_names.insert(step.name.as_str());
        }
        outcomes.push((step.name.clone(), outcome));
    }
    let selected: Vec<PluginManifest> = manifests
        .iter()
        .filter(|manifest| registered_names.contains(manifest.name.as_str()))
        .cloned()
        .collect();
    let permission_set = merge_permissions(&selected, permissions, target_platform);
    RegistrationReport {
        outcomes,
        permissions: permission_set,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_registry::resolve_registration;

    fn manifest(name: &str, platform: &str) -> PluginManifest {
        PluginManifest::new(name, "1.0")
            .on_platform(platform)
            .provides(format!("capability.{name}"))
    }

    fn ok() -> Result<(), &'static str> {
        Ok(())
    }

    fn fail() -> Result<(), &'static str> {
        Err("host surface unavailable")
    }

    #[test]
    fn execution_keeps_dependency_order_and_isolates_failure() {
        let manifests = vec![
            manifest("camera", "android"),
            manifest("path", "android").depends_on("camera"),
            manifest("share", "android"),
        ];
        let plan = resolve_registration(&manifests, Some("android")).unwrap();
        assert_eq!(plan.names(), vec!["camera", "path", "share"]);
        let functions = [
            RegistrationFn {
                name: "camera",
                function: ok,
            },
            RegistrationFn {
                name: "path",
                function: fail,
            },
            RegistrationFn {
                name: "share",
                function: ok,
            },
        ];
        let permissions = std::collections::BTreeMap::from([(
            "camera",
            vec!["android.permission.CAMERA", "android.permission.INTERNET"],
        )]);
        let report = execute_plan(&plan, &functions, &manifests, &permissions, "android");
        assert_eq!(report.registered(), vec!["camera", "share"]);
        assert_eq!(report.failed(), vec!["path"]);
        assert_eq!(report.outcomes()[1].0, "path");
        assert_eq!(
            report.outcomes()[1].1,
            StepOutcome::Failed {
                reason: "host surface unavailable".to_owned()
            }
        );
        assert!(!report.all_registered());
        assert_eq!(
            report.permissions(),
            &BTreeSet::from([
                "android.permission.CAMERA".to_owned(),
                "android.permission.INTERNET".to_owned(),
            ])
        );
    }

    #[test]
    fn missing_function_is_a_typed_failure_not_a_panic() {
        let manifests = vec![manifest("only", "ios")];
        let plan = resolve_registration(&manifests, Some("ios")).unwrap();
        let report = execute_plan(
            &plan,
            &[],
            &manifests,
            &std::collections::BTreeMap::new(),
            "ios",
        );
        assert_eq!(report.failed(), vec!["only"]);
        assert_eq!(
            report.outcomes()[0].1,
            StepOutcome::Failed {
                reason: "no registration function".to_owned()
            }
        );
    }

    #[test]
    fn permission_merge_excludes_failed_plugins() {
        let manifests = vec![manifest("a", "linux"), manifest("b", "linux")];
        let plan = resolve_registration(&manifests, Some("linux")).unwrap();
        let functions = [RegistrationFn {
            name: "a",
            function: ok,
        }];
        let permissions =
            std::collections::BTreeMap::from([("a", vec!["perm.a"]), ("b", vec!["perm.b"])]);
        let report = execute_plan(&plan, &functions, &manifests, &permissions, "linux");
        assert_eq!(report.registered(), vec!["a"]);
        assert_eq!(report.permissions(), &BTreeSet::from(["perm.a".to_owned()]));
    }
}
