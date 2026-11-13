//! Bounded, deterministic multi-frame GPU animation loop.
//!
//! The software renderer emits the reference frame and the exact display list
//! at each animation tick. Every display list is rasterized and presented by
//! the Metal backend through a real `CAMetalLayer` drawable, and the live
//! window is captured after each present. The captured sequence is compared
//! with the software sequence at the same ticks; a captured frame must also
//! differ from the software reference of every other tick and from the
//! neighbouring captured frame, which proves the layer surface is refreshed
//! on every frame rather than only the first.

#[cfg(any(target_os = "macos", test))]
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use crate::CHANNEL_TOLERANCE;
#[cfg(any(target_os = "macos", test))]
use crate::{compare_rgba_to_ppm, PpmImage, MISMATCH_BUDGET_PERCENT};

/// Distinct, deterministic animation clock positions in milliseconds. The last
/// one is the settled position, so the sequence covers entry and rest.
pub const ANIMATION_TICKS: [i64; 4] = [0, 96, 192, 4000];

/// Canonical animated IR, shared with the renderer's fixture tests.
pub const ANIMATION_IR: &str = "opurom-render/tests/fixtures/animation-loop.opuir.json";

/// The new CLI flag; `--verify-gpu-presentation` keeps its single-frame output.
pub const ANIMATION_FLAG: &str = "--verify-gpu-animation-loop";

/// Ticks are ordered, distinct and bounded, so no frame is presented twice.
pub fn ticks_are_distinct(ticks: &[i64]) -> bool {
    !ticks.is_empty() && ticks.len() <= 8 && ticks.windows(2).all(|pair| pair[0] < pair[1])
}

#[cfg(target_os = "macos")]
fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convert captured RGBA8 content to an RGB8 reference for capture-to-capture
/// comparison with the same channel tolerance as the software oracle.
#[cfg(any(target_os = "macos", test))]
fn rgb_from_rgba(rgba: &[u8], width: u32, height: u32) -> PpmImage {
    let mut pixels = Vec::with_capacity(rgba.len() / 4 * 3);
    for pixel in rgba.chunks_exact(4) {
        pixels.extend_from_slice(&pixel[..3]);
    }
    PpmImage {
        width,
        height,
        pixels,
    }
}

/// Pixels whose worst channel differs by more than `CHANNEL_TOLERANCE`.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn differing_pixels(lhs: &[u8], rhs: &[u8], width: u32, height: u32) -> u64 {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4));
    if expected != Some(lhs.len()) || lhs.len() != rhs.len() {
        return u64::MAX;
    }
    compare_rgba_to_ppm(lhs, width, height, &rgb_from_rgba(rhs, width, height)).mismatched_pixels
}

/// Smallest difference that is clearly outside the documented mismatch budget.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn difference_threshold(compared_pixels: u64) -> u64 {
    (compared_pixels as f64 * MISMATCH_BUDGET_PERCENT / 100.0).ceil() as u64
}

#[cfg(any(target_os = "macos", test))]
fn frame_png(base: &Path, index: usize, tick: i64) -> PathBuf {
    let stem = base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("opurom-animation");
    base.with_file_name(format!("{stem}-frame{index}-tick{tick}.png"))
}

#[cfg(target_os = "macos")]
mod host {
    use super::*;
    use crate::gpu_presentation::{present_animation_frame, FrameTimings, PresentationOutcome};
    use crate::{
        capture_window, compare_capture, read_ppm, render_animation_frame, software_renderer_path,
        window_availability, workspace_root, write_png, FrameView, MacHost, PixelComparison,
        SKIP_WINDOW_ENV,
    };
    use std::fs;

    struct Frame {
        tick: i64,
        timings: FrameTimings,
        comparison: PixelComparison,
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        png: PathBuf,
    }

    struct References {
        image: PpmImage,
        commands: String,
    }

    enum Verdict {
        Passed {
            frames: Vec<Frame>,
            min_interframe: u64,
            min_cross_tick: u64,
        },
        Failed(String),
        Skipped(String),
    }

