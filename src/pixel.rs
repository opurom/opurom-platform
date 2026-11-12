//! Tolerant pixel comparison for the R7 on-screen verification.
//!
//! The window capture and the software renderer never agree bit-for-bit: the
//! capture goes through the display colour profile, the compositor may blend
//! edges and the captured region can include system chrome. The comparison
//! therefore uses a documented per-channel tolerance plus a mismatch budget.

use crate::ppm::PpmImage;

/// Maximum accepted absolute difference per channel (0-255) between a captured
/// pixel and the software-renderer reference pixel.
///
/// Documented tolerance: 32/255 (12.5%) per channel. The observed shift on a
/// Display P3 screen is at most 28 (a blue accent turns from 79,70,229 into
/// 71,58,201), so 32 covers the colour-profile conversion plus a small
/// compositing margin while still rejecting a genuinely wrong frame.
pub const CHANNEL_TOLERANCE: u32 = 32;

/// Maximum accepted fraction of mismatching pixels, in percent.
///
/// Documented tolerance: 2% of the compared region. Only the content region
/// of the captured window is compared (the system title bar is excluded), so
/// this budget covers edge antialiasing and rounding without hiding a wrong
/// frame, which would mismatch in nearly every pixel.
pub const MISMATCH_BUDGET_PERCENT: f64 = 2.0;

/// Result of comparing a captured region against a software-renderer frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelComparison {
    /// Number of pixels whose worst channel difference exceeds the tolerance.
    pub mismatched_pixels: u64,
    /// Total number of compared pixels.
    pub compared_pixels: u64,
    /// Largest absolute per-channel difference observed.
    pub max_channel_delta: u32,
}

impl PixelComparison {
    /// Mismatched pixels as a percentage of the compared region.
    pub fn mismatch_percent(&self) -> f64 {
        if self.compared_pixels == 0 {
            return 0.0;
        }
        (self.mismatched_pixels as f64) * 100.0 / (self.compared_pixels as f64)
    }

    /// PASS when the mismatch percentage stays inside the documented budget.
    pub fn is_pass(&self) -> bool {
        self.mismatch_percent() <= MISMATCH_BUDGET_PERCENT
    }
}

/// Compare an RGBA8 captured region against a reference PPM of the same size.
///
/// Both images are compared top-left first, channel by channel; the alpha
/// channel of the capture is ignored because the window is opaque.
pub fn compare_rgba_to_ppm(
    captured: &[u8],
    captured_width: u32,
    captured_height: u32,
    reference: &PpmImage,
) -> PixelComparison {
    let mut comparison = PixelComparison {
        mismatched_pixels: 0,
        compared_pixels: 0,
        max_channel_delta: 0,
    };
    let width = captured_width.min(reference.width) as usize;
    let height = captured_height.min(reference.height) as usize;
    for y in 0..height {
        for x in 0..width {
            let captured_pixel = (y * captured_width as usize + x) * 4;
            let reference_pixel = (y * reference.width as usize + x) * 3;
            let mut worst = 0u32;
            for channel in 0..3 {
                let got = captured[captured_pixel + channel] as i32;
                let want = reference.pixels[reference_pixel + channel] as i32;
                let delta = (got - want).unsigned_abs();
                if delta > worst {
                    worst = delta;
                }
            }
            if worst > comparison.max_channel_delta {
                comparison.max_channel_delta = worst;
            }
            if worst > CHANNEL_TOLERANCE {
                comparison.mismatched_pixels += 1;
            }
            comparison.compared_pixels += 1;
        }
    }
    comparison
}

#[cfg(test)]
mod tests {
    use super::{compare_rgba_to_ppm, PixelComparison, CHANNEL_TOLERANCE, MISMATCH_BUDGET_PERCENT};
    use crate::ppm::PpmImage;

    fn reference(pixels: Vec<u8>, width: u32, height: u32) -> PpmImage {
        PpmImage {
            width,
            height,
            pixels,
        }
    }

    #[test]
    fn identical_frames_match_exactly() {
        let reference = reference(vec![10, 20, 30, 40, 50, 60], 2, 1);
        let captured = [10u8, 20, 30, 255, 40, 50, 60, 255];
        let comparison = compare_rgba_to_ppm(&captured, 2, 1, &reference);
        assert_eq!(comparison.mismatched_pixels, 0);
        assert_eq!(comparison.compared_pixels, 2);
        assert_eq!(comparison.max_channel_delta, 0);
        assert!(comparison.is_pass());
    }

    #[test]
    fn a_shift_inside_the_tolerance_still_passes() {
        let reference = reference(vec![79, 70, 229], 1, 1);
        let captured = [71u8, 58, 201, 255];
        let comparison = compare_rgba_to_ppm(&captured, 1, 1, &reference);
        assert_eq!(comparison.mismatched_pixels, 0);
        assert_eq!(comparison.max_channel_delta, 28);
        assert!(comparison.is_pass());
    }

    #[test]
    fn a_wrong_frame_fails_the_budget() {
        let reference = reference(vec![79, 70, 229], 1, 1);
        let captured = [0u8, 0, 0, 255];
        let comparison = compare_rgba_to_ppm(&captured, 1, 1, &reference);
        assert_eq!(comparison.mismatched_pixels, 1);
        assert_eq!(comparison.compared_pixels, 1);
        assert_eq!(comparison.max_channel_delta, 229);
        assert!(!comparison.is_pass());
        assert!(comparison.mismatch_percent() > MISMATCH_BUDGET_PERCENT);
    }

    #[test]
    fn the_tolerance_is_documented_and_bounded() {
        assert_eq!(CHANNEL_TOLERANCE, 32);
        assert_eq!(MISMATCH_BUDGET_PERCENT, 2.0);
        let empty = PixelComparison {
            mismatched_pixels: 0,
            compared_pixels: 0,
            max_channel_delta: 0,
        };
        assert_eq!(empty.mismatch_percent(), 0.0);
        assert!(empty.is_pass());
    }
}
