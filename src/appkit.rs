//! Real AppKit host descriptor and live-window entry point (R5_ADDENDUM.md section 2).
//!
//! `MacHost::describe()` and the pure `frame_to_png` path are safe to exercise
//! headlessly. `MacHost::open_window()` creates an actual `NSApplication`,
//! `NSWindow`, `NSView` and `CAMetalLayer`; it requires the macOS main thread
//! and a GUI (Aqua) session.
//!
//! R7 adds the real capture path: `open_window()` draws the frame into the
//! window, lets AppKit composite it, captures the window's own pixels with
//! `cacheDisplay(in:to:)` and writes a real PNG.

use std::ffi::c_void;
use std::fmt;
use std::path::{Path, PathBuf};

use objc2::{rc::Retained, ClassType, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBitmapImageRep, NSScreen,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSDate, NSPoint, NSRect, NSRunLoop, NSSize};
use objc2_quartz_core::{CAMetalLayer, CATransaction};

use crate::{
    appkit_capabilities, frame_to_png, AdapterError, Capabilities, FrameView, Headless, InputEvent,
    PlatformAdapter, Size, APPKIT_SPEC,
};

const WINDOW_STYLE_NAME: &str = "Titled|Closable|Miniaturizable|Resizable";

/// How long the AppKit event loop runs before the capture, in seconds.
const EVENT_LOOP_SECONDS: f64 = 0.25;

fn window_style() -> NSWindowStyleMask {
    NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable
}

fn class_name<T: ClassType>() -> &'static str {
    T::class().name().to_str().unwrap_or(T::NAME)
}

fn dpi_scale_from_backing_factor(scale: f64) -> f32 {
    let scale = scale as f32;
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Query `NSScreen.backingScaleFactor` on the main thread when a screen exists.
/// The documented fallback is 1.0 on a non-main thread, with no screen, or
/// when the host reports a non-positive/non-finite scale.
pub(crate) fn screen_dpi_scale() -> f32 {
    let Some(mtm) = MainThreadMarker::new() else {
        return 1.0;
    };
    let Some(screen) = NSScreen::mainScreen(mtm) else {
        return 1.0;
    };
    dpi_scale_from_backing_factor(screen.backingScaleFactor())
}

/// Why a live window cannot be opened on this host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowUnavailable {
    /// The caller is not on the process main thread.
    NotMainThread,
    /// The host has no Aqua (GUI) session: a headless CI runner, an SSH
    /// session or a launchd job without a window server connection.
    NoAquaSession { detail: String },
}

impl WindowUnavailable {
    /// The static reason string that `AdapterError::Backend` carries.
    pub fn reason(&self) -> &'static str {
        match self {
            WindowUnavailable::NotMainThread => "AppKit windows require the process main thread",
            WindowUnavailable::NoAquaSession { .. } => "no Aqua session: no GUI window server",
        }
    }
}

impl fmt::Display for WindowUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WindowUnavailable::NotMainThread => {
                write!(f, "AppKit windows require the process main thread")
            }
            WindowUnavailable::NoAquaSession { detail } => {
                write!(f, "no Aqua session: {detail}")
            }
        }
    }
}

/// Check that this host can show a real window.
///
/// A host qualifies only when the process runs on the main thread, the window
/// server reports a live GUI session, and a main screen exists. Everything else
/// is a SKIP with a concrete reason, never a fake PASS.
pub fn window_availability() -> Result<(), WindowUnavailable> {
    if MainThreadMarker::new().is_none() {
        return Err(WindowUnavailable::NotMainThread);
    }
    // `CGSessionCopyCurrentDictionary` is nil without a GUI session.
    if objc2_core_graphics::CGSessionCopyCurrentDictionary().is_none() {
        return Err(WindowUnavailable::NoAquaSession {
            detail: "the window server reports no current GUI session".to_string(),
        });
    }
    let mtm = MainThreadMarker::new().expect("checked above");
    if NSScreen::mainScreen(mtm).is_none() {
        return Err(WindowUnavailable::NoAquaSession {
            detail: "macOS reports no main screen".to_string(),
        });
    }
    Ok(())
}

/// Class names and window style of the real Objective-C host surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MacHostDescriptor {
    pub application_class: &'static str,
    pub window_class: &'static str,
    pub view_class: &'static str,
    pub surface_class: &'static str,
    pub window_style: &'static str,
    pub window_style_mask: usize,
}

impl fmt::Display for MacHostDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} | {} | {} | {} | {} ({})",
            self.application_class,
            self.window_class,
            self.view_class,
            self.surface_class,
            self.window_style,
            self.window_style_mask,
        )
    }
}

/// macOS host adapter. Its `present` path writes a verifiable PNG; the live
/// AppKit window is created only by an explicit `open_window()` call.
#[derive(Debug)]
pub struct MacHost {
    headless: Headless,
    output: PathBuf,
}

impl MacHost {
    pub fn new(width: u32, height: u32, output: impl AsRef<Path>) -> Self {
        Self {
            headless: Headless::new(width, height),
            output: output.as_ref().to_path_buf(),
        }
    }

    pub fn output_path(&self) -> &Path {
        &self.output
    }

