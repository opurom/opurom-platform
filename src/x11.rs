//! X11 window host (R8_ADDENDUM.md section 3).
//!
//! Xlib is loaded at runtime with `dlopen`/`dlsym` instead of a link-time
//! `-lX11` dependency, so a Linux CI runner needs neither X11 headers nor X11
//! development libraries to build this crate. A missing `libX11.so.6` or a
//! missing symbol is a documented `AdapterError`, never a fake window. This
//! host is only compile-checked here: the R8 macOS host has no X server. A
//! 24/32-bit TrueColor visual is required; RGBA8 is packed through the
//! visual's masks into an `XImage` and presented with `XPutImage`.

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

type XOpenDisplayFn = unsafe extern "C" fn(*const c_char) -> *mut Display;
type XDefaultScreenFn = unsafe extern "C" fn(*mut Display) -> c_int;
type XRootWindowFn = unsafe extern "C" fn(*mut Display, c_int) -> Window;
type XBlackPixelFn = unsafe extern "C" fn(*mut Display, c_int) -> c_ulong;
type XWhitePixelFn = unsafe extern "C" fn(*mut Display, c_int) -> c_ulong;
type XDefaultVisualFn = unsafe extern "C" fn(*mut Display, c_int) -> *mut Visual;
type XDefaultDepthFn = unsafe extern "C" fn(*mut Display, c_int) -> c_int;
type XCreateSimpleWindowFn = unsafe extern "C" fn(
    *mut Display,
    Window,
    c_int,
    c_int,
    c_uint,
    c_uint,
    c_uint,
    c_ulong,
    c_ulong,
) -> Window;
type XCreateGCFn = unsafe extern "C" fn(*mut Display, Drawable, c_ulong, *mut XGCValues) -> GC;
type XFreeGCFn = unsafe extern "C" fn(*mut Display, GC) -> c_int;
type XDestroyWindowFn = unsafe extern "C" fn(*mut Display, Window) -> c_int;
type XSelectInputFn = unsafe extern "C" fn(*mut Display, Window, c_long) -> c_int;
type XMapWindowFn = unsafe extern "C" fn(*mut Display, Window) -> c_int;
type XFlushFn = unsafe extern "C" fn(*mut Display) -> c_int;
type XNextEventFn = unsafe extern "C" fn(*mut Display, *mut XEvent) -> c_int;
type XCloseDisplayFn = unsafe extern "C" fn(*mut Display) -> c_int;
type XPutImageFn = unsafe extern "C" fn(
    *mut Display,
    Drawable,
    GC,
    *mut XImage,
    c_int,
    c_int,
    c_int,
    c_int,
    c_uint,
    c_uint,
) -> c_int;
type XFreeFn = unsafe extern "C" fn(*mut c_void) -> c_int;

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
const RTLD_NOW: c_int = 2;
const RTLD_LOCAL: c_int = 0;
const X11_SONAMES: [&[u8]; 2] = [b"libX11.so.6\0", b"libX11.so\0"];
const X11_LOAD_ERROR: &str = "X11 unavailable: dlopen(libX11.so.6/libX11.so) failed; install the X11 runtime or use a headless adapter";
const X11_SYMBOL_ERROR: &str = "X11 unavailable: libX11 is missing a required Xlib symbol";

// Modern glibc exports these from libc; there is deliberately no -ldl link.
extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *mut c_char;
    fn dlclose(handle: *mut c_void) -> c_int;
}

/// All Xlib entry points resolved from one runtime library handle.
struct Xlib {
    handle: *mut c_void,
    open_display: XOpenDisplayFn,
    default_screen: XDefaultScreenFn,
    root_window: XRootWindowFn,
    black_pixel: XBlackPixelFn,
    white_pixel: XWhitePixelFn,
    default_visual: XDefaultVisualFn,
    default_depth: XDefaultDepthFn,
    create_simple_window: XCreateSimpleWindowFn,
    create_gc: XCreateGCFn,
    free_gc: XFreeGCFn,
    destroy_window: XDestroyWindowFn,
    select_input: XSelectInputFn,
    map_window: XMapWindowFn,
    flush: XFlushFn,
    next_event: XNextEventFn,
    close_display: XCloseDisplayFn,
    create_image: XImageCreateFn,
    put_image: XPutImageFn,
    free: XFreeFn,
}

impl Xlib {
    fn load() -> Result<Self, AdapterError> {
        let handle = unsafe { open_x11_library() };
        if handle.is_null() {
            return Err(AdapterError::Backend(X11_LOAD_ERROR));
        }
        let loaded = unsafe { Self::from_handle(handle) };
        if loaded.is_err() {
            // SAFETY: no Xlib value owns the handle after a failed resolution.
            unsafe { dlclose(handle) };
        }
        loaded
    }

