//! Deterministic plugin registration contract.
//!
//! Flutter baseline: 3.50-candidate, engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Source pins:
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:225-292` —
//!   `sortByDependencies`: dependencies before dependents, alphabetical tie
//!   break, deterministic cycle breaking by smallest in-degree then name.
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:528-551` —
//!   `GeneratedPluginRegistrant` generation: one `plugins.add` call per
//!   method-channel plugin in sorted order, every call in its own try/catch.
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:307-350` —
//!   `.flutter-plugins-dependencies` manifest: `dependencies` array of plugin
//!   names, filtered to plugin dependencies only.
//!
//! Opurom keeps the algorithm and the failure isolation, not the class
//! structure: there is no runtime `PluginRegistry` object tree. A
//! [`PluginManifest`] list is resolved once into a frozen
//! [`RegistrationPlan`], which native hosts execute by calling their own
//! typed registration functions. `RegistrationPlan` is deterministic across
//! runs and platforms, so the same plugin set produces the same order.

use std::collections::{BTreeMap, BTreeSet};

/// One plugin entry, equivalent to a Flutter package's `pubspec` + platform
/// declaration projected into a stable value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    /// Other plugin names this plugin depends on. Non-plugin dependencies
    /// are filtered out before building this list, matching Flutter's
    /// `where(dep => pluginNames.contains(dep))`
    /// (`flutter_plugins.dart:498,513`).
    pub dependencies: Vec<String>,
    /// Capability strings this plugin provides, e.g. `camera.takePhoto`.
    pub capabilities: Vec<String>,
    /// Native platforms this plugin declares support for. Empty means all.
    pub platforms: BTreeSet<String>,
    /// Optional stable interface id/version from the Opurom plugin manifest.
    pub interface: Option<(String, String)>,
}

impl PluginManifest {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            dependencies: Vec::new(),
            capabilities: Vec::new(),
            platforms: BTreeSet::new(),
            interface: None,
        }
    }

    pub fn depends_on(mut self, name: impl Into<String>) -> Self {
        self.dependencies.push(name.into());
        self
    }

    pub fn provides(mut self, capability: impl Into<String>) -> Self {
        self.capabilities.push(capability.into());
        self
    }

    pub fn on_platform(mut self, platform: impl Into<String>) -> Self {
        self.platforms.insert(platform.into());
        self
    }
}

/// A frozen registration step. Native hosts iterate this in order and call
/// their own registration function for each name; a failure in one step is
/// isolated, mirroring Flutter's per-plugin try/catch
/// (`flutter_plugins.dart:541-550`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationStep {
    pub name: String,
    pub version: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationPlan {
    steps: Vec<RegistrationStep>,
    /// Cycles broken by Flutter's deterministic rule, recorded for auditing.
    cycle_breaks: Vec<String>,
}

impl RegistrationPlan {
    pub fn steps(&self) -> &[RegistrationStep] {
        &self.steps
    }

    pub fn cycle_breaks(&self) -> &[String] {
        &self.cycle_breaks
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Names in registration order.
    pub fn names(&self) -> Vec<&str> {
        self.steps.iter().map(|s| s.name.as_str()).collect()
    }
}

/// A validation/registration error. No plugin is partially registered: the
/// plan is built whole or rejected whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    DuplicatePlugin(String),
    UnknownDependency {
        plugin: String,
        dependency: String,
    },
    SelfDependency(String),
    DuplicateCapability {
        capability: String,
        first: String,
        second: String,
    },
    InterfaceMismatch {
        plugin: String,
        target: String,
    },
    InvalidName(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::DuplicatePlugin(name) => write!(f, "duplicate plugin {name}"),
            RegistryError::UnknownDependency { plugin, dependency } => {
                write!(f, "plugin {plugin} depends on unknown plugin {dependency}")
            }
            RegistryError::SelfDependency(name) => write!(f, "plugin {name} depends on itself"),
            RegistryError::DuplicateCapability {
                capability,
                first,
                second,
            } => write!(
                f,
                "capability {capability} is claimed by both {first} and {second}"
            ),
            RegistryError::InterfaceMismatch { plugin, target } => {
                write!(f, "plugin {plugin} interface disagrees with {target}")
            }
            RegistryError::InvalidName(name) => write!(f, "invalid plugin name {name}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Resolves manifests into a deterministic registration plan.
///
/// The algorithm is a direct translation of Flutter's `sortByDependencies`
/// (`flutter_plugins.dart:231-292`): compute in-degree over plugin-to-plugin
/// edges, keep the zero-in-degree set sorted alphabetically, emit one name at
/// a time, decrement dependents, re-sort newly available names. Cycles are
/// broken by taking the unplaced plugin with smallest in-degree, then
/// alphabetically — no arbitrary hash-map iteration.
pub fn resolve_registration(
    manifests: &[PluginManifest],
    target_platform: Option<&str>,
) -> Result<RegistrationPlan, RegistryError> {
    // Filter by platform first: Flutter generates a registrant only for
    // plugins that declare support for the host platform.
    let selected: Vec<&PluginManifest> = manifests
        .iter()
        .filter(|m| {
            m.platforms.is_empty()
                || target_platform.is_none()
                || m.platforms.contains(target_platform.unwrap())
        })
        .collect();

    let mut by_name: BTreeMap<&str, &PluginManifest> = BTreeMap::new();
    for manifest in &selected {
        if manifest.name.is_empty()
            || !manifest
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(RegistryError::InvalidName(manifest.name.clone()));
        }
        if by_name.insert(&manifest.name, manifest).is_some() {
            return Err(RegistryError::DuplicatePlugin(manifest.name.clone()));
        }
    }

    // Capability claims must be unique: two plugins answering the same
    // method would make dispatch ambiguous. This is the Opurom equivalent of
    // Flutter's one-handler-per-channel rule.
    let mut claims: BTreeMap<&str, &str> = BTreeMap::new();
    for manifest in &selected {
        for capability in &manifest.capabilities {
            if let Some(first) = claims.insert(capability, &manifest.name) {
                return Err(RegistryError::DuplicateCapability {
                    capability: capability.clone(),
                    first: first.to_owned(),
                    second: manifest.name.clone(),
                });
            }
        }
    }

    // Interface declarations must agree between a plugin and its targets.
    for manifest in &selected {
        if let Some((ref id, ref version)) = manifest.interface {
            for dependency in &manifest.dependencies {
                if let Some(target) = by_name.get(dependency.as_str()) {
                    if let Some((ref target_id, ref target_version)) = target.interface {
                        if id != target_id || version != target_version {
                            return Err(RegistryError::InterfaceMismatch {
                                plugin: manifest.name.clone(),
                                target: dependency.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    // Build the dependency graph, ignoring self and non-plugin edges as
    // Flutter does (`flutter_plugins.dart:243-245`).
    let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for name in by_name.keys() {
        in_degree.insert(name, 0);
        dependents.insert(name, Vec::new());
    }
    for (name, manifest) in &by_name {
        if manifest.dependencies.iter().any(|dep| dep == name) {
            return Err(RegistryError::SelfDependency((*name).to_owned()));
        }
        let direct: BTreeSet<&str> = manifest
            .dependencies
            .iter()
            .map(String::as_str)
            .filter(|dep| *dep != *name && by_name.contains_key(*dep))
            .collect();
        for dep in direct {
            *in_degree.get_mut(name).unwrap() += 1;
            dependents.get_mut(dep).unwrap().push(name);
        }
    }

    let mut available: Vec<&str> = in_degree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(name, _)| *name)
        .collect();
    available.sort_unstable();

    let mut steps = Vec::with_capacity(by_name.len());
    let mut placed = BTreeSet::new();
    let mut cycle_breaks = Vec::new();

    while placed.len() < by_name.len() {
        if available.is_empty() {
            // Deterministic cycle breaking, identical to Flutter's
            // `unplaced.sort(inDegree, then name)` selection.
            let mut unplaced: Vec<&str> = by_name
                .keys()
                .copied()
                .filter(|name| !placed.contains(*name))
                .collect();
            unplaced.sort_by(|a, b| in_degree[a].cmp(&in_degree[b]).then_with(|| a.cmp(b)));
            let chosen = unplaced[0];
            cycle_breaks.push(chosen.to_owned());
            available.push(chosen);
        }
        let current = available.remove(0);
        if placed.insert(current) {
            let manifest = by_name[current];
            steps.push(RegistrationStep {
                name: current.to_owned(),
                version: manifest.version.clone(),
                capabilities: manifest.capabilities.clone(),
            });
            let mut newly_available = Vec::new();
            for dependent in &dependents[current] {
                let degree = in_degree.get_mut(dependent).unwrap();
                *degree = degree.saturating_sub(1);
                if *degree == 0 && !placed.contains(dependent) {
                    newly_available.push(*dependent);
                }
            }
            if !newly_available.is_empty() {
                available.extend(newly_available);
                available.sort_unstable();
            }
        }
    }

    Ok(RegistrationPlan {
        steps,
        cycle_breaks,
    })
}

/// Permissions declared by a plugin, merged into a per-platform set. Flutter
/// builds this by reading each plugin's Android manifest / iOS Info.plist and
/// merging into the host project (`flutter_plugins.dart:380-420`). Opurom
/// takes the same union semantics over explicit declarations, with no file
/// editing at runtime.
pub fn merge_permissions(
    manifests: &[PluginManifest],
    permissions: &BTreeMap<&str, Vec<&str>>,
    target_platform: &str,
) -> BTreeSet<String> {
    let mut merged = BTreeSet::new();
    for manifest in manifests {
        if !manifest.platforms.is_empty() && !manifest.platforms.contains(target_platform) {
            continue;
        }
        if let Some(granted) = permissions.get(manifest.name.as_str()) {
            for permission in granted {
                merged.insert((*permission).to_owned());
            }
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugins() -> Vec<PluginManifest> {
        vec![
            PluginManifest::new("zeta", "1.0.0").depends_on("alpha"),
            PluginManifest::new("alpha", "1.0.0"),
            PluginManifest::new("middle", "1.0.0").depends_on("alpha"),
        ]
    }

    #[test]
    fn dependencies_come_before_dependents_with_alphabetical_ties() {
        let plan = resolve_registration(&plugins(), None).unwrap();
        assert_eq!(plan.names(), vec!["alpha", "middle", "zeta"]);
        assert!(plan.cycle_breaks.is_empty());
    }

    #[test]
    fn cycles_are_broken_deterministically() {
        let manifests = vec![
            PluginManifest::new("b", "1.0.0").depends_on("c"),
            PluginManifest::new("a", "1.0.0").depends_on("b"),
            PluginManifest::new("c", "1.0.0").depends_on("a"),
        ];
        let first = resolve_registration(&manifests, None).unwrap();
        let second = resolve_registration(&manifests, None).unwrap();
        assert_eq!(first.names(), second.names());
        assert_eq!(first.names().len(), 3);
        // All three have in-degree 1, so the alphabetical name is chosen.
        assert_eq!(first.cycle_breaks(), &["a".to_owned()]);
    }

    #[test]
    fn duplicate_capabilities_are_rejected() {
        let manifests = vec![
            PluginManifest::new("a", "1.0.0").provides("camera.takePhoto"),
            PluginManifest::new("b", "1.0.0").provides("camera.takePhoto"),
        ];
        let error = resolve_registration(&manifests, None).unwrap_err();
        assert!(matches!(error, RegistryError::DuplicateCapability { .. }));
    }

    #[test]
    fn unknown_dependencies_are_ignored_like_flutter() {
        // Flutter filters `where(pluginNames.contains(dep))`, so a pubspec
        // dependency on a non-plugin package is not an error.
        let manifests = vec![PluginManifest::new("a", "1.0.0").depends_on("http")];
        let plan = resolve_registration(&manifests, None).unwrap();
        assert_eq!(plan.names(), vec!["a"]);
    }

    #[test]
    fn platform_filtering_matches_the_generated_registrant() {
        let manifests = vec![
            PluginManifest::new("android-only", "1.0.0").on_platform("android"),
            PluginManifest::new("all", "1.0.0"),
        ];
        let plan = resolve_registration(&manifests, Some("ios")).unwrap();
        assert_eq!(plan.names(), vec!["all"]);
        let plan = resolve_registration(&manifests, Some("android")).unwrap();
        assert_eq!(plan.names(), vec!["all", "android-only"]);
    }

    #[test]
    fn permission_merge_is_a_deterministic_union() {
        let manifests = vec![
            PluginManifest::new("camera", "1.0.0").on_platform("android"),
            PluginManifest::new("location", "1.0.0").on_platform("android"),
        ];
        let permissions = BTreeMap::from([
            ("camera", vec!["android.permission.CAMERA"]),
            ("location", vec!["android.permission.ACCESS_FINE_LOCATION"]),
        ]);
        let merged = merge_permissions(&manifests, &permissions, "android");
        assert!(merged.contains("android.permission.CAMERA"));
        assert!(merged.contains("android.permission.ACCESS_FINE_LOCATION"));
        assert!(merge_permissions(&manifests, &permissions, "ios").is_empty());
    }
}