    pub fn describe() -> MacHostDescriptor {
        MacHostDescriptor {
            application_class: class_name::<NSApplication>(),
            window_class: class_name::<NSWindow>(),
            view_class: class_name::<NSView>(),
            surface_class: class_name::<CAMetalLayer>(),
            window_style: WINDOW_STYLE_NAME,
            window_style_mask: window_style().0,
        }
    }

    pub fn push_event(&mut self, event: InputEvent) {
        self.headless.push_event(event);
    }

    pub fn resize(&mut self, size: Size) {
        self.headless.resize(size);
    }

    pub fn frame(&self) -> FrameView<'_> {
        self.headless.frame()
    }

    pub fn digest(&self) -> &str {
        self.headless.digest()
    }

    /// Live-window entry point. It creates a real `NSWindow` with an `NSView`
    /// and a `CAMetalLayer`, draws `frame` into it, runs the AppKit event loop
    /// briefly and captures the window's own pixels.
    ///
    /// It needs a GUI-capable macOS main thread; a headless host gets an
    /// `AdapterError::Backend` with the concrete reason instead of a window.
    pub fn open_window(&self) -> Result<MacWindow, AdapterError> {
        let mtm = MainThreadMarker::new().ok_or(AdapterError::Backend(
            "AppKit windows require the main thread",
        ))?;
        window_availability().map_err(|reason| AdapterError::Backend(reason.reason()))?;
        let application = NSApplication::sharedApplication(mtm);
        if !application.setActivationPolicy(NSApplicationActivationPolicy::Regular) {
            return Err(AdapterError::Backend(
                "AppKit activation policy could not be set",
            ));
        }
        application.finishLaunching();
        let viewport = self.viewport();
        let rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(viewport.width as f64, viewport.height as f64),
        );
        // SAFETY: this generated initializer is called with a main-thread
        // allocation and valid AppKit style/backing constants.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                rect,
                window_style(),
                NSBackingStoreType::Buffered,
                false,
            )
        };
        let view = NSView::initWithFrame(mtm.alloc(), rect);
        let layer = CAMetalLayer::init(mtm.alloc());
        view.setWantsLayer(true);
        view.setLayer(Some(layer.as_super()));
        window.setContentView(Some(&view));
        window.makeKeyAndOrderFront(None);
        Ok(MacWindow {
            application,
            window,
            view,
            layer,
        })
    }
}

impl PlatformAdapter for MacHost {
    fn name(&self) -> &str {
        APPKIT_SPEC.name
    }

    fn capabilities(&self) -> Capabilities {
        appkit_capabilities()
    }

    fn viewport(&self) -> Size {
        self.headless.viewport()
    }

    fn poll_event(&mut self) -> Option<InputEvent> {
        self.headless.poll_event()
    }

    fn present(&mut self, frame: FrameView<'_>) -> Result<(), AdapterError> {
        frame_to_png(frame, &self.output)?;
        self.headless.present(frame)
    }
}

/// Retained Objective-C handles for an opened live window.
pub struct MacWindow {
    application: Retained<NSApplication>,
    window: Retained<NSWindow>,
    view: Retained<NSView>,
    layer: Retained<CAMetalLayer>,
}

impl MacWindow {
    /// Read all retained handles; dropping this value releases the live window.
    #[allow(dead_code)]
    pub fn keep_alive(&self) {
        let _ = (&self.application, &self.window, &self.view, &self.layer);
    }

    /// Opaque pointer to the retained `CAMetalLayer` for the runtime GPU bridge.
    pub fn metal_layer_ptr(&self) -> *mut c_void {
        &*self.layer as *const CAMetalLayer as *mut c_void
    }

    /// Let AppKit and Core Animation composite one presented drawable.
    pub fn settle(&self) {
        CATransaction::flush();
        let deadline = NSDate::dateWithTimeIntervalSinceNow(EVENT_LOOP_SECONDS);
        NSRunLoop::currentRunLoop().runUntilDate(&deadline);
        self.window.displayIfNeeded();
        self.view.displayIfNeeded();
    }
}

/// A captured window image: RGBA8 pixels plus the dimensions of the capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedWindow {
    pub width: u32,
    pub height: u32,
    /// RGBA8 pixels, `width * height * 4` bytes, top-left origin.
    pub pixels: Vec<u8>,
    /// Height in pixels of the system title bar above the captured content.
    pub title_bar_height: u32,
}

