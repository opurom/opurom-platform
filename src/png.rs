//! Headless RGBA8-to-PNG conversion for the R5 macOS host.
//!
//! On macOS the bytes are wrapped in a real `CGImage` before the cached `png`
//! crate encodes them. This path needs CoreGraphics but no GUI session or
//! `NSApplication`. Other targets return the documented `AppKitUnavailable`
//! error instead of claiming a native host.

use std::path::Path;

use crate::{AdapterError, FrameView};

#[cfg(target_os = "macos")]
use std::{fs::File, sync::Arc};

#[cfg(target_os = "macos")]
use core_graphics::{
    base::{kCGBitmapByteOrder32Big, kCGImageAlphaLast, kCGRenderingIntentDefault},
    color_space::CGColorSpace,
    data_provider::CGDataProvider,
    image::CGImage,
};

/// Write a borrowed RGBA8 frame as a valid PNG file.
///
/// macOS builds create a `CGImage` and encode its data with the `png` crate.
/// Non-macOS builds return `AdapterError::Backend(APPKIT_UNAVAILABLE)`; there is
/// no fake CoreGraphics call and no std-only PNG writer in this crate.
#[cfg(target_os = "macos")]
pub fn frame_to_png(frame: FrameView<'_>, path: impl AsRef<Path>) -> Result<(), AdapterError> {
    let row_bytes = validate_frame(frame)?;
    let pixels = Arc::new(frame.pixels.to_vec());
    let provider = CGDataProvider::from_buffer(pixels);
    let color_space = CGColorSpace::create_device_rgb();
    let bitmap_info = kCGImageAlphaLast | kCGBitmapByteOrder32Big;
    let image = CGImage::new(
        frame.width as usize,
        frame.height as usize,
        8,
        32,
        row_bytes,
        &color_space,
        bitmap_info,
        &provider,
        false,
        kCGRenderingIntentDefault,
    );
    if image.width() != frame.width as usize
        || image.height() != frame.height as usize
        || image.bits_per_pixel() != 32
    {
        return Err(AdapterError::Backend("CGImage rejected the RGBA8 frame"));
    }

    let data = image.data();
    let bytes = data.bytes();
    if bytes.len() != frame.pixels.len() {
        return Err(AdapterError::Backend("CGImage returned truncated pixels"));
    }
    let file = File::create(path).map_err(|_| AdapterError::Backend("PNG file creation failed"))?;
    let mut encoder = png::Encoder::new(file, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|_| AdapterError::Backend("PNG header write failed"))?;
    writer
        .write_image_data(bytes)
        .map_err(|_| AdapterError::Backend("PNG pixel write failed"))?;
    Ok(())
}

/// Documented non-macOS fallback: no AppKit/CoreGraphics call is faked.
#[cfg(not(target_os = "macos"))]
pub fn frame_to_png(_frame: FrameView<'_>, _path: impl AsRef<Path>) -> Result<(), AdapterError> {
    Err(AdapterError::Backend(crate::APPKIT_UNAVAILABLE))
}

#[cfg(target_os = "macos")]
fn validate_frame(frame: FrameView<'_>) -> Result<usize, AdapterError> {
    if frame.width == 0 || frame.height == 0 {
        return Err(AdapterError::InvalidFrame);
    }
    let row_bytes = (frame.width as usize)
        .checked_mul(4)
        .ok_or(AdapterError::InvalidFrame)?;
    let expected = row_bytes
        .checked_mul(frame.height as usize)
        .ok_or(AdapterError::InvalidFrame)?;
    if frame.pixels.len() != expected {
        return Err(AdapterError::InvalidFrame);
    }
    Ok(row_bytes)
}
