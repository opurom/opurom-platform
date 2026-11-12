//! R7: live native window with on-screen pixel verification.
//!
//! The headline test opens a REAL AppKit window, captures its own pixels with
//! `cacheDisplay(in:to:)` and compares them against the software renderer's
//! PPM for the same IR. On a host without an Aqua session the check reports
//! SKIP with the concrete reason and still exits 0; it never claims a PASS.

use std::path::{Path, PathBuf};
use std::process::Command;

use opurom_platform::{compare_rgba_to_ppm, read_ppm, CHANNEL_TOLERANCE, MISMATCH_BUDGET_PERCENT};

const COUNTER_IR: &str = "../opurom-spec/counter.opuir.json";
const WIDTH: u32 = 200;
const HEIGHT: u32 = 150;

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

fn skip(message: &str) {
    use std::io::Write;
    // Direct stderr writes keep the SKIP notice visible without --nocapture.
    let _ = writeln!(std::io::stderr().lock(), "SKIP: {message}");
}

/// The sibling software renderer, used to produce the reference PPM.
fn software_renderer() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("OPUROM_RENDER_BIN") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let path = PathBuf::from("../opurom-render/target/debug/opurom-render");
    path.is_file().then_some(path)
}

/// Run the CLI verification and return `(stdout, exit code)`.
fn run_verification(out: &Path) -> (String, i32) {
    let output = Command::new(binary())
        .args(["--verify-window", "--out"])
        .arg(out)
        .output()
        .expect("run opurom-platform --verify-window");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// The pixel-verification test: a real window, a real capture and a real
/// comparison against the software renderer.
#[test]
fn live_window_pixels_match_the_software_renderer() {
    let out = std::env::temp_dir().join(format!(
        "opurom-platform-r7-window-{}.png",
        std::process::id()
    ));

    let (stdout, code) = run_verification(&out);
    let stdout = stdout.trim().to_string();

    if stdout.starts_with("SKIP") {
        // A headless host reports SKIP with the concrete reason and exits 0.
        assert_eq!(code, 0, "a SKIP must exit 0: {stdout}");
        assert!(
            stdout.len() > "SKIP ".len(),
            "a SKIP must state the reason: {stdout}"
        );
        skip(&stdout);
        let _ = std::fs::remove_file(&out);
        return;
    }

    assert!(
        stdout.starts_with("PASS"),
        "the live window verification must PASS or SKIP, never anything else: {stdout}"
    );
    assert_eq!(code, 0, "a PASS must exit 0: {stdout}");

    // The captured PNG must exist and carry real pixels.
    let bytes = std::fs::read(&out).expect("the captured window PNG must exist");
    assert_eq!(&bytes[..8], &b"\x89PNG\r\n\x1a\n"[..], "real PNG signature");
    assert_eq!(&bytes[12..16], &b"IHDR"[..], "real PNG IHDR");
    let png_width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let png_height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    assert_eq!(png_width, WIDTH, "the captured window keeps its width");
    assert!(
        png_height > HEIGHT,
        "the captured window includes the system title bar, so it is taller than the frame"
    );

    // Decode the capture and compare it against the software renderer's PPM
    // for the same IR, independently of the CLI's own arithmetic.
    let Some(renderer) = software_renderer() else {
        skip("the software renderer binary was not found; the CLI report stands alone");
        let _ = std::fs::remove_file(&out);
        return;
    };
    let reference_path =
        std::env::temp_dir().join(format!("opurom-platform-r7-ref-{}.ppm", std::process::id()));
    let render = Command::new(&renderer)
        .arg(COUNTER_IR)
        .args([WIDTH.to_string(), HEIGHT.to_string()])
        .arg(&reference_path)
        .output()
        .expect("the software renderer must run");
    assert!(
        render.status.success(),
        "the software renderer failed: {}",
        String::from_utf8_lossy(&render.stderr)
    );
    let reference = read_ppm(&std::fs::read(&reference_path).unwrap()).unwrap();
    assert_eq!(reference.width, WIDTH);
    assert_eq!(reference.height, HEIGHT);

    let captured = decode_png(&bytes);
    assert_eq!(captured.0, WIDTH, "the capture keeps the frame width");
    assert!(captured.1 > HEIGHT, "the capture includes the title bar");

    // The content region starts below the system title bar.
    let title_bar_height = captured.1 - HEIGHT;
    let row = title_bar_height as usize * captured.0 as usize * 4;
    let content = &captured.2[row..];
    let comparison = compare_rgba_to_ppm(content, captured.0, HEIGHT, &reference);
    println!(
        "LIVE WINDOW: captured={}x{} title_bar={}px mismatched_pixels={} compared_pixels={} mismatch={:.4}% max_channel_delta={} tolerance={} budget={}%",
        captured.0,
        captured.1,
        title_bar_height,
        comparison.mismatched_pixels,
        comparison.compared_pixels,
        comparison.mismatch_percent(),
        comparison.max_channel_delta,
        CHANNEL_TOLERANCE,
        MISMATCH_BUDGET_PERCENT,
    );
    assert!(
        comparison.is_pass(),
        "the captured window pixels must match the software renderer within the documented tolerance: {comparison:?}"
    );

    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(&reference_path);
}

/// `OPR_SKIP_WINDOW=1` forces a SKIP with the concrete reason, on any host.
#[test]
fn the_skip_override_forces_a_skip_and_exits_zero() {
    let out = std::env::temp_dir().join(format!(
        "opurom-platform-r7-skip-{}.png",
        std::process::id()
    ));
    let output = Command::new(binary())
        .args(["--verify-window", "--out"])
        .arg(&out)
        .env("OPR_SKIP_WINDOW", "1")
        .output()
        .expect("run opurom-platform --verify-window");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(0),
        "a forced SKIP must exit 0: {stdout}"
    );
    assert!(
        stdout.starts_with("SKIP"),
        "OPR_SKIP_WINDOW=1 must report SKIP: {stdout}"
    );
    assert!(
        stdout.contains("OPR_SKIP_WINDOW"),
        "the SKIP must name the override: {stdout}"
    );
    assert!(
        !out.exists(),
        "a skipped verification must not write a window capture"
    );
}

/// The documented tolerance is part of the contract, not a magic number.
#[test]
fn the_tolerance_is_documented() {
    assert_eq!(CHANNEL_TOLERANCE, 32, "documented per-channel tolerance");
    assert_eq!(MISMATCH_BUDGET_PERCENT, 2.0, "documented mismatch budget");
}

/// The pixel comparison must reject a genuinely wrong frame.
#[test]
fn a_wrong_frame_is_reported_as_a_mismatch() {
    let reference = opurom_platform::PpmImage {
        width: 2,
        height: 2,
        pixels: vec![79, 70, 229, 79, 70, 229, 79, 70, 229, 79, 70, 229],
    };
    let wrong = [0u8; 16];
    let comparison = compare_rgba_to_ppm(&wrong, 2, 2, &reference);
    assert_eq!(comparison.mismatched_pixels, 4);
    assert_eq!(comparison.compared_pixels, 4);
    assert!(!comparison.is_pass());
}

/// Decode a PNG into `(width, height, RGBA8 bytes)`.
fn decode_png(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().unwrap();
    let mut decoded = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut decoded).unwrap();
    decoded.truncate(info.buffer_size());
    (info.width, info.height, decoded)
}
