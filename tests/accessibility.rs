//! The a11y CLI reports an honest PASS/SKIP without faking permission.

use std::path::PathBuf;
use std::process::Command;

fn binary() -> PathBuf {
    option_env!("CARGO_BIN_EXE_opurom_platform")
        .or_else(|| option_env!("CARGO_BIN_EXE_opurom-platform"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/opurom-platform")
        })
}

#[test]
fn the_forced_a11y_skip_makes_no_os_call() {
    let output = Command::new(binary())
        .arg("--post-a11y")
        .env("OPR_SKIP_A11Y", "1")
        .output()
        .expect("run opurom-platform --post-a11y");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.starts_with("SKIP"), "{stdout}");
    assert!(stdout.contains("OPR_SKIP_A11Y"), "{stdout}");
    assert!(
        stdout.contains("no Accessibility API was called"),
        "{stdout}"
    );
}

#[test]
fn the_a11y_cli_reports_pass_or_a_concrete_skip() {
    let output = Command::new(binary())
        .arg("--post-a11y")
        .env_remove("OPR_SKIP_A11Y")
        .output()
        .expect("run opurom-platform --post-a11y");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let report = stdout.trim();
    eprintln!("a11y CLI result: {report}");
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    if report.starts_with("PASS") {
        assert!(report.contains("a11y_nodes=9"), "{stdout}");
    } else {
        assert!(report.starts_with("SKIP "), "{stdout}");
        assert!(report.len() > "SKIP ".len(), "a SKIP must give a reason");
    }
}
