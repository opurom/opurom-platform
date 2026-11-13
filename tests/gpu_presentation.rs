//! Real Metal drawable presentation with on-screen verification.
//!
//! The headline test drives `--verify-gpu-presentation`: a live AppKit window
//! receives the GPU compute buffer through a real `MTLBlitCommandEncoder`,
//! `presentDrawable` and `commit`, then `cacheDisplay(in:to:)` captures the
//! presented frame for the tolerance comparison. A host without an Aqua
//! session, a Metal device, a drawable or the GPU dylib reports `SKIP <reason>`
//! and exits 0; it never claims a PASS.

use std::path::{Path, PathBuf};
use std::process::Command;

use opurom_platform::{
    classify, compare_rgba_to_ppm, present_display_list, read_ppm, workspace_root,
    PresentationOutcome, CHANNEL_TOLERANCE, MISMATCH_BUDGET_PERCENT, PRESENT_METAL_UNAVAILABLE,
    PRESENT_NO_DRAWABLE, PRESENT_OK,
};

const COUNTER_IR: &str = "../opurom-spec/counter.opuir.json";
const WIDTH: u32 = 200;
const HEIGHT: u32 = 150;

/// Absolute path to the built CLI, as in `tests/native_window.rs`.
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
    let _ = writeln!(std::io::stderr().lock(), "SKIP: {message}");
}

/// The sibling software renderer, used for the independent comparison.
fn software_renderer() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("OPUROM_RENDER_BIN") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let path = workspace_root().join("opurom-render/target/debug/opurom-render");
    path.is_file().then_some(path)
}

