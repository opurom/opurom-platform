#[cfg(target_os = "macos")]
use opurom_platform::Verification;
use opurom_platform::{frame_to_png, host_platforms, registry, FrameView};

#[cfg(target_os = "macos")]
use opurom_platform::{
    compare_capture, read_ppm, render_reference_ppm, software_renderer_path, window_availability,
    PpmImage, REFERENCE_PPM_ENV, SKIP_WINDOW_ENV,
};
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

const PNG_WIDTH: u32 = 4;
const PNG_HEIGHT: u32 = 4;

/// A deterministic 4x4 RGBA8 gradient, independent of clocks and device state.
fn png_pixels() -> Vec<u8> {
    let mut pixels = Vec::with_capacity((PNG_WIDTH * PNG_HEIGHT * 4) as usize);
    for y in 0..PNG_HEIGHT {
        for x in 0..PNG_WIDTH {
            pixels.extend_from_slice(&[(x * 64) as u8, (y * 64) as u8, ((x + y) * 32) as u8, 255]);
        }
    }
    pixels
}

fn write_test_png(path: &str) {
    let pixels = png_pixels();
    let frame = FrameView {
        width: PNG_WIDTH,
        height: PNG_HEIGHT,
        pixels: &pixels,
    };
    if let Err(error) = frame_to_png(frame, path) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

/// The default reference IR of the OPUROM project.
#[cfg(target_os = "macos")]
const DEFAULT_IR: &str = "../opurom-spec/counter.opuir.json";
#[cfg(target_os = "macos")]
const DEFAULT_WIDTH: u32 = 200;
#[cfg(target_os = "macos")]
const DEFAULT_HEIGHT: u32 = 150;

/// Run the on-screen verification and print its verdict. Returns the process
/// exit code: 0 for PASS and SKIP, 1 for FAIL.
#[cfg(target_os = "macos")]
fn verify_window(args: &[String]) -> i32 {
    if std::env::var(SKIP_WINDOW_ENV).is_ok() {
        let verification = Verification::Skipped {
            reason: format!("{SKIP_WINDOW_ENV} is set"),
        };
        println!("{}", verification.report());
        return 0;
    }
    if let Err(reason) = window_availability() {
        let verification = Verification::Skipped {
            reason: reason.to_string(),
        };
        println!("{}", verification.report());
        return 0;
    }

    let ir = args
        .iter()
        .position(|arg| arg == "--ir")
        .and_then(|index| args.get(index + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_IR));
    let width = args
        .iter()
        .position(|arg| arg == "--width")
        .and_then(|index| args.get(index + 1))
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_WIDTH);
    let height = args
        .iter()
        .position(|arg| arg == "--height")
        .and_then(|index| args.get(index + 1))
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_HEIGHT);
    let png_path = args
        .iter()
        .position(|arg| arg == "--out")
        .and_then(|index| args.get(index + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("opurom-window.png"));

    // The reference frame comes from the software renderer, either from a
    // pre-rendered PPM or by running the sibling renderer binary.
    let reference_ppm = match std::env::var(REFERENCE_PPM_ENV) {
        Ok(path) => {
            let path = PathBuf::from(path);
            match std::fs::read(&path)
                .map_err(|error| error.to_string())
                .and_then(|bytes| read_ppm(&bytes).map_err(|error| error.to_string()))
            {
                Ok(image) => image,
                Err(error) => {
                    println!("SKIP cannot read the reference PPM {path:?}: {error}");
                    return 0;
                }
            }
        }
        Err(_) => {
            let Some(renderer) = software_renderer_path() else {
                println!(
                    "SKIP the software renderer was not found at ../opurom-render/target/debug/opurom-render (set {REFERENCE_PPM_ENV} to a pre-rendered PPM)"
                );
                return 0;
            };
            let reference_path = png_path.with_extension("ppm");
            if let Err(error) = render_reference_ppm(&renderer, &ir, width, height, &reference_path)
            {
                println!("SKIP the software renderer failed: {error}");
                return 0;
            }
            let bytes = match std::fs::read(&reference_path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    println!("SKIP cannot read the reference PPM {reference_path:?}: {error}");
                    return 0;
                }
            };
            match read_ppm(&bytes) {
                Ok(image) => image,
                Err(error) => {
                    println!("SKIP the reference PPM is malformed: {error}");
                    return 0;
                }
            }
        }
    };

    let verification = match run_window_capture(&ir, width, height, &png_path, &reference_ppm) {
        Ok(verification) => verification,
        Err(reason) => Verification::Skipped { reason },
    };
    println!("{}", verification.report());
    match verification {
        Verification::Failed { .. } => 1,
        _ => 0,
    }
}

