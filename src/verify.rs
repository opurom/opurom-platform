//! The R7 on-screen verification: open a real window, capture its pixels and
//! compare them against the software renderer's PPM for the same frame.
//!
//! The result is a value, never a fake PASS: a host without an Aqua session
//! reports `Skipped` with the concrete reason, and a pixel mismatch reports
//! `Failed` with the measured numbers.

use std::path::{Path, PathBuf};

use crate::pixel::{compare_rgba_to_ppm, PixelComparison};
use crate::{AdapterError, FrameView, PpmImage};

/// Outcome of the on-screen verification.
#[derive(Clone, Debug, PartialEq)]
pub enum Verification {
    /// The captured window pixels match the software renderer within the
    /// documented tolerance.
    Passed {
        captured: (u32, u32),
        comparison: PixelComparison,
        png: PathBuf,
    },
    /// The captured window pixels differ from the software renderer beyond the
    /// documented tolerance.
    Failed {
        captured: (u32, u32),
        comparison: PixelComparison,
        png: PathBuf,
    },
    /// No window could be opened on this host; `reason` says why.
    Skipped { reason: String },
}

impl Verification {
    /// The reported verdict string: `PASS`, `FAIL` or `SKIP`.
    pub fn verdict(&self) -> &'static str {
        match self {
            Verification::Passed { .. } => "PASS",
            Verification::Failed { .. } => "FAIL",
            Verification::Skipped { .. } => "SKIP",
        }
    }

    /// A one-line human readable report.
    pub fn report(&self) -> String {
        match self {
            Verification::Passed {
                captured,
                comparison,
                png,
            } => format!(
                "PASS window={}x{} mismatched_pixels={} compared_pixels={} mismatch={:.4}% max_channel_delta={} tolerance={} png={}",
                captured.0,
                captured.1,
                comparison.mismatched_pixels,
                comparison.compared_pixels,
                comparison.mismatch_percent(),
                comparison.max_channel_delta,
                crate::CHANNEL_TOLERANCE,
                png.display(),
            ),
            Verification::Failed {
                captured,
                comparison,
                png,
            } => format!(
                "FAIL window={}x{} mismatched_pixels={} compared_pixels={} mismatch={:.4}% max_channel_delta={} tolerance={} png={}",
                captured.0,
                captured.1,
                comparison.mismatched_pixels,
                comparison.compared_pixels,
                comparison.mismatch_percent(),
                comparison.max_channel_delta,
                crate::CHANNEL_TOLERANCE,
                png.display(),
            ),
            Verification::Skipped { reason } => format!("SKIP {reason}"),
        }
    }
}

/// Locate the sibling software renderer binary, honouring `OPUROM_RENDER_BIN`.
pub fn software_renderer_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var(crate::RENDERER_BIN_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
        return None;
    }
    let path = PathBuf::from("../opurom-render/target/debug/opurom-render");
    path.is_file().then_some(path)
}

/// Render the reference PPM for `ir` with the software renderer.
pub fn render_reference_ppm(
    renderer: &Path,
    ir: &Path,
    width: u32,
    height: u32,
    out: &Path,
) -> Result<(), AdapterError> {
    let output = std::process::Command::new(renderer)
        .arg(ir)
        .args([width.to_string(), height.to_string()])
        .arg(out)
        .output()
        .map_err(|_| AdapterError::Backend("the software renderer could not be started"))?;
    if !output.status.success() {
        return Err(AdapterError::Backend(
            "the software renderer failed to render the reference frame",
        ));
    }
    Ok(())
}

/// Compare a captured window against a software-renderer PPM.
///
/// Both images must have the same size; a different size is a FAIL, not a
/// silent partial comparison.
pub fn compare_capture(
    captured: &FrameView<'_>,
    reference: &PpmImage,
) -> Result<PixelComparison, AdapterError> {
    if captured.width != reference.width || captured.height != reference.height {
        return Err(AdapterError::Backend(
            "the captured window size differs from the reference frame size",
        ));
    }
    Ok(compare_rgba_to_ppm(
        captured.pixels,
        captured.width,
        captured.height,
        reference,
    ))
}

#[cfg(test)]
mod tests {
    use super::{compare_capture, Verification};
    use crate::pixel::PixelComparison;
    use crate::{AdapterError, FrameView, PpmImage};

    fn reference() -> PpmImage {
        PpmImage {
            width: 2,
            height: 1,
            pixels: vec![10, 20, 30, 40, 50, 60],
        }
    }

    #[test]
    fn a_size_mismatch_is_an_error_not_a_partial_comparison() {
        let captured = FrameView {
            width: 4,
            height: 4,
            pixels: &[0u8; 64],
        };
        assert!(matches!(
            compare_capture(&captured, &reference()),
            Err(AdapterError::Backend(_))
        ));
    }

    #[test]
    fn the_verdict_and_report_state_the_outcome() {
        let comparison = PixelComparison {
            mismatched_pixels: 0,
            compared_pixels: 10,
            max_channel_delta: 28,
        };
        let passed = Verification::Passed {
            captured: (100, 50),
            comparison,
            png: "/tmp/window.png".into(),
        };
        assert_eq!(passed.verdict(), "PASS");
        assert!(passed.report().starts_with("PASS window=100x50"));

        let skipped = Verification::Skipped {
            reason: "no Aqua session".into(),
        };
        assert_eq!(skipped.verdict(), "SKIP");
        assert_eq!(skipped.report(), "SKIP no Aqua session");
    }
}
