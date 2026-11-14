//! Bridge between the adapter's RGBA8 frames and CoreGraphics `CGImage`.
//!
//! `CGImage` is a CoreFoundation type, not an Objective-C one, so it cannot
//! be created through `objc2`'s message send. This module uses the
//! `objc2-core-graphics` bindings for the real `CGImageCreate` call and hands
//! the result to AppKit as a `CALayer` content object.

use objc2_core_foundation::CFData;
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo,
};

use crate::AdapterError;

/// Wrap owned RGBA8 bytes in a real `CGImage` (8 bits per component, alpha
/// last, device RGB).
pub fn cg_image_from_rgba(
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<objc2_core_foundation::CFRetained<CGImage>, AdapterError> {
    if width == 0 || height == 0 {
        return Err(AdapterError::InvalidFrame);
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|count| count.checked_mul(4))
        .ok_or(AdapterError::InvalidFrame)?;
    if pixels.len() != expected {
        return Err(AdapterError::InvalidFrame);
    }
    let color_space = CGColorSpace::new_device_rgb().ok_or(AdapterError::Backend(
        "CoreGraphics refused the device RGB colour space",
    ))?;
    let data = CFData::from_bytes(pixels);
    let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or(AdapterError::Backend(
        "CoreGraphics refused the frame data provider",
    ))?;
    let bitmap_info = CGBitmapInfo(CGImageByteOrderInfo::Order32Big.0 | CGImageAlphaInfo::Last.0);
    // SAFETY: the provider owns a copy of `data` for the lifetime of the
    // image, the colour space is a live `CGColorSpace` and the geometry
    // matches the provider's byte count.
    let image = unsafe {
        CGImage::new(
            width as usize,
            height as usize,
            8,
            32,
            (width as usize) * 4,
            Some(&color_space),
            bitmap_info,
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .ok_or(AdapterError::Backend(
        "CoreGraphics refused to create the frame image",
    ))?;
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::cg_image_from_rgba;
    use crate::AdapterError;
    use objc2_core_graphics::CGImage;

    #[test]
    fn a_real_image_is_created_for_a_valid_frame() {
        let pixels = [255u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 1, 2, 3, 4];
        let image = cg_image_from_rgba(2, 2, &pixels).unwrap();
        assert_eq!(CGImage::width(Some(&image)), 2);
        assert_eq!(CGImage::height(Some(&image)), 2);
        assert_eq!(CGImage::bits_per_pixel(Some(&image)), 32);
    }

    #[test]
    fn an_invalid_frame_is_rejected_before_core_graphics_is_called() {
        assert_eq!(
            cg_image_from_rgba(2, 2, &[0u8; 15]).err(),
            Some(AdapterError::InvalidFrame)
        );
        assert_eq!(
            cg_image_from_rgba(0, 0, &[]).err(),
            Some(AdapterError::InvalidFrame)
        );
    }
}
