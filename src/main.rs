use opurom_platform::{frame_to_png, host_platforms, registry, FrameView};

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