/// Open the real window on the process main thread, capture it and compare it.
///
/// AppKit only allows window creation on the process main thread, so this runs
/// directly in `main()`; the integration test drives it through this binary.
#[cfg(target_os = "macos")]
fn run_window_capture(
    ir: &Path,
    width: u32,
    height: u32,
    png_path: &Path,
    reference: &PpmImage,
) -> Result<Verification, String> {
    use opurom_platform::{capture_window, draw_and_capture, write_png, MacHost};

    let host = MacHost::new(width, height, png_path);
    let window = host.open_window().map_err(|error| error.to_string())?;
    // The frame under test is the software renderer's own output, so the
    // window shows exactly the pixels the comparison uses.
    let frame_pixels = render_frame_png(ir, width, height)?;
    let frame = FrameView {
        width,
        height,
        pixels: &frame_pixels,
    };
    draw_and_capture(&window, frame).map_err(|error| error.to_string())?;

    // The PNG is the whole window, system title bar included.
    let captured = capture_window(&window).map_err(|error| error.to_string())?;
    write_png(captured.width, captured.height, &captured.pixels, png_path)
        .map_err(|error| error.to_string())?;

    // The comparison uses the content region only: the system title bar is
    // AppKit chrome, not part of the rendered frame.
    let content_height = captured.height.saturating_sub(captured.title_bar_height);
    let content_row = captured.title_bar_height as usize * captured.width as usize * 4;
    let content = FrameView {
        width: captured.width,
        height: content_height,
        pixels: &captured.pixels[content_row..],
    };
    let comparison = compare_capture(&content, reference).map_err(|error| error.to_string())?;
    let size = (captured.width, content_height);
    Ok(if comparison.is_pass() {
        Verification::Passed {
            captured: size,
            comparison,
            png: png_path.to_path_buf(),
        }
    } else {
        Verification::Failed {
            captured: size,
            comparison,
            png: png_path.to_path_buf(),
        }
    })
}

/// Render the reference frame into raw RGBA8 bytes with the software renderer.
#[cfg(target_os = "macos")]
fn render_frame_png(ir: &Path, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let Some(renderer) = software_renderer_path() else {
        return Err(
            "the software renderer was not found at ../opurom-render/target/debug/opurom-render"
                .to_string(),
        );
    };
    let ppm_path = std::env::temp_dir().join(format!(
        "opurom-platform-frame-{}-{width}x{height}.ppm",
        std::process::id()
    ));
    render_reference_ppm(&renderer, ir, width, height, &ppm_path)
        .map_err(|error| error.to_string())?;
    let bytes = std::fs::read(&ppm_path).map_err(|error| error.to_string())?;
    let _ = std::fs::remove_file(&ppm_path);
    let image = read_ppm(&bytes).map_err(|error| error.to_string())?;
    let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for chunk in image.pixels.chunks_exact(3) {
        pixels.push(chunk[0]);
        pixels.push(chunk[1]);
        pixels.push(chunk[2]);
        pixels.push(255);
    }
    Ok(pixels)
}

/// The on-screen verification is macOS only; other hosts report SKIP.
#[cfg(not(target_os = "macos"))]
fn verify_window(_args: &[String]) -> i32 {
    println!("SKIP the native window verification is macOS only");
    0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(index) = args.iter().position(|arg| arg == "--png") {
        let Some(path) = args.get(index + 1) else {
            eprintln!("--png requires an output path");
            std::process::exit(2);
        };
        write_test_png(path);
        return;
    }
    if args.iter().any(|arg| arg == "--verify-window") {
        std::process::exit(verify_window(&args));
    }

    // Default output stays the 14 target rows; `--hosts` lists Verified hosts.
    let hosts = args.iter().any(|arg| arg == "--hosts");
    let specs = if hosts { host_platforms() } else { registry() };
    println!("name | emitter | status | gpu | native_embed | multi_window");
    for spec in specs {
        let caps = spec.capabilities;
        println!(
            "{} | {} | {} | {} | {} | {}",
            spec.name,
            spec.emitter,
            spec.status.as_str(),
            caps.gpu,
            caps.native_embed,
            caps.multi_window,
        );
    }
}
