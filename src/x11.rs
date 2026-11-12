//! X11 window host (R8_ADDENDUM.md section 3).
//!
//! The Xlib calls are raw `extern "C"` declarations. This host is only
//! compile-checked: the R8 macOS host has no X server or X11 development
//! libraries, so no X11 window is opened here. A 24/32-bit TrueColor visual
//! is required; RGBA8 is packed through the visual's masks into an `XImage`
//! and presented with `XPutImage`.

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::ptr::{null, null_mut};

use crate::{AdapterError, Capabilities, FrameView, InputEvent, Lifecycle, PlatformAdapter, Size};

type Window = c_ulong;
type Drawable = c_ulong;

#[repr(C)]
struct Display {
    _private: [u8; 0],
}

#[repr(C)]
struct XGCValues {
    _private: [u8; 0],
}

type GC = *mut XGCValues;

#[repr(C)]
#[allow(dead_code)]
struct Visual {
    ext_data: *mut c_void,
    visualid: c_ulong,
    class: c_int,
    red_mask: c_ulong,
    green_mask: c_ulong,
    blue_mask: c_ulong,
    bits_per_rgb: c_int,
    map_entries: c_int,
}

type XImageCreateFn = unsafe extern "C" fn(
    *mut Display,
    *mut Visual,
    c_uint,
    c_int,
    c_int,
    *mut c_char,
    c_uint,
    c_uint,
    c_int,
    c_int,
) -> *mut XImage;
type XImageDestroyFn = unsafe extern "C" fn(*mut XImage) -> c_int;
type XImageGetPixelFn = unsafe extern "C" fn(*mut XImage, c_int, c_int) -> c_ulong;
type XImagePutPixelFn = unsafe extern "C" fn(*mut XImage, c_int, c_int, c_ulong) -> c_int;
type XImageSubImageFn =
    unsafe extern "C" fn(*mut XImage, c_int, c_int, c_uint, c_uint) -> *mut XImage;
type XImageAddPixelFn = unsafe extern "C" fn(*mut XImage, c_long) -> c_int;

#[repr(C)]
#[allow(dead_code)]
struct XImageFuncs {
    create_image: Option<XImageCreateFn>,
    destroy_image: Option<XImageDestroyFn>,
    get_pixel: Option<XImageGetPixelFn>,
    put_pixel: Option<XImagePutPixelFn>,
    sub_image: Option<XImageSubImageFn>,
    add_pixel: Option<XImageAddPixelFn>,
}

#[repr(C)]
#[allow(dead_code)]
struct XImage {
    width: c_int,
    height: c_int,
    xoffset: c_int,
    format: c_int,
    data: *mut c_char,
    byte_order: c_int,
    bitmap_unit: c_int,
    bitmap_bit_order: c_int,
    bitmap_pad: c_int,
    depth: c_int,
    bytes_per_line: c_int,
    bits_per_pixel: c_int,
    red_mask: c_ulong,
    green_mask: c_ulong,
    blue_mask: c_ulong,
    obdata: *mut c_void,
    f: XImageFuncs,
}

#[repr(C)]
#[derive(Copy, Clone)]
#[allow(dead_code)]
struct XKeyEvent {
    kind: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut Display,
    window: Window,
    root: Window,
    subwindow: Window,
    time: c_ulong,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    keycode: c_uint,
}

#[repr(C)]
#[derive(Copy, Clone)]
#[allow(dead_code)]
struct XButtonEvent {
    kind: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut Display,
    window: Window,
    root: Window,
    subwindow: Window,
    time: c_ulong,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    button: c_uint,
}

#[repr(C)]
#[derive(Copy, Clone)]
#[allow(dead_code)]
struct XConfigureEvent {
    kind: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut Display,
    event: Window,
    window: Window,
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
    border_width: c_int,
    above: Window,
    override_redirect: c_int,
}

#[repr(C)]
#[derive(Copy, Clone)]
#[allow(dead_code)]
union XEvent {
    kind: c_int,
    key: XKeyEvent,
    button: XButtonEvent,
    configure: XConfigureEvent,
    pad: [c_long; 24],
}

