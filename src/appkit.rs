//! Real AppKit host descriptor and live-window entry point (R5_ADDENDUM.md section 2).
//!
//! `MacHost::describe()` and the pure `frame_to_png` path are safe to exercise
//! headlessly. `MacHost::open_window()` creates an actual `NSApplication`,
//! `NSWindow`, `NSView` and `CAMetalLayer`; it is NEVER called by tests or CI,
//! which have no GUI session. It requires the macOS main thread.

use std::fmt;
use std::path::{Path, PathBuf};

use objc2::{rc::Retained, ClassType, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSScreen, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::CAMetalLayer;

use crate::{
    appkit_capabilities, frame_to_png, AdapterError, Capabilities, FrameView, Headless, InputEvent,
    PlatformAdapter, Size, APPKIT_SPEC,
};

const WINDOW_STYLE_NAME: &str = "Titled|Closable|Miniaturizable|Resizable";

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

    /// Live-window entry point. NEVER call this in tests or CI: it creates a
    /// real AppKit window, needs a GUI-capable macOS main thread, and does not
    /// run an event loop. The handles in `MacWindow` keep the window alive.
    pub fn open_window(&self) -> Result<MacWindow, AdapterError> {
        let mtm = MainThreadMarker::new().ok_or(AdapterError::Backend(
            "AppKit windows require the main thread",
        ))?;
        let application = NSApplication::sharedApplication(mtm);
        if !application.setActivationPolicy(NSApplicationActivationPolicy::Regular) {
            return Err(AdapterError::Backend(
                "AppKit activation policy could not be set",
            ));
        }
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
    pub fn keep_alive(&self) {
        let _ = (&self.application, &self.window, &self.view, &self.layer);
    }
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
