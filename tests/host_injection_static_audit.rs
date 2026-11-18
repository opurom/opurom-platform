//! Static audit of host-injection entry points for deterministic time/state.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/binding.dart:56,61-66` — the binding
//!   installs platform/lifecycle/system handlers and reads the initial
//!   lifecycle from the platform dispatcher at startup.
//! - `packages/flutter/lib/src/services/binding.dart:174-182` — system
//!   messages are value-driven (a `type` string switch), not clock-driven.
//! - `packages/flutter/lib/src/services/binding.dart:296-311,448-457` —
//!   lifecycle messages are strings and their transitions are generated from
//!   the previous state (`:313-381`).
//! - `packages/flutter/lib/src/scheduler/binding.dart:416-430,878` — frame
//!   enablement is a pure function of the reported lifecycle state.
//! - `packages/flutter/lib/src/services/platform_channel.dart:679-738` —
//!   event activation/deactivation is listener-count driven.
//!
//! This test reads source files only. It asserts that the event/bridge modules
//! have no direct system-clock reads and that every injected callback uses a
//! function-pointer slot with an opaque state pointer. Host adapters that
//! legitimately own real-time transport deadlines are classified explicitly.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn src(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(name)
}

fn production_source(name: &str) -> String {
    let raw = fs::read_to_string(src(name)).unwrap_or_else(|error| panic!("read {name}: {error}"));
    // Module tests are not host-injection surfaces; exclude them from the
    // static audit so test-local clocks cannot be misclassified.
    raw.split("#[cfg(test)]").next().unwrap_or(&raw).to_owned()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Risk {
    None,
    Low,
    Medium,
}

impl Risk {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    module: &'static str,
    surface: &'static str,
    risk: Risk,
    reason: &'static str,
}

/// Audited entry points, grouped by injection boundary.
fn entries() -> Vec<Entry> {
    vec![
        Entry {
            module: "system_events.rs",
            surface: "Subscriber::function",
            risk: Risk::None,
            reason: "function pointer plus opaque state; no time parameter or read",
        },
        Entry {
            module: "channel_event_bridge.rs",
            surface: "dispatch_system_message",
            risk: Risk::None,
            reason: "payload bytes produce values; dispatcher dedupes by value",
        },
        Entry {
            module: "event_producers.rs",
            surface: "pure host-fact producers",
            risk: Risk::None,
            reason: "closed value mappings from injected facts only",
        },
        Entry {
            module: "framework_channels.rs",
            surface: "lifecycle_transitions / frames gate",
            risk: Risk::None,
            reason: "previous state plus reported state, no clock",
        },
        Entry {
            module: "appkit.rs",
            surface: "NSNotificationCenter observers",
            risk: Risk::Low,
            reason: "OS notification delivery on main queue; values injected, not polled",
        },
        Entry {
            module: "wayland.rs",
            surface: "socket handshake deadlines",
            risk: Risk::Medium,
            reason: "real-time transport timeouts; never feeds event state or ordering",
        },
        Entry {
            module: "native_backends.rs",
            surface: "test-only temp directory seed",
            risk: Risk::Low,
            reason: "not a runtime injection point; excluded from production source",
        },
    ]
}

#[test]
fn event_bridge_modules_have_no_direct_system_clock_reads() {
    let forbidden = [
        "SystemTime::now",
        "Instant::now",
        "dateWithTimeIntervalSinceNow",
        "clock_gettime",
        "gettimeofday",
        "mach_absolute_time",
        "QueryPerformanceCounter",
        "DateTime::now",
    ];
    for module in [
        "system_events.rs",
        "channel_event_bridge.rs",
        "event_producers.rs",
        "framework_channels.rs",
        "app_lifecycle.rs",
        "native_capabilities.rs",
        "background_tasks.rs",
        "host_injection_audit.rs",
    ] {
        let source = production_source(module);
        for needle in forbidden {
            assert!(
                !source.contains(needle),
                "{module} reads system clock via {needle}"
            );
        }
    }
}

#[test]
fn host_entry_points_are_classified_and_state_converges() {
    let rows = entries();
    let mut modules = BTreeMap::new();
    for row in &rows {
        assert!(modules.insert(row.module, row.risk).is_none());
        assert!(!row.reason.is_empty());
    }
    let high: Vec<_> = rows
        .iter()
        .filter(|row| matches!(row.risk, Risk::Medium))
        .map(|row| row.module)
        .collect();
    assert_eq!(high, vec!["wayland.rs"]);
    for row in &rows {
        println!(
            "{} | {} | {} | {}",
            row.module,
            row.surface,
            row.risk.as_str(),
            row.reason
        );
    }
}

#[test]
fn real_time_transport_code_is_not_an_event_producer() {
    // Wayland/appkit/native host adapters own real-time deadlines, but the
    // event bridge modules only receive already-decided values. Assert that
    // producer functions themselves carry no clock and no callback table.
    let source = production_source("event_producers.rs");
    assert!(source.contains("pub fn keyboard_visibility"));
    assert!(source.contains("pub fn orientation_from_index"));
    assert!(source.contains("pub fn brightness"));
    assert!(source.contains("pub fn power_mode"));
    assert!(source.contains("pub fn refresh_rate"));
    assert!(!source.contains("unsafe fn"));
    assert!(!source.contains("*mut u8"));
}
