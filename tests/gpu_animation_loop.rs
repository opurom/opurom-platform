//! Bounded multi-frame GPU animation loop: real on-screen verification plus
//! honest SKIP paths. A host without an Aqua session, Metal device, drawable,
//! software renderer or animated IR reports `SKIP <reason>` and exits 0; no
//! fake PASS is ever produced.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use opurom_platform::{
    classify, read_ppm, ANIMATION_FLAG, ANIMATION_TICKS, CHANNEL_TOLERANCE,
    MISMATCH_BUDGET_PERCENT, PRESENT_METAL_UNAVAILABLE, PRESENT_NO_DRAWABLE, PRESENT_OK,
};

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

fn run(args: &[&str], envs: &[(&str, &str)]) -> (String, i32) {
    let mut command = Command::new(binary());
    command.arg(ANIMATION_FLAG).args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().expect("run opurom-platform");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

fn decode_png(path: &Path) -> Vec<u8> {
    let bytes = std::fs::read(path).expect("the captured PNG must exist");
    assert_eq!(&bytes[..8], &b"\x89PNG\r\n\x1a\n"[..], "real PNG signature");
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().unwrap();
    let mut decoded = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut decoded).unwrap();
    decoded.truncate(info.buffer_size());
    assert_eq!(info.width, 200);
    assert!(info.height > 150);
    decoded
}