const ZPIXMAP: c_int = 2;
const TRUE_COLOR: c_int = 4;
const KEY_PRESS: c_int = 2;
const KEY_RELEASE: c_int = 3;
const BUTTON_PRESS: c_int = 4;
const EXPOSE: c_int = 12;
const DESTROY_NOTIFY: c_int = 17;
const CONFIGURE_NOTIFY: c_int = 22;
const KEY_PRESS_MASK: c_long = 1 << 0;
const KEY_RELEASE_MASK: c_long = 1 << 1;
const BUTTON_PRESS_MASK: c_long = 1 << 2;
const EXPOSURE_MASK: c_long = 1 << 15;
const STRUCTURE_NOTIFY_MASK: c_long = 1 << 17;

#[link(name = "X11")]
extern "C" {
    fn XOpenDisplay(name: *const c_char) -> *mut Display;
    fn XDefaultScreen(display: *mut Display) -> c_int;
    fn XRootWindow(display: *mut Display, screen: c_int) -> Window;
    fn XBlackPixel(display: *mut Display, screen: c_int) -> c_ulong;
    fn XWhitePixel(display: *mut Display, screen: c_int) -> c_ulong;
    fn XDefaultVisual(display: *mut Display, screen: c_int) -> *mut Visual;
    fn XDefaultDepth(display: *mut Display, screen: c_int) -> c_int;
    fn XCreateSimpleWindow(
        display: *mut Display,
        parent: Window,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
        border_width: c_uint,
        border: c_ulong,
        background: c_ulong,
    ) -> Window;
    fn XCreateGC(
        display: *mut Display,
        drawable: Drawable,
        value_mask: c_ulong,
        values: *mut XGCValues,
    ) -> GC;
    fn XFreeGC(display: *mut Display, gc: GC) -> c_int;
    fn XDestroyWindow(display: *mut Display, window: Window) -> c_int;
    fn XSelectInput(display: *mut Display, window: Window, event_mask: c_long) -> c_int;
    fn XMapWindow(display: *mut Display, window: Window) -> c_int;
    fn XFlush(display: *mut Display) -> c_int;
    fn XNextEvent(display: *mut Display, event: *mut XEvent) -> c_int;
    fn XCloseDisplay(display: *mut Display) -> c_int;
    fn XCreateImage(
        display: *mut Display,
        visual: *mut Visual,
        depth: c_uint,
        format: c_int,
        offset: c_int,
        data: *mut c_char,
        width: c_uint,
        height: c_uint,
        bitmap_pad: c_int,
        bytes_per_line: c_int,
    ) -> *mut XImage;
    fn XPutImage(
        display: *mut Display,
        drawable: Drawable,
        gc: GC,
        image: *mut XImage,
        src_x: c_int,
        src_y: c_int,
        dest_x: c_int,
        dest_y: c_int,
        width: c_uint,
        height: c_uint,
    ) -> c_int;
    fn XFree(data: *mut c_void) -> c_int;
}

fn checked_len(width: u32, height: u32) -> Result<usize, AdapterError> {
    if width == 0
        || height == 0
        || width > c_int::MAX as u32
        || height > c_int::MAX as u32
        || width > (c_int::MAX as u32) / 4
    {
        return Err(AdapterError::InvalidFrame);
    }
    (width as usize)
        .checked_mul(4)
        .and_then(|row| row.checked_mul(height as usize))
        .ok_or(AdapterError::InvalidFrame)
}

