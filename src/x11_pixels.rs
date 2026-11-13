//! Pure pixel maths for the X11 adapter (BROWSER_AND_NATIVE_SURFACES.md section 3).
//!
//! Kept outside the Linux-gated `x11` module so the arithmetic is verified by
//! `cargo test` on every host, including CI runners and macOS workstations.

use crate::AdapterError;

pub fn checked_len(width: u32, height: u32) -> Result<usize, AdapterError> {
    if width == 0
        || height == 0
        || width > i32::MAX as u32
        || height > i32::MAX as u32
        || width > (i32::MAX as u32) / 4
    {
        return Err(AdapterError::InvalidFrame);
    }
    (width as usize)
        .checked_mul(4)
        .and_then(|row| row.checked_mul(height as usize))
        .ok_or(AdapterError::InvalidFrame)
}

/// Pack RGBA8 through the 32-bit TrueColor masks in XImage native byte order.
/// Bits outside the RGB masks come from the visual's alpha mask.
pub fn rgba_to_xpixel(pixels: &[u8], masks: (u32, u32, u32, u32)) -> Result<Vec<u8>, AdapterError> {
    let (red, green, blue, alpha) = masks;
    if red == 0 || green == 0 || blue == 0 {
        return Err(AdapterError::Backend("X11 TrueColor masks are invalid"));
    }
    let mut packed = Vec::with_capacity(pixels.len());
    for rgba in pixels.chunks_exact(4) {
        let value = ((rgba[0] as u32) << red.trailing_zeros()) & red
            | ((rgba[1] as u32) << green.trailing_zeros()) & green
            | ((rgba[2] as u32) << blue.trailing_zeros()) & blue
            | alpha;
        packed.extend_from_slice(&value.to_ne_bytes());
    }
    Ok(packed)
}

#[cfg(test)]
mod tests {
    use super::{checked_len, rgba_to_xpixel};
    use crate::AdapterError;

    #[test]
    fn rgba_uses_the_visual_masks_in_native_byte_order() {
        // The X11 TrueColor visual supplies its own channel masks; the packed
        // pixel follows those masks rather than a hard-coded RGBA/BGRA order.
        let rgb = (0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0xff00_0000);
        let packed = rgba_to_xpixel(&[10, 20, 30, 255], rgb).unwrap();
        assert_eq!(&packed[..4], &0xff0a141eu32.to_ne_bytes());

        let bgr = (0x0000_00ff, 0x0000_ff00, 0x00ff_0000, 0xff00_0000);
        let swapped = rgba_to_xpixel(&[10, 20, 30, 255], bgr).unwrap();
        assert_eq!(&swapped[..4], &0xff1e140au32.to_ne_bytes());
        assert_ne!(
            packed, swapped,
            "the visual masks must change the packed pixel"
        );
        assert!(rgba_to_xpixel(&[10, 20, 30, 255], (0, 0, 0, 0)).is_err());
    }

    #[test]
    fn frame_lengths_are_checked_before_any_xlib_call() {
        assert_eq!(checked_len(2, 2), Ok(16));
        assert_eq!(checked_len(0, 1), Err(AdapterError::InvalidFrame));
        assert_eq!(
            checked_len(u32::MAX, u32::MAX),
            Err(AdapterError::InvalidFrame)
        );
    }
}