fn differing_pixels(lhs: &[u8], rhs: &[u8], width: u32, height: u32) -> u64 {
    let mut count = 0;
    for y in 0..height {
        for x in 0..width {
            let a = ((y * width + x) * 4) as usize;
            let worst = (0..3)
                .map(|c| (lhs[a + c] as i32 - rhs[a + c] as i32).unsigned_abs())
                .max()
                .unwrap();
            if worst > CHANNEL_TOLERANCE {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn tick_sequence_is_bounded_distinct_and_repeatable() {
    assert_eq!(ANIMATION_TICKS.len(), 4);
    assert_eq!(
        ANIMATION_TICKS
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
    assert!(ANIMATION_TICKS.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn the_skip_override_forces_a_loop_skip_and_exits_zero() {
    let (stdout, code) = run(
        &["--out", "/tmp/opurom-loop-forced-skip.png"],
        &[("OPR_SKIP_WINDOW", "1")],
    );
    assert_eq!(code, 0, "a SKIP must exit 0: {stdout}");
    assert!(stdout.starts_with("SKIP"), "expected a SKIP: {stdout}");
    assert!(stdout.contains("OPR_SKIP_WINDOW"), "{stdout}");
    assert!(!stdout.contains("PASS"), "never a fake PASS: {stdout}");
}

#[test]
fn a_missing_gpu_dylib_is_a_concrete_skip() {
    let (stdout, code) = run(
        &["--out", "/tmp/opurom-loop-nodylib.png"],
        &[("OPUROM_GPU_DYLIB", "/nonexistent/no-such-gpu.dylib")],
    );
    assert_eq!(code, 0, "an unavailable dylib is a SKIP: {stdout}");
    assert!(stdout.starts_with("SKIP"), "expected a SKIP: {stdout}");
    assert!(
        stdout.len() > "SKIP ".len(),
        "a SKIP names its reason: {stdout}"
    );
    assert!(!stdout.contains("PASS"), "never a fake PASS: {stdout}");
}

#[test]
fn a_missing_software_renderer_is_a_concrete_skip() {
    let (stdout, code) = run(
        &["--out", "/tmp/opurom-loop-norender.png"],
        &[("OPUROM_RENDER_BIN", "/nonexistent/opurom-render")],
    );
    assert_eq!(code, 0, "a missing renderer is a SKIP: {stdout}");
    assert!(stdout.starts_with("SKIP"), "expected a SKIP: {stdout}");
    assert!(stdout.contains("software renderer"), "{stdout}");
    assert!(!stdout.contains("PASS"), "never a fake PASS: {stdout}");
}

#[test]
fn a_missing_animated_ir_is_a_concrete_skip() {
    let (stdout, code) = run(
        &[
            "--ir",
            "/nonexistent/animated.opuir.json",
            "--out",
            "/tmp/opurom-loop-noir.png",
        ],
        &[],
    );
    assert_eq!(code, 0, "a missing animated IR is a SKIP: {stdout}");
    assert!(stdout.starts_with("SKIP"), "expected a SKIP: {stdout}");
    assert!(stdout.contains("animated IR fixture"), "{stdout}");
    assert!(!stdout.contains("PASS"), "never a fake PASS: {stdout}");
}

#[test]
fn unavailable_device_and_drawable_are_skips_never_passes() {
    assert!(matches!(
        classify(
            PRESENT_METAL_UNAVAILABLE,
            "MTLCreateSystemDefaultDevice returned nil"
        ),
        opurom_platform::PresentationOutcome::Skipped(_)
    ));
    assert!(matches!(
        classify(PRESENT_NO_DRAWABLE, "nextDrawable returned nil"),
        opurom_platform::PresentationOutcome::Skipped(_)
    ));
    assert!(matches!(
        classify(PRESENT_OK, ""),
        opurom_platform::PresentationOutcome::Presented
    ));
}

#[test]
fn the_captured_sequence_matches_the_software_sequence_on_screen() {
    let base = "/tmp/opurom-loop-integration.png";
    let (stdout, code) = run(&["--out", base], &[]);
    let stdout = stdout.trim().to_string();

    if stdout.starts_with("SKIP") {
        assert_eq!(code, 0, "a SKIP must exit 0: {stdout}");
        assert!(
            stdout.len() > "SKIP ".len(),
            "a SKIP names its reason: {stdout}"
        );
        assert!(!stdout.contains("PASS"), "never a fake PASS: {stdout}");
        skip(&stdout);
        return;
    }

    assert_eq!(code, 0, "a PASS must exit 0: {stdout}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), ANIMATION_TICKS.len() + 1, "{stdout}");
    let pass = lines.last().unwrap();
    assert!(pass.starts_with("PASS backend=metal-blit"), "{stdout}");
    for field in [
        "frames=4",
        "ticks=0,96,192,4000",
        "compared_pixels=120000",
        "tolerance=32",
        "budget=2.0%",
        "min_interframe_delta=",
        "min_cross_tick_delta=",
    ] {
        assert!(
            pass.contains(field),
            "the verdict must report {field}: {stdout}"
        );
    }

    for (index, &tick) in ANIMATION_TICKS.iter().enumerate() {
        let line = lines[index];
        assert!(
            line.starts_with(&format!("frame={index} tick={tick} ")),
            "{line}"
        );
        for field in [
            "dispatch_ms=",
            "present_ms=",
            "mismatched_pixels=",
            "compared_pixels=30000",
            "mismatch=0.0000%",
        ] {
            assert!(
                line.contains(field),
                "frame {index} must report {field}: {line}"
            );
        }
    }

    // Independently decode each capture and prove the frames differ from each
    // other by more than the documented tolerance.
    let mut captures = Vec::new();
    for (index, &tick) in ANIMATION_TICKS.iter().enumerate() {
        let path = PathBuf::from(format!(
            "/tmp/opurom-loop-integration-frame{index}-tick{tick}.png"
        ));
        let rgba = decode_png(&path);
        captures.push(rgba[32 * 200 * 4..(32 + 150) * 200 * 4].to_vec());
    }
    let threshold = (200.0 * 150.0 * MISMATCH_BUDGET_PERCENT / 100.0).ceil() as u64;
    for pair in captures.windows(2) {
        let delta = differing_pixels(&pair[0], &pair[1], 200, 150);
        assert!(
            delta > threshold,
            "captured frames at different ticks must differ beyond the budget: {delta}"
        );
    }

    // Each capture must also be tick-specific: it matches its own software
    // reference while clearly differing from another tick's reference.
    let renderer = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../opurom-render/target/debug/opurom-render");
    let ir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../opurom-render/tests/fixtures/animation-loop.opuir.json");
    if renderer.is_file() && ir.is_file() {
        for (index, &tick) in ANIMATION_TICKS.iter().enumerate() {
            let ppm_path = PathBuf::from(format!("/tmp/opurom-loop-ref-{index}.ppm"));
            let output = Command::new(&renderer)
                .arg(&ir)
                .args(["200", "150"])
                .arg(&ppm_path)
                .args(["--tick", &tick.to_string()])
                .output()
                .expect("the software renderer must run");
            assert!(
                output.status.success(),
                "renderer failed: {:?}",
                output.stderr
            );
            let reference = read_ppm(&std::fs::read(&ppm_path).unwrap()).unwrap();
            let own = opurom_platform::compare_rgba_to_ppm(&captures[index], 200, 150, &reference);
            assert!(
                own.is_pass(),
                "frame {index} must match its own reference: {own:?}"
            );
            let other_tick = ANIMATION_TICKS[(index + 1) % ANIMATION_TICKS.len()];
            let other_path = PathBuf::from(format!("/tmp/opurom-loop-ref-other-{index}.ppm"));
            let output = Command::new(&renderer)
                .arg(&ir)
                .args(["200", "150"])
                .arg(&other_path)
                .args(["--tick", &other_tick.to_string()])
                .output()
                .expect("the software renderer must run");
            assert!(
                output.status.success(),
                "renderer failed: {:?}",
                output.stderr
            );
            let other = read_ppm(&std::fs::read(&other_path).unwrap()).unwrap();
            let cross = opurom_platform::compare_rgba_to_ppm(&captures[index], 200, 150, &other);
            assert!(
                cross.mismatched_pixels > threshold,
                "frame {index} must differ from tick {other_tick}'s reference: {cross:?}"
            );
        }
    } else {
        skip("the sibling renderer or fixture is absent; the CLI report stands alone");
    }
}