    fn options(args: &[String]) -> (PathBuf, u32, u32, PathBuf) {
        let value = |name: &str| {
            args.iter()
                .position(|arg| arg == name)
                .and_then(|index| args.get(index + 1))
                .cloned()
        };
        let ir = value("--ir")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace_root().join(ANIMATION_IR));
        let width = value("--width").and_then(|v| v.parse().ok()).unwrap_or(200);
        let height = value("--height")
            .and_then(|v| v.parse().ok())
            .unwrap_or(150);
        let png = value("--out")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("opurom-gpu-animation-loop.png"));
        (ir, width, height, png)
    }

    fn render_references(
        renderer: &Path,
        ir: &Path,
        width: u32,
        height: u32,
    ) -> Result<Vec<References>, Verdict> {
        let suffix = format!("{}-{width}x{height}", std::process::id());
        let mut references = Vec::new();
        for (index, &tick) in ANIMATION_TICKS.iter().enumerate() {
            let ppm_path =
                std::env::temp_dir().join(format!("opurom-animation-ref-{suffix}-{index}.ppm"));
            let commands_path = std::env::temp_dir()
                .join(format!("opurom-animation-commands-{suffix}-{index}.json"));
            if let Err(error) =
                render_animation_frame(renderer, ir, width, height, tick, &ppm_path, &commands_path)
            {
                return Err(Verdict::Failed(format!(
                    "the software renderer failed at tick {tick}: {error}"
                )));
            }
            let bytes = match fs::read(&ppm_path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return Err(Verdict::Failed(format!(
                        "cannot read the reference PPM at tick {tick}: {error}"
                    )))
                }
            };
            let image = match read_ppm(&bytes) {
                Ok(image) => image,
                Err(error) => {
                    return Err(Verdict::Failed(format!(
                        "the reference PPM at tick {tick} is malformed: {error}"
                    )))
                }
            };
            if image.width != width || image.height != height {
                return Err(Verdict::Failed(format!(
                    "the reference PPM at tick {tick} is {}x{}, expected {width}x{height}",
                    image.width, image.height
                )));
            }
            let commands = match fs::read_to_string(&commands_path) {
                Ok(json) => json,
                Err(error) => {
                    return Err(Verdict::Failed(format!(
                        "cannot read the display list at tick {tick}: {error}"
                    )))
                }
            };
            references.push(References { image, commands });
        }
        Ok(references)
    }

    fn run(args: &[String]) -> Verdict {
        let (ir, width, height, png) = options(args);
        let Some(renderer) = software_renderer_path() else {
            return Verdict::Skipped(
                "the software renderer binary was not found; cannot produce the reference sequence"
                    .to_owned(),
            );
        };
        if !ir.is_file() {
            return Verdict::Skipped(format!(
                "the animated IR fixture was not found at {}",
                ir.display()
            ));
        }
        let references = match render_references(&renderer, &ir, width, height) {
            Ok(references) => references,
            Err(verdict) => return verdict,
        };

        let host = MacHost::new(width, height, &png);
        let window = match host.open_window() {
            Ok(window) => window,
            Err(error) => {
                return Verdict::Skipped(format!("AppKit could not open the live window: {error}"))
            }
        };

        let mut frames = Vec::new();
        for (index, reference) in references.iter().enumerate() {
            let tick = ANIMATION_TICKS[index];
            let (outcome, timings) =
                present_animation_frame(&reference.commands, window.metal_layer_ptr());
            match outcome {
                PresentationOutcome::Presented => {}
                PresentationOutcome::Skipped(reason) => return Verdict::Skipped(reason),
                PresentationOutcome::Failed(reason) => return Verdict::Failed(reason),
            }

            // The window is captured AFTER this frame's present, never once
            // for the whole loop; each capture sees the drawable just presented.
            window.settle();
            let captured = match capture_window(&window) {
                Ok(captured) => captured,
                Err(error) => {
                    return Verdict::Failed(format!("cacheDisplay capture failed: {error}"))
                }
            };
            let png_path = frame_png(&png, index, tick);
            if let Err(error) =
                write_png(captured.width, captured.height, &captured.pixels, &png_path)
            {
                return Verdict::Failed(format!("cannot write the captured PNG: {error}"));
            }
            let content_height = captured.height.saturating_sub(captured.title_bar_height);
            let content_row = captured.title_bar_height as usize * captured.width as usize * 4;
            let Some(content_pixels) = captured.pixels.get(content_row..) else {
                return Verdict::Failed(
                    "the captured window is shorter than its title bar".to_owned(),
                );
            };
            let content = FrameView {
                width: captured.width,
                height: content_height,
                pixels: content_pixels,
            };
            let comparison = match compare_capture(&content, &reference.image) {
                Ok(comparison) => comparison,
                Err(error) => {
                    return Verdict::Failed(format!("on-screen comparison failed: {error}"))
                }
            };
            frames.push(Frame {
                tick,
                timings,
                comparison,
                pixels: content_pixels.to_vec(),
                width: content.width,
                height: content.height,
                png: png_path,
            });
        }

        let compared_pixels: u64 = frames.iter().map(|f| f.comparison.compared_pixels).sum();
        let mismatched_pixels: u64 = frames.iter().map(|f| f.comparison.mismatched_pixels).sum();
        if compared_pixels == 0
            || frames.iter().any(|f| !f.comparison.is_pass())
            || mismatched_pixels as f64 * 100.0 / compared_pixels as f64 > MISMATCH_BUDGET_PERCENT
        {
            return Verdict::Failed(format!(
                "the captured sequence does not match the software renderer: mismatched_pixels={mismatched_pixels} compared_pixels={compared_pixels}"
            ));
        }

        let threshold = difference_threshold(frames[0].comparison.compared_pixels);
        let mut min_interframe = u64::MAX;
        for pair in frames.windows(2) {
            let delta = differing_pixels(
                &pair[0].pixels,
                &pair[1].pixels,
                pair[0].width,
                pair[0].height,
            );
            min_interframe = min_interframe.min(delta);
        }
        if min_interframe <= threshold {
            return Verdict::Failed(format!(
                "captured frames do not change between ticks: min_interframe_delta={min_interframe} threshold={threshold}"
            ));
        }

        let mut min_cross_tick = u64::MAX;
        for (index, frame) in frames.iter().enumerate() {
            for (other, reference) in references.iter().enumerate() {
                if index == other {
                    continue;
                }
                let delta =
                    compare_rgba_to_ppm(&frame.pixels, frame.width, frame.height, &reference.image)
                        .mismatched_pixels;
                min_cross_tick = min_cross_tick.min(delta);
            }
        }
        if min_cross_tick <= threshold {
            return Verdict::Failed(format!(
                "a captured frame also matches another tick's software reference: min_cross_tick_delta={min_cross_tick} threshold={threshold}"
            ));
        }

        Verdict::Passed {
            frames,
            min_interframe,
            min_cross_tick,
        }
    }

    /// Drive the bounded loop and print its measured verdict. Exit code 0 for
    /// PASS and SKIP, 1 for FAIL.
    pub fn verify(args: &[String]) -> i32 {
        if std::env::var_os(SKIP_WINDOW_ENV).is_some() {
            println!("SKIP {SKIP_WINDOW_ENV} is set");
            return 0;
        }
        if let Err(reason) = window_availability() {
            println!("SKIP {}", one_line(&reason.to_string()));
            return 0;
        }
        let png = options(args).3;
        match run(args) {
            Verdict::Skipped(reason) => {
                println!("SKIP {}", one_line(&reason));
                0
            }
            Verdict::Failed(reason) => {
                println!("FAIL {}", one_line(&reason));
                1
            }
            Verdict::Passed {
                frames,
                min_interframe,
                min_cross_tick,
            } => {
                let compared_pixels: u64 =
                    frames.iter().map(|f| f.comparison.compared_pixels).sum();
                let mismatched_pixels: u64 =
                    frames.iter().map(|f| f.comparison.mismatched_pixels).sum();
                for (index, frame) in frames.iter().enumerate() {
                    println!(
                        "frame={index} tick={} dispatch_ms={:.3} present_ms={:.3} mismatched_pixels={} compared_pixels={} mismatch={:.4}% max_channel_delta={} png={}",
                        frame.tick,
                        frame.timings.dispatch_ns as f64 / 1_000_000.0,
                        frame.timings.present_ns as f64 / 1_000_000.0,
                        frame.comparison.mismatched_pixels,
                        frame.comparison.compared_pixels,
                        frame.comparison.mismatch_percent(),
                        frame.comparison.max_channel_delta,
                        frame.png.display(),
                    );
                }
                let ticks = ANIMATION_TICKS
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                println!(
                    "PASS backend=metal-blit frames={} ticks={ticks} compared_pixels={compared_pixels} mismatched_pixels={mismatched_pixels} mismatch={:.4}% tolerance={} budget={:.1}% min_interframe_delta={min_interframe} min_cross_tick_delta={min_cross_tick} png={}",
                    frames.len(),
                    mismatched_pixels as f64 * 100.0 / compared_pixels as f64,
                    CHANNEL_TOLERANCE,
                    MISMATCH_BUDGET_PERCENT,
                    frame_png(&png, 0, ANIMATION_TICKS[0]).display(),
                );
                0
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub fn verify_gpu_animation_loop(args: &[String]) -> i32 {
    host::verify(args)
}

/// Non-macOS hosts cannot present a Metal drawable: a concrete SKIP, exit 0.
#[cfg(not(target_os = "macos"))]
pub fn verify_gpu_animation_loop(_args: &[String]) -> i32 {
    if std::env::var_os(crate::SKIP_WINDOW_ENV).is_some() {
        println!("SKIP {} is set", crate::SKIP_WINDOW_ENV);
    } else {
        println!("SKIP GPU animation loop is macOS/Metal only");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_bounded_and_strictly_increasing() {
        assert!(ticks_are_distinct(&ANIMATION_TICKS));
        assert_eq!(ANIMATION_TICKS.len(), 4);
        assert!(!ticks_are_distinct(&[]));
        assert!(!ticks_are_distinct(&[0, 0]));
        assert!(!ticks_are_distinct(&[96, 0]));
    }

    #[test]
    fn identical_captures_have_no_difference() {
        let pixels = vec![10u8, 20, 30, 255, 40, 50, 60, 255];
        assert_eq!(differing_pixels(&pixels, &pixels, 2, 1), 0);
    }

    #[test]
    fn different_captures_are_counted_beyond_the_tolerance() {
        let left = vec![0u8; 4 * 4];
        let right = vec![200u8; 4 * 4];
        assert_eq!(differing_pixels(&left, &right, 2, 2), 4);
        assert_eq!(difference_threshold(200), 4);
    }

    #[test]
    fn frame_png_names_are_distinct_per_tick() {
        let base = Path::new("/tmp/opurom-loop.png");
        let names: Vec<PathBuf> = ANIMATION_TICKS
            .iter()
            .enumerate()
            .map(|(index, &tick)| frame_png(base, index, tick))
            .collect();
        let unique: std::collections::BTreeSet<_> = names.iter().collect();
        assert_eq!(unique.len(), ANIMATION_TICKS.len());
    }
}
