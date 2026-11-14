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

fn run(args: &[&str]) -> String {
    let output = Command::new(binary())
        .args(args)
        .output()
        .expect("run opurom-platform binary");
    assert!(output.status.success());
    String::from_utf8(output.stdout).expect("stdout is utf-8")
}

#[test]
fn default_output_keeps_the_14_unique_target_rows() {
    let out = run(&[]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0],
        "name | emitter | status | gpu | native_embed | multi_window"
    );
    assert_eq!(lines.len(), 15, "header + exactly 14 target data rows");
    let names: Vec<&str> = lines[1..]
        .iter()
        .map(|line| line.split(" | ").next().unwrap())
        .collect();
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 14, "14 unique target names");
    assert!(!names.contains(&"terminal"), "terminal is not a target row");
    assert!(!names.contains(&"headless"), "headless is not a target row");
    assert!(!names.contains(&"appkit"), "appkit is not a target row");
}

#[test]
fn hosts_flag_lists_the_verified_hosts() {
    let out = run(&["--hosts"]);
    let lines: Vec<&str> = out.lines().collect();
    let expected_lines = if cfg!(target_os = "macos") { 4 } else { 3 };
    assert_eq!(
        lines.len(),
        expected_lines,
        "header + cross-platform hosts + macOS appkit"
    );
    let data = &lines[1..];
    let names: Vec<&str> = data
        .iter()
        .map(|line| line.split(" | ").next().unwrap())
        .collect();
    assert!(names.contains(&"headless"));
    assert!(names.contains(&"terminal"));
    let terminal = data
        .iter()
        .find(|line| line.starts_with("terminal"))
        .unwrap();
    assert!(
        terminal.contains(" | Verified | false | false | false"),
        "terminal is a Verified thin host with no GPU / native embed / multi window"
    );
}