/// Pack RGBA8 through the 32-bit TrueColor masks in XImage native byte order.
/// Bits outside the RGB masks are set to one so the pixel is opaque.
fn rgba_to_xpixel(pixels: &[u8], masks: (u32, u32, u32, u32)) -> Result<Vec<u8>, AdapterError> {
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

unsafe fn destroy_image(image: *mut XImage) {
    if image.is_null() {
        return;
    }
    if let Some(destroy) = (*image).f.destroy_image {
        destroy(image);
    } else {
        XFree((*image).data.cast::<c_void>());
        XFree(image.cast::<c_void>());
    }
}

/// A live X11 window host. `pump()` consumes one `XNextEvent`; `run()` pumps
/// until the window is destroyed.
pub struct X11Host {
    display: *mut Display,
    window: Window,
    gc: GC,
    visual: *mut Visual,
    depth: c_uint,
    masks: (u32, u32, u32, u32),
    viewport: Size,
    events: VecDeque<InputEvent>,
    last_frame: Vec<u8>,
    last_size: Size,
    closed: bool,
}

impl X11Host {
    pub fn open(width: u32, height: u32) -> Result<Self, AdapterError> {
        checked_len(width, height)?;
        let display = unsafe { XOpenDisplay(null()) };
        if display.is_null() {
            return Err(AdapterError::Backend(
                "XOpenDisplay failed: DISPLAY is unset or no X server is reachable",
            ));
        }
        let mut host = Self {
            display,
            window: 0,
            gc: null_mut(),
            visual: null_mut(),
            depth: 0,
            masks: (0, 0, 0, 0),
            viewport: Size { width, height },
            events: VecDeque::new(),
            last_frame: Vec::new(),
            last_size: Size {
                width: 0,
                height: 0,
            },
            closed: false,
        };
        let screen = unsafe { XDefaultScreen(display) };
        let root = unsafe { XRootWindow(display, screen) };
        let border = unsafe { XBlackPixel(display, screen) };
        let background = unsafe { XWhitePixel(display, screen) };
        host.window = unsafe {
            XCreateSimpleWindow(display, root, 0, 0, width, height, 0, border, background)
        };
        if host.window == 0 {
            host.close();
            return Err(AdapterError::Backend("XCreateSimpleWindow failed"));
        }
        host.visual = unsafe { XDefaultVisual(display, screen) };
        let depth = unsafe { XDefaultDepth(display, screen) };
        if host.visual.is_null() || depth < 24 {
            host.close();
            return Err(AdapterError::Backend(
                "X11 requires a 24/32-bit TrueColor visual",
            ));
        }
        let visual = unsafe { &*host.visual };
        if visual.class != TRUE_COLOR {
            host.close();
            return Err(AdapterError::Backend("X11 visual is not TrueColor"));
        }
        let red = visual.red_mask as u32;
        let green = visual.green_mask as u32;
        let blue = visual.blue_mask as u32;
        if red == 0
            || green == 0
            || blue == 0
            || visual.red_mask >> 32 != 0
            || visual.green_mask >> 32 != 0
            || visual.blue_mask >> 32 != 0
        {
            host.close();
            return Err(AdapterError::Backend("X11 TrueColor masks are invalid"));
        }
        host.masks = (red, green, blue, 0xffff_ffff_u32 & !(red | green | blue));
        host.depth = depth as c_uint;
        host.gc = unsafe { XCreateGC(display, host.window, 0, null_mut()) };
        if host.gc.is_null() {
            host.close();
            return Err(AdapterError::Backend("XCreateGC failed"));
        }
        let mask = KEY_PRESS_MASK
            | KEY_RELEASE_MASK
            | BUTTON_PRESS_MASK
            | EXPOSURE_MASK
            | STRUCTURE_NOTIFY_MASK;
        // SAFETY: all handles came from the live Xlib display above.
        let ready = unsafe {
            XSelectInput(display, host.window, mask) != 0
                && XMapWindow(display, host.window) != 0
                && XFlush(display) != 0
        };
        if !ready {
            host.close();
            return Err(AdapterError::Backend(
                "XSelectInput, XMapWindow or XFlush failed",
            ));
        }
        Ok(host)
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        // SAFETY: handles are null-checked and owned by this host.
        unsafe {
            if !self.gc.is_null() {
                XFreeGC(self.display, self.gc);
                self.gc = null_mut();
            }
            if self.window != 0 {
                XDestroyWindow(self.display, self.window);
                self.window = 0;
            }
            if !self.display.is_null() {
                XCloseDisplay(self.display);
                self.display = null_mut();
            }
        }
        self.closed = true;
    }

    fn draw(&mut self, width: u32, height: u32, pixels: &[u8]) -> Result<(), AdapterError> {
        if self.closed {
            return Err(AdapterError::Backend("the X11 display is closed"));
        }
        let row_bytes = (width as usize)
            .checked_mul(4)
            .filter(|&row| row <= c_int::MAX as usize)
            .ok_or(AdapterError::InvalidFrame)? as c_int;
        let image = unsafe {
            XCreateImage(
                self.display,
                self.visual,
                self.depth,
                ZPIXMAP,
                0,
                null_mut(),
                width,
                height,
                32,
                row_bytes,
            )
        };
        if image.is_null() {
            return Err(AdapterError::Backend("XCreateImage failed"));
        }
        // SAFETY: XCreateImage owns its data because `data` was null.
        let result = unsafe {
            let info = &mut *image;
            let data_len = (info.bytes_per_line as usize)
                .checked_mul(height as usize)
                .unwrap_or(0);
            if info.bits_per_pixel != 32
                || info.data.is_null()
                || data_len != pixels.len()
                || data_len == 0
            {
                Err(AdapterError::Backend(
                    "XCreateImage returned an unsupported image",
                ))
            } else {
                std::ptr::copy_nonoverlapping(
                    pixels.as_ptr(),
                    info.data.cast::<u8>(),
                    pixels.len(),
                );
                let put = XPutImage(
                    self.display,
                    self.window,
                    self.gc,
                    image,
                    0,
                    0,
                    0,
                    0,
                    width,
                    height,
                );
                let flushed = XFlush(self.display);
                if put == 0 {
                    Err(AdapterError::Backend("XPutImage failed"))
                } else if flushed == 0 {
                    Err(AdapterError::Backend("XFlush failed"))
                } else {
                    Ok(())
                }
            }
        };
        unsafe { destroy_image(image) };
        result
    }

    /// Consume exactly one X event. `XNextEvent` blocks until one arrives.
    pub fn pump(&mut self) -> Result<(), AdapterError> {
        if self.closed {
            return Err(AdapterError::Backend("the X11 display is closed"));
        }
        let mut event: XEvent = unsafe { std::mem::zeroed() };
        unsafe { XNextEvent(self.display, &mut event) };
        let kind = unsafe { event.kind };
        match kind {
            KEY_PRESS | KEY_RELEASE => {
                let key = unsafe { event.key };
                self.events.push_back(InputEvent::Key {
                    code: key.keycode,
                    pressed: kind == KEY_PRESS,
                });
            }
            BUTTON_PRESS => {
                let button = unsafe { event.button };
                self.events.push_back(InputEvent::Tap {
                    x: button.x as f32,
                    y: button.y as f32,
                    node_id: u32::MAX,
                });
            }
            CONFIGURE_NOTIFY => {
                let configure = unsafe { event.configure };
                let width = u32::try_from(configure.width).unwrap_or(0);
                let height = u32::try_from(configure.height).unwrap_or(0);
                self.events.push_back(InputEvent::Resize {
                    size: Size { width, height },
                });
            }
            EXPOSE => {
                if !self.last_frame.is_empty() {
                    let frame = std::mem::take(&mut self.last_frame);
                    let size = self.last_size;
                    let result = self.draw(size.width, size.height, &frame);
                    self.last_frame = frame;
                    result?;
                }
            }
            DESTROY_NOTIFY => {
                self.events
                    .push_back(InputEvent::Lifecycle(Lifecycle::Destroyed));
                self.close();
            }
            _ => {}
        }
        Ok(())
    }

    pub fn run(&mut self) -> Result<(), AdapterError> {
        while !self.closed {
            self.pump()?;
        }
        Ok(())
    }
}

impl PlatformAdapter for X11Host {
    fn name(&self) -> &str {
        "x11"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            gpu: false,
            native_embed: false,
            multi_window: false,
            lifecycle: true,
            dpi_scale: 1.0,
        }
    }

    fn viewport(&self) -> Size {
        self.viewport
    }

    fn poll_event(&mut self) -> Option<InputEvent> {
        let event = self.events.pop_front()?;
        if let InputEvent::Resize { size } = &event {
            self.viewport = *size;
        }
        Some(event)
    }

    fn present(&mut self, frame: FrameView<'_>) -> Result<(), AdapterError> {
        let expected = checked_len(frame.width, frame.height)?;
        if frame.pixels.len() != expected {
            return Err(AdapterError::InvalidFrame);
        }
        let packed = rgba_to_xpixel(frame.pixels, self.masks)?;
        self.last_size = Size {
            width: frame.width,
            height: frame.height,
        };
        self.last_frame = packed.clone();
        self.draw(frame.width, frame.height, &packed)
    }
}

impl Drop for X11Host {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::{checked_len, rgba_to_xpixel};
    use crate::AdapterError;

    #[test]
    fn rgba_uses_the_visual_masks_in_native_byte_order() {
        let masks = (0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0xff00_0000);
        let packed = rgba_to_xpixel(&[10, 20, 30, 255], masks).unwrap();
        assert_eq!(&packed[..4], &0xff1e140au32.to_ne_bytes());
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