/// Run `--verify-gpu-presentation` and return `(stdout, exit code)`.
fn run_gpu(out: &Path, dylib: Option<&str>) -> (String, i32) {
    let mut command = Command::new(binary());
    command
        .args(["--verify-gpu-presentation", "--out"])
        .arg(out);
    if let Some(path) = dylib {
        command.env("OPUROM_GPU_DYLIB", path);
    }
    let output = command.output().expect("run opurom-platform");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// The presentation test: real blit/present plus a real on-screen comparison.
#[test]
fn gpu_presentation_matches_the_software_renderer_on_screen() {
    let out = std::env::temp_dir().join(format!("opurom-platform-gpu-{}.png", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let (stdout, code) = run_gpu(&out, None);
    let stdout = stdout.trim().to_string();

    if stdout.starts_with("SKIP") {
        assert_eq!(code, 0, "a SKIP must exit 0: {stdout}");
        assert!(
            stdout.len() > "SKIP ".len(),
            "a SKIP must state the concrete reason: {stdout}"
        );
        assert!(
            !out.exists(),
            "a skipped presentation must not write a capture"
        );
        skip(&stdout);
        return;
    }

    assert!(
        stdout.starts_with("PASS") && stdout.contains("backend=metal-blit"),
        "the GPU presentation must PASS or SKIP, never anything else: {stdout}"
    );
    assert_eq!(code, 0, "a PASS must exit 0: {stdout}");
    for field in [
        "mismatched_pixels=",
        "compared_pixels=",
        "mismatch=",
        "tolerance=32",
        "budget=2.0%",
    ] {
        assert!(
            stdout.contains(field),
            "the verdict must report {field}: {stdout}"
        );
    }

    // The captured PNG exists and carries the live window pixels.
    let bytes = std::fs::read(&out).expect("the captured PNG must exist");
    assert_eq!(&bytes[..8], &b"\x89PNG\r\n\x1a\n"[..], "real PNG signature");
    assert_eq!(&bytes[12..16], &b"IHDR"[..], "real PNG IHDR");
    let png_width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let png_height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    assert_eq!(png_width, WIDTH, "the capture keeps the frame width");
    assert!(
        png_height > HEIGHT,
        "the capture includes the system title bar, so it is taller than the frame"
    );

    // Independent comparison: decode the capture and diff it against the
    // software renderer's PPM, outside the CLI's own arithmetic.
    let Some(renderer) = software_renderer() else {
        skip("the software renderer binary was not found; the CLI report stands alone");
        let _ = std::fs::remove_file(&out);
        return;
    };
    let reference_path = std::env::temp_dir().join(format!(
        "opurom-platform-gpu-ref-{}.ppm",
        std::process::id()
    ));
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
    let (captured_width, captured_height, pixels) = decode_png(&bytes);
    assert_eq!(captured_width, WIDTH);
    assert!(captured_height > HEIGHT);

    let title_bar = captured_height - HEIGHT;
    let row = title_bar as usize * captured_width as usize * 4;
    let comparison = compare_rgba_to_ppm(&pixels[row..], captured_width, HEIGHT, &reference);
    println!(
        "GPU PRESENTATION: captured={}x{} title_bar={}px mismatched_pixels={} compared_pixels={} mismatch={:.4}% max_channel_delta={} tolerance={} budget={}%",
        captured_width,
        captured_height,
        title_bar,
        comparison.mismatched_pixels,
        comparison.compared_pixels,
        comparison.mismatch_percent(),
        comparison.max_channel_delta,
        CHANNEL_TOLERANCE,
        MISMATCH_BUDGET_PERCENT,
    );
    assert!(
        comparison.is_pass(),
        "the presented frame must match the software renderer within the documented tolerance: {comparison:?}"
    );

    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(&reference_path);
}

/// `OPR_SKIP_WINDOW=1` still forces a documented SKIP for the GPU path.
#[test]
fn the_skip_override_forces_a_gpu_skip_and_exits_zero() {
    let out = std::env::temp_dir().join(format!(
        "opurom-platform-gpu-skip-{}.png",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&out);
    let output = Command::new(binary())
        .args(["--verify-gpu-presentation", "--out"])
        .arg(&out)
        .env("OPR_SKIP_WINDOW", "1")
        .output()
        .expect("run opurom-platform");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(0),
        "a forced SKIP exits 0: {stdout}"
    );
    assert!(
        stdout.starts_with("SKIP"),
        "OPR_SKIP_WINDOW must SKIP: {stdout}"
    );
    assert!(
        stdout.contains("OPR_SKIP_WINDOW"),
        "the SKIP must name the override: {stdout}"
    );
    assert!(
        !out.exists(),
        "a skipped presentation must not write a capture"
    );
}

/// A missing GPU dylib is a concrete SKIP, never a fake PASS.
#[test]
fn a_missing_gpu_dylib_is_a_concrete_skip() {
    let out = std::env::temp_dir().join(format!(
        "opurom-platform-gpu-nodylib-{}.png",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&out);
    let (stdout, code) = run_gpu(&out, Some("/nonexistent/opurom-gpu-does-not-exist.dylib"));
    let stdout = stdout.trim().to_string();
    assert_eq!(
        code, 0,
        "an unavailable dylib is a SKIP, not a FAIL: {stdout}"
    );
    assert!(stdout.starts_with("SKIP"), "expected a SKIP: {stdout}");
    if cfg!(target_os = "macos") {
        assert!(
            stdout.contains("OPUROM_GPU_DYLIB"),
            "the SKIP must name the missing dylib: {stdout}"
        );
    } else {
        assert!(
            stdout.contains("macOS/Metal only"),
            "the SKIP must name the platform limit: {stdout}"
        );
    }
    assert!(!stdout.contains("PASS"), "never a fake PASS: {stdout}");
    assert!(!out.exists());
}

/// A null layer never fabricates a success, and every unavailable status maps
/// to an honest SKIP verdict.
#[test]
fn the_presentation_status_vocabulary_is_honest() {
    let outcome = present_display_list("{\"version\":1}", std::ptr::null_mut());
    assert!(
        !matches!(outcome, PresentationOutcome::Presented),
        "a null layer must not be a pass: {outcome:?}"
    );
    match &outcome {
        PresentationOutcome::Skipped(reason) | PresentationOutcome::Failed(reason) => {
            assert!(!reason.is_empty(), "every non-pass carries a reason")
        }
        PresentationOutcome::Presented => unreachable!("checked above"),
    }

    assert!(matches!(
        classify(PRESENT_NO_DRAWABLE, "no drawable"),
        PresentationOutcome::Skipped(reason) if reason == "no drawable"
    ));
    assert!(matches!(
        classify(PRESENT_METAL_UNAVAILABLE, "no device"),
        PresentationOutcome::Skipped(reason) if reason == "no device"
    ));
    assert!(matches!(
        classify(PRESENT_OK, ""),
        PresentationOutcome::Presented
    ));
    assert!(matches!(
        classify(PRESENT_OK, "unexpected"),
        PresentationOutcome::Failed(_)
    ));
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
