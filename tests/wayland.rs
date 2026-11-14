//! The Wayland adapter reports an honest SKIP without a compositor.
//!
//! The wire encoding itself is verified by the pure unit tests in
//! `src/wayland_wire.rs`, which run on every host. This file checks the
//! runtime behaviour: a host without a Wayland compositor reports
//! `SKIP <concrete reason>` and exits 0; it never claims a PASS.

use std::path::PathBuf;
use std::process::Command;

/// Absolute path to the built CLI. Cargo exposes it to integration tests as
/// `CARGO_BIN_EXE_<name>` (hyphen or underscore spelling across versions); fall
/// back to the manifest-relative debug path when neither is defined.
fn binary() -> PathBuf {
    option_env!("CARGO_BIN_EXE_opurom_platform")
        .or_else(|| option_env!("CARGO_BIN_EXE_opurom-platform"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/opurom-platform")
        })
}

/// Run the CLI without a compositor and return `(stdout, exit code)`.
fn run_wayland(display: Option<&str>) -> (String, i32) {
    let mut command = Command::new(binary());
    command.arg("--verify-wayland");
    match display {
        Some(name) => {
            command.env("WAYLAND_DISPLAY", name);
        }
        None => {
            command.env_remove("WAYLAND_DISPLAY");
        }
    }
    let output = command
        .output()
        .expect("run opurom-platform --verify-wayland");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn a_missing_display_is_a_concrete_skip_and_exits_zero() {
    let (stdout, code) = run_wayland(None);
    assert_eq!(code, 0, "a SKIP must exit 0: {stdout}");
    assert!(stdout.starts_with("SKIP"), "expected a SKIP: {stdout}");
    assert!(
        stdout.contains("WAYLAND_DISPLAY"),
        "the SKIP must name the missing variable: {stdout}"
    );
    assert!(
        !stdout.contains("PASS"),
        "a host without a compositor must never claim a PASS: {stdout}"
    );
}

#[test]
fn an_empty_display_is_a_concrete_skip_and_exits_zero() {
    let (stdout, code) = run_wayland(Some(""));
    assert_eq!(code, 0, "a SKIP must exit 0: {stdout}");
    assert!(stdout.starts_with("SKIP"), "expected a SKIP: {stdout}");
    assert!(
        stdout.contains("WAYLAND_DISPLAY"),
        "the SKIP must name the empty variable: {stdout}"
    );
}

#[test]
fn a_named_display_without_a_compositor_is_never_a_pass() {
    // A display name that no compositor serves still reports a concrete SKIP
    // on a host without Wayland, and never a fabricated PASS.
    let (stdout, code) = run_wayland(Some("wayland-0"));
    assert_eq!(code, 0, "a SKIP must exit 0: {stdout}");
    assert!(
        stdout.starts_with("SKIP"),
        "a host without a compositor must report a SKIP: {stdout}"
    );
    assert!(
        !stdout.contains("PASS"),
        "a host without a compositor must never claim a PASS: {stdout}"
    );
}
