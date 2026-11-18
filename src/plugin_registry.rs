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
//! - `packages/flutter_tools/lib/src/plugins.dart:203` — the `implements`
//!   plugin manifest key; `flutter_plugins.dart:142-145` — the
//!   `environment.flutter` version constraint carried on each plugin.
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:1904-1908` +
//!   `plugins.dart:1900-1910` — desktop self-implementation requires an
//!   explicit Flutter constraint whose minimum is `>= 2.11.0`.
//! - `packages/flutter_tools/lib/src/commands/create_base.dart:731,735-804,811-813`
//!   — Pub package names match `[a-z_][a-z0-9_]*` and are not Dart keywords.
//! - `packages/flutter_tools/lib/src/flutter_plugins.dart:480-498` — the
//!   generated per-platform plugin list carries `name`, `path`,
//!   `native_build` (method-channel or FFI), `shared_darwin_source` and the
//!   plugin-only dependency list.
//! - `packages/flutter_tools/lib/src/platform_plugins.dart:82-96,213-229,343,457`
//!   — `NativeOrDartPlugin.hasMethodChannel/hasFfi`, the `ffiPlugin` flag and
//!   the Darwin `sharedDarwinSource` declaration.
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

    /// Set the stable interface id/version pair carried by the Opurom plugin
    /// manifest. Both halves must be present for an auditable declaration.
    pub fn with_interface(mut self, id: impl Into<String>, version: impl Into<String>) -> Self {
        self.interface = Some((id.into(), version.into()));
        self
    }

    /// The Flutter package this plugin implements (`plugins.dart:203`). An
    /// app-facing plugin implements itself; a platform implementation names
    /// its app-facing interface.
    pub fn implements_package(&self) -> Option<&str> {
        self.interface.as_ref().map(|(id, _)| id.as_str())
    }
}

/// Per-platform native injection descriptor for one manifest, projected from
/// the platform block of a Flutter plugin declaration
/// (`platform_plugins.dart:82-96,213-229,343,457`). `None` fields mean the
/// platform block did not declare the capability; they are never guessed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlatformInjection {
    /// `pluginClass` is present (method-channel implementation).
    pub method_channel: bool,
    /// `ffiPlugin: true` (`platform_plugins.dart:213-229`).
    pub ffi: bool,
    /// `sharedDarwinSource: true` for iOS/macOS
    /// (`platform_plugins.dart:93-96,343,457`).
    pub shared_darwin_source: bool,
}

impl PlatformInjection {
    /// Flutter writes `native_build` when a platform block has a method
    /// channel or FFI implementation (`flutter_plugins.dart:494-497`).
    pub fn native_build(&self) -> bool {
        self.method_channel || self.ffi
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

/// Manifest-level ecosystem errors, raised before a registration plan is
/// built. These are pure-data checks: no files are edited and no plugin code
/// is executed. Each rule cites the Flutter manifest rule it projects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestError {
    /// Package name is not `[a-z_][a-z0-9_]*` or is a Dart keyword
    /// (`create_base.dart:731,811-813`).
    InvalidPackageName { plugin: String, name: String },
    /// A platform implementation names an interface that is not in the
    /// manifest set (`flutter_plugins.dart:1985-1995` finds candidates for an
    /// implemented package and errors when none exists).
    UnknownInterface { plugin: String, interface: String },
    /// Desktop self-implementation needs `environment.flutter` with minimum
    /// `>= 2.11.0` (`flutter_plugins.dart:1904-1908`,
    /// `create_base.dart:811-813`).
    DesktopSelfImplementationNeedsFlutterConstraint { plugin: String },
    /// The declared interface version must match the app-facing package's
    /// `interface` version when both declare one.
    InterfaceVersionMismatch {
        plugin: String,
        interface: String,
        expected: String,
        actual: String,
    },
    /// Two plugins claim to implement the same interface for the same target
    /// platform; Flutter's resolver rejects ambiguous candidates
    /// (`flutter_plugins.dart:1950-1995`).
    AmbiguousImplementation {
        interface: String,
        platform: String,
        first: String,
        second: String,
    },
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::InvalidPackageName { plugin, name } => {
                write!(f, "plugin {plugin} has invalid package name {name}")
            }
            ManifestError::UnknownInterface { plugin, interface } => {
                write!(f, "plugin {plugin} implements unknown interface {interface}")
            }
            ManifestError::DesktopSelfImplementationNeedsFlutterConstraint { plugin } => write!(
                f,
                "plugin {plugin} needs an environment.flutter >= 2.11.0 constraint for desktop self-implementation"
            ),
            ManifestError::InterfaceVersionMismatch {
                plugin,
                interface,
                expected,
                actual,
            } => write!(
                f,
                "plugin {plugin} interface {interface} version {actual} disagrees with {expected}"
            ),
            ManifestError::AmbiguousImplementation {
                interface,
                platform,
                first,
                second,
            } => write!(
                f,
                "interface {interface} on {platform} is implemented by both {first} and {second}"
            ),
        }
    }
}

/// A validation/registration error. No plugin is partially registered: the
/// plan is built whole or rejected whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    Manifest(ManifestError),
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
            RegistryError::Manifest(error) => write!(f, "{error}"),
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

impl std::error::Error for ManifestError {}

impl std::error::Error for RegistryError {}

/// Dart keyword set used by Flutter's Pub package-name validator
/// (`commands/create_base.dart:735-804`). Kept as a sorted slice so the
/// compiled set is a fixed table, with no runtime allocation per check.
const DART_KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "assert",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "covariant",
    "default",
    "deferred",
    "do",
    "dynamic",
    "else",
    "enum",
    "export",
    "extends",
    "extension",
    "external",
    "factory",
    "false",
    "final",
    "finally",
    "for",
    "function",
    "get",
    "hide",
    "if",
    "implements",
    "import",
    "in",
    "inout",
    "interface",
    "is",
    "late",
    "library",
    "mixin",
    "native",
    "new",
    "null",
    "of",
    "on",
    "operator",
    "out",
    "part",
    "patch",
    "required",
    "rethrow",
    "return",
    "set",
    "show",
    "source",
    "static",
    "super",
    "switch",
    "sync",
    "this",
    "throw",
    "true",
    "try",
    "typedef",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// `isValidPackageName` (`commands/create_base.dart:811-813`) checks the
/// full-string match of `[a-z_][a-z0-9_]*` (`:731`) and rejects Dart keywords.
pub fn is_valid_package_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first == '_')
        || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return false;
    }
    !DART_KEYWORDS.binary_search(&name).is_ok()
}

/// A plugin's declared Flutter constraint (`flutter_plugins.dart:142-145`)
/// parsed as a minimum version. Opurom accepts only the leading
/// `>= X.Y.Z` form used by Flutter's desktop self-implementation gate
/// (`flutter_plugins.dart:1904-1908`). Other constraint shapes have no
/// provable minimum and are reported as `None`, never guessed.
pub fn minimum_flutter_version(constraint: &str) -> Option<(u64, u64, u64)> {
    let rest = constraint.trim().strip_prefix(">=")?.trim_start();
    let mut parts = rest.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Check a candidate implementation's desktop self-implementation rule.
/// Flutter gates desktop self-implementation on a Flutter constraint whose
/// minimum is at least 2.11.0 (`flutter_plugins.dart:1904-1908`), because
/// desktop Dart-only self-implementation was not supported before that
/// (`plugins.dart:1900-1910`).
pub fn desktop_self_impl_allowed(flutter_constraint: Option<&str>) -> bool {
    let Some(constraint) = flutter_constraint else {
        return false;
    };
    matches!(minimum_flutter_version(constraint), Some(version) if version >= (2, 11, 0))
}

/// Pure manifest/interface consistency audit over a frozen manifest set.
/// It never edits files, loads plugin code or registers handlers: it answers
/// whether the declarations could be resolved by Flutter's rules
/// (`plugins.dart:203`, `flutter_plugins.dart:142-145,1904-1908,1950-1995`).
///
/// `flutter_constraints` is keyed by plugin name and mirrors each plugin's
/// `environment.flutter` value; absent entries mean "no declared minimum".
pub fn audit_manifests(
    manifests: &[PluginManifest],
    flutter_constraints: &BTreeMap<&str, &str>,
    target_platform: &str,
) -> Result<(), ManifestError> {
    let mut by_name: BTreeMap<&str, &PluginManifest> = BTreeMap::new();
    for manifest in manifests {
        if !is_valid_package_name(&manifest.name) {
            return Err(ManifestError::InvalidPackageName {
                plugin: manifest.name.clone(),
                name: manifest.name.clone(),
            });
        }
        if by_name.insert(manifest.name.as_str(), manifest).is_some() {
            return Err(ManifestError::InvalidPackageName {
                plugin: manifest.name.clone(),
                name: manifest.name.clone(),
            });
        }
    }

    // An `implements` declaration must name a package in the resolved set
    // (`flutter_plugins.dart:1985-1995` errors when no candidate exists).
    for manifest in manifests {
        if let Some((interface, _)) = &manifest.interface {
            if interface != &manifest.name && !by_name.contains_key(interface.as_str()) {
                return Err(ManifestError::UnknownInterface {
                    plugin: manifest.name.clone(),
                    interface: interface.clone(),
                });
            }
        }
    }

    // A platform implementation's version must agree with the app-facing
    // package when both declare one.
    for manifest in manifests {
        if let Some((interface, version)) = &manifest.interface {
            if interface == &manifest.name {
                continue;
            }
            if let Some((_, expected)) = by_name
                .get(interface.as_str())
                .and_then(|target| target.interface.as_ref())
            {
                if expected != version {
                    return Err(ManifestError::InterfaceVersionMismatch {
                        plugin: manifest.name.clone(),
                        interface: interface.clone(),
                        expected: expected.clone(),
                        actual: version.clone(),
                    });
                }
            }
        }
    }

    // Desktop self-implementation needs an explicit >= 2.11.0 minimum.
    let is_desktop = matches!(target_platform, "linux" | "macos" | "windows");
    if is_desktop {
        for manifest in manifests {
            if matches!(&manifest.interface, Some((id, _)) if id == &manifest.name) {
                let constraint = flutter_constraints.get(manifest.name.as_str()).copied();
                if !desktop_self_impl_allowed(constraint) {
                    return Err(
                        ManifestError::DesktopSelfImplementationNeedsFlutterConstraint {
                            plugin: manifest.name.clone(),
                        },
                    );
                }
            }
        }
    }

    // Only one implementation may serve an interface on a target platform.
    // Flutter selects among candidates and rejects multiple direct
    // implementations (`flutter_plugins.dart:1950-1995`).
    let mut claimed: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    for manifest in manifests {
        if !manifest.platforms.is_empty() && !manifest.platforms.contains(target_platform) {
            continue;
        }
        let Some((interface, _)) = &manifest.interface else {
            continue;
        };
        if let Some(first) = claimed.insert(
            (interface.as_str(), target_platform),
            manifest.name.as_str(),
        ) {
            return Err(ManifestError::AmbiguousImplementation {
                interface: interface.clone(),
                platform: target_platform.to_owned(),
                first: first.to_owned(),
                second: manifest.name.clone(),
            });
        }
    }
    Ok(())
}

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

/// A typed reason a manifest cannot be injected on a target platform.
/// Flutter's generator asserts every listed plugin provides an implementation
/// for the platform it is generated under (`flutter_plugins.dart:484-487`);
/// Opurom keeps the same rule as a value instead of an assertion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InjectionGap {
    /// The manifest declares no support for the platform.
    PlatformNotSupported { plugin: String, platform: String },
    /// The platform block declares no method-channel or FFI implementation
    /// and no shared Darwin source, so nothing can be injected natively.
    NoNativeImplementation { plugin: String, platform: String },
    /// `sharedDarwinSource` is declared for a platform other than iOS/macOS.
    SharedDarwinSourceOutsideDarwin { plugin: String, platform: String },
}

impl std::fmt::Display for InjectionGap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InjectionGap::PlatformNotSupported { plugin, platform } => write!(
                f,
                "plugin {plugin} does not support platform {platform}"
            ),
            InjectionGap::NoNativeImplementation { plugin, platform } => write!(
                f,
                "plugin {plugin} has no native implementation on {platform}"
            ),
            InjectionGap::SharedDarwinSourceOutsideDarwin { plugin, platform } => write!(
                f,
                "plugin {plugin} declares sharedDarwinSource on {platform}, which is not iOS or macOS"
            ),
        }
    }
}

impl std::error::Error for InjectionGap {}

/// Audit one plugin's native injection against a target platform. This is a
/// pure function of the declaration: it never edits files, loads plugin code
/// or registers a handler.
///
/// A manifest with no platform filter supports every platform (empty means
/// all, matching the registrant's platform filter). A plugin that supports
/// the platform but declares no native implementation is a typed gap, not a
/// silent no-op (`flutter_plugins.dart:484-497`).
pub fn audit_platform_injection(
    manifest: &PluginManifest,
    injection: &PlatformInjection,
    target_platform: &str,
) -> Result<(), InjectionGap> {
    if !manifest.platforms.is_empty() && !manifest.platforms.contains(target_platform) {
        return Err(InjectionGap::PlatformNotSupported {
            plugin: manifest.name.clone(),
            platform: target_platform.to_owned(),
        });
    }
    if injection.shared_darwin_source && !matches!(target_platform, "ios" | "macos") {
        return Err(InjectionGap::SharedDarwinSourceOutsideDarwin {
            plugin: manifest.name.clone(),
            platform: target_platform.to_owned(),
        });
    }
    if !injection.native_build() && !injection.shared_darwin_source {
        return Err(InjectionGap::NoNativeImplementation {
            plugin: manifest.name.clone(),
            platform: target_platform.to_owned(),
        });
    }
    Ok(())
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
    fn platform_injection_matches_generated_registrant_rules() {
        let android_plugin = PluginManifest::new("camera_android", "1.0.0").on_platform("android");
        let method_channel = PlatformInjection {
            method_channel: true,
            ..PlatformInjection::default()
        };
        assert!(audit_platform_injection(&android_plugin, &method_channel, "android").is_ok());
        assert_eq!(
            audit_platform_injection(&android_plugin, &method_channel, "ios"),
            Err(InjectionGap::PlatformNotSupported {
                plugin: "camera_android".into(),
                platform: "ios".into(),
            })
        );
        let ffi_only = PlatformInjection {
            ffi: true,
            ..PlatformInjection::default()
        };
        assert!(audit_platform_injection(&android_plugin, &ffi_only, "android").is_ok());
        let no_impl = PlatformInjection::default();
        assert_eq!(
            audit_platform_injection(&android_plugin, &no_impl, "android"),
            Err(InjectionGap::NoNativeImplementation {
                plugin: "camera_android".into(),
                platform: "android".into(),
            })
        );
    }

    #[test]
    fn shared_darwin_source_is_confined_to_darwin_platforms() {
        let manifest = PluginManifest::new("url_launcher_darwin", "1.0.0")
            .on_platform("ios")
            .on_platform("macos");
        let shared = PlatformInjection {
            method_channel: true,
            shared_darwin_source: true,
            ..PlatformInjection::default()
        };
        assert!(audit_platform_injection(&manifest, &shared, "ios").is_ok());
        assert!(audit_platform_injection(&manifest, &shared, "macos").is_ok());
        let anywhere = PluginManifest::new("shared", "1.0.0");
        assert_eq!(
            audit_platform_injection(&anywhere, &shared, "linux"),
            Err(InjectionGap::SharedDarwinSourceOutsideDarwin {
                plugin: "shared".into(),
                platform: "linux".into(),
            })
        );
    }

    #[test]
    fn package_names_follow_flutter_pub_rules() {
        assert!(is_valid_package_name("path_provider"));
        assert!(is_valid_package_name("_private"));
        assert!(!is_valid_package_name("class"));
        assert!(!is_valid_package_name("WithCaps"));
        assert!(!is_valid_package_name("has-dash"));
        assert!(!is_valid_package_name(""));
    }

    #[test]
    fn flutter_constraints_require_explicit_minimum_2_11_for_desktop() {
        assert_eq!(minimum_flutter_version(">=2.11.0"), Some((2, 11, 0)));
        assert_eq!(minimum_flutter_version(">=2.10.0"), Some((2, 10, 0)));
        assert_eq!(minimum_flutter_version("any"), None);
        assert!(!desktop_self_impl_allowed(None));
        assert!(!desktop_self_impl_allowed(Some(">=2.10.0")));
        assert!(desktop_self_impl_allowed(Some(">=2.11.0")));
    }

    #[test]
    fn manifest_audit_rejects_unknown_interface_and_mismatched_versions() {
        let impl_plugin = PluginManifest::new("camera_linux", "1.0.0")
            .with_interface("camera", "2.0.0")
            .on_platform("linux");
        let unknown = vec![impl_plugin.clone()];
        assert_eq!(
            audit_manifests(&unknown, &BTreeMap::new(), "linux"),
            Err(ManifestError::UnknownInterface {
                plugin: "camera_linux".into(),
                interface: "camera".into(),
            })
        );
        let app_facing = PluginManifest::new("camera", "1.0.0").with_interface("camera", "1.0.0");
        let mismatch = vec![app_facing, impl_plugin];
        assert_eq!(
            audit_manifests(&mismatch, &BTreeMap::new(), "linux"),
            Err(ManifestError::InterfaceVersionMismatch {
                plugin: "camera_linux".into(),
                interface: "camera".into(),
                expected: "1.0.0".into(),
                actual: "2.0.0".into(),
            })
        );
    }

    #[test]
    fn desktop_self_impl_and_ambiguous_candidates_are_rejected() {
        let self_impl = PluginManifest::new("camera", "1.0.0")
            .with_interface("camera", "1.0.0")
            .on_platform("linux");
        assert_eq!(
            audit_manifests(&[self_impl.clone()], &BTreeMap::new(), "linux"),
            Err(
                ManifestError::DesktopSelfImplementationNeedsFlutterConstraint {
                    plugin: "camera".into(),
                }
            )
        );
        let constraints = BTreeMap::from([("camera", ">=2.11.0")]);
        assert!(audit_manifests(&[self_impl.clone()], &constraints, "linux").is_ok());
        assert!(audit_manifests(&[self_impl.clone()], &constraints, "android").is_ok());
        let ambiguous = vec![
            self_impl,
            PluginManifest::new("camera_alt", "1.0.0")
                .with_interface("camera", "1.0.0")
                .on_platform("linux"),
        ];
        assert_eq!(
            audit_manifests(&ambiguous, &constraints, "linux"),
            Err(ManifestError::AmbiguousImplementation {
                interface: "camera".into(),
                platform: "linux".into(),
                first: "camera".into(),
                second: "camera_alt".into(),
            })
        );
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