    unsafe fn from_handle(handle: *mut c_void) -> Result<Self, AdapterError> {
        Ok(Self {
            handle,
            open_display: symbol(handle, b"XOpenDisplay\0")?,
            default_screen: symbol(handle, b"XDefaultScreen\0")?,
            root_window: symbol(handle, b"XRootWindow\0")?,
            black_pixel: symbol(handle, b"XBlackPixel\0")?,
            white_pixel: symbol(handle, b"XWhitePixel\0")?,
            default_visual: symbol(handle, b"XDefaultVisual\0")?,
            default_depth: symbol(handle, b"XDefaultDepth\0")?,
            create_simple_window: symbol(handle, b"XCreateSimpleWindow\0")?,
            create_gc: symbol(handle, b"XCreateGC\0")?,
            free_gc: symbol(handle, b"XFreeGC\0")?,
            destroy_window: symbol(handle, b"XDestroyWindow\0")?,
            select_input: symbol(handle, b"XSelectInput\0")?,
            map_window: symbol(handle, b"XMapWindow\0")?,
            flush: symbol(handle, b"XFlush\0")?,
            next_event: symbol(handle, b"XNextEvent\0")?,
            close_display: symbol(handle, b"XCloseDisplay\0")?,
            create_image: symbol(handle, b"XCreateImage\0")?,
            put_image: symbol(handle, b"XPutImage\0")?,
            free: symbol(handle, b"XFree\0")?,
        })
    }
}

impl Drop for Xlib {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: the handle came from dlopen and is closed exactly once.
            unsafe { dlclose(self.handle) };
            self.handle = null_mut();
        }
    }
}

unsafe fn open_x11_library() -> *mut c_void {
    for name in X11_SONAMES {
        let handle = dlopen(name.as_ptr().cast::<c_char>(), RTLD_NOW | RTLD_LOCAL);
        if !handle.is_null() {
            return handle;
        }
        let _ = dlerror();
    }
    null_mut()
}

unsafe fn symbol<T>(handle: *mut c_void, name: &[u8]) -> Result<T, AdapterError> {
    let _ = dlerror();
    let pointer = dlsym(handle, name.as_ptr().cast::<c_char>());
    let error = dlerror();
    if pointer.is_null() || !error.is_null() {
        return Err(AdapterError::Backend(X11_SYMBOL_ERROR));
    }
    // POSIX permits a void* symbol address to be used as a function pointer;
    // on Linux both representations have the same size.
    Ok(std::mem::transmute_copy::<*mut c_void, T>(&pointer))
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

unsafe fn destroy_image(lib: &Xlib, image: *mut XImage) {
    if image.is_null() {
        return;
    }
    if let Some(destroy) = (*image).f.destroy_image {
        destroy(image);
    } else {
        (lib.free)((*image).data.cast::<c_void>());
        (lib.free)(image.cast::<c_void>());
    }
}

/// A live X11 window host. `pump()` consumes one `XNextEvent`; `run()` pumps
/// until the window is destroyed. Xlib is loaded only when `open()` is called.
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
    lib: Xlib,
}

impl X11Host {
    pub fn open(width: u32, height: u32) -> Result<Self, AdapterError> {
        checked_len(width, height)?;
        let lib = Xlib::load()?;
        let display = unsafe { (lib.open_display)(null()) };
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
            lib,
        };
        let screen = unsafe { (host.lib.default_screen)(display) };
        let root = unsafe { (host.lib.root_window)(display, screen) };
        let border = unsafe { (host.lib.black_pixel)(display, screen) };
        let background = unsafe { (host.lib.white_pixel)(display, screen) };
        host.window = unsafe {
            (host.lib.create_simple_window)(
                display, root, 0, 0, width, height, 0, border, background,
            )
        };
        if host.window == 0 {
            host.close();
            return Err(AdapterError::Backend("XCreateSimpleWindow failed"));
        }
        host.visual = unsafe { (host.lib.default_visual)(display, screen) };
        let depth = unsafe { (host.lib.default_depth)(display, screen) };
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
        host.gc = unsafe { (host.lib.create_gc)(display, host.window, 0, null_mut()) };
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
            (host.lib.select_input)(display, host.window, mask) != 0
                && (host.lib.map_window)(display, host.window) != 0
                && (host.lib.flush)(display) != 0
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
            if !self.display.is_null() {
                if !self.gc.is_null() {
                    (self.lib.free_gc)(self.display, self.gc);
                }
                if self.window != 0 {
                    (self.lib.destroy_window)(self.display, self.window);
                }
                (self.lib.close_display)(self.display);
            }
        }
        self.gc = null_mut();
        self.window = 0;
        self.display = null_mut();
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
            (self.lib.create_image)(
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
                let put = (self.lib.put_image)(
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
                let flushed = (self.lib.flush)(self.display);
                if put == 0 {
                    Err(AdapterError::Backend("XPutImage failed"))
                } else if flushed == 0 {
                    Err(AdapterError::Backend("XFlush failed"))
                } else {
                    Ok(())
                }
            }
        };
        unsafe { destroy_image(&self.lib, image) };
        result
    }

    /// Consume exactly one X event. `XNextEvent` blocks until one arrives.
    pub fn pump(&mut self) -> Result<(), AdapterError> {
        if self.closed {
            return Err(AdapterError::Backend("the X11 display is closed"));
        }
        let mut event: XEvent = unsafe { std::mem::zeroed() };
        unsafe { (self.lib.next_event)(self.display, &mut event) };
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
    use super::{checked_len, rgba_to_xpixel, Xlib};
    use crate::AdapterError;

    #[test]
    fn runtime_loader_reports_missing_x11_or_resolves_all_symbols() {
        match Xlib::load() {
            Ok(lib) => assert!(!lib.handle.is_null()),
            Err(error) => assert!(error.to_string().contains("X11 unavailable"), "{error}"),
        }
    }

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