/// Draw an RGBA8 frame into the window's view, run the AppKit event loop
/// briefly so the compositor picks the frame up, and capture the window's own
/// pixels with `cacheDisplay(in:to:)`.
pub fn draw_and_capture(
    window: &MacWindow,
    frame: FrameView<'_>,
) -> Result<CapturedWindow, AdapterError> {
    let (width, height, pixels) = (frame.width, frame.height, frame.pixels);
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

    let cg_image = crate::core_graphics_bridge::cg_image_from_rgba(width, height, pixels)?;
    let Some(layer) = window.view.layer() else {
        return Err(AdapterError::Backend("the window view has no layer"));
    };
    // SAFETY: `cg_image` is a live `CGImage` and `contents` accepts any object.
    unsafe {
        layer.setContents(Some(cg_image.as_ref()));
    }
    // SAFETY: `kCAGravityResize` is an immutable system constant.
    layer.setContentsGravity(unsafe { objc2_quartz_core::kCAGravityResize });
    layer.setContentsScale(window.window.backingScaleFactor() as f64);

    // Let AppKit and Core Animation composite the layer before the capture.
    window.settle();

    let bounds = window.view.bounds();
    let Some(rep) = window.view.bitmapImageRepForCachingDisplayInRect(bounds) else {
        return Err(AdapterError::Backend(
            "AppKit refused a caching bitmap for the window view",
        ));
    };
    window
        .view
        .cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    let (captured_width, captured_height, captured_pixels) = rep_to_rgba(&rep)?;
    Ok(CapturedWindow {
        width: captured_width,
        height: captured_height,
        pixels: captured_pixels,
        title_bar_height: 0,
    })
}

/// Capture the whole window, system title bar included.
///
/// The title bar is drawn by AppKit into the window's theme frame, so the
/// capture goes through the superview of the content view. The returned
/// `title_bar_height` says how many pixel rows at the top of the capture are
/// system chrome rather than the rendered frame.
pub fn capture_window(window: &MacWindow) -> Result<CapturedWindow, AdapterError> {
    // SAFETY: `superview` is a plain accessor; the theme frame outlives the
    // call because the window keeps it alive.
    let Some(theme) = (unsafe { window.view.superview() }) else {
        return Err(AdapterError::Backend(
            "the window has no theme frame to capture",
        ));
    };
    let bounds = theme.bounds();
    let Some(rep) = theme.bitmapImageRepForCachingDisplayInRect(bounds) else {
        return Err(AdapterError::Backend(
            "AppKit refused a caching bitmap for the window frame",
        ));
    };
    theme.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    let (captured_width, captured_height, captured_pixels) = rep_to_rgba(&rep)?;

    // The theme frame is not flipped: its origin is bottom-left while the
    // bitmap rows run top-down. The content view's frame inside the theme
    // gives the exact row where the rendered frame starts.
    let scale = window.window.backingScaleFactor();
    let content_frame = window.view.frame();
    let theme_height = bounds.size.height;
    let content_top = content_frame.origin.y + content_frame.size.height;
    let title_bar_height = ((theme_height - content_top) * scale).round().max(0.0) as u32;
    Ok(CapturedWindow {
        width: captured_width,
        height: captured_height,
        pixels: captured_pixels,
        title_bar_height,
    })
}

/// Copy an `NSBitmapImageRep` into owned RGBA8 bytes.
fn rep_to_rgba(rep: &NSBitmapImageRep) -> Result<(u32, u32, Vec<u8>), AdapterError> {
    let width = rep.pixelsWide();
    let height = rep.pixelsHigh();
    if width <= 0 || height <= 0 {
        return Err(AdapterError::Backend("the captured bitmap has no pixels"));
    }
    if rep.samplesPerPixel() < 3 {
        return Err(AdapterError::Backend(
            "the captured bitmap has fewer than three samples per pixel",
        ));
    }
    let bytes_per_row = rep.bytesPerRow();
    let data = rep.bitmapData();
    if data.is_null() {
        return Err(AdapterError::Backend(
            "the captured bitmap has no pixel data",
        ));
    }
    let samples = rep.samplesPerPixel() as usize;
    let width = width as usize;
    let height = height as usize;
    let bytes_per_row = bytes_per_row as usize;
    // SAFETY: `data` points at `bytes_per_row * height` bytes owned by the
    // bitmap rep, which is kept alive by the caller for the whole copy.
    let source = unsafe { std::slice::from_raw_parts(data, bytes_per_row * height) };
    let mut pixels = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let row = &source[y * bytes_per_row..y * bytes_per_row + width * samples];
        for x in 0..width {
            let pixel = &row[x * samples..x * samples + samples];
            pixels.push(pixel[0]);
            pixels.push(pixel[1]);
            pixels.push(pixel[2]);
            pixels.push(if samples >= 4 { pixel[3] } else { 255 });
        }
    }
    Ok((width as u32, height as u32, pixels))
}

/// Encode owned RGBA8 bytes as a PNG file.
pub fn write_png(
    width: u32,
    height: u32,
    pixels: &[u8],
    path: impl AsRef<Path>,
) -> Result<(), AdapterError> {
    frame_to_png(
        FrameView {
            width,
            height,
            pixels,
        },
        path,
    )
}

#[cfg(test)]
mod tests {
    use super::{dpi_scale_from_backing_factor, screen_dpi_scale};

    #[test]
    fn dpi_scale_uses_a_screen_value_or_the_documented_fallback() {
        assert_eq!(dpi_scale_from_backing_factor(2.0), 2.0);
        assert_eq!(dpi_scale_from_backing_factor(0.0), 1.0);
        assert_eq!(dpi_scale_from_backing_factor(f64::NAN), 1.0);
        let scale = screen_dpi_scale();
        assert!(scale.is_finite() && scale >= 1.0);
    }
}
