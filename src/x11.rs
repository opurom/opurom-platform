//! X11 window host (the surface implementation framework rules).
//!
//! Xlib is loaded at runtime with `dlopen`/`dlsym` instead of a link-time
//! `-lX11` dependency, so a Linux CI runner needs neither X11 headers nor X11
//! development libraries to build this crate. A missing `libX11.so.6` or a
//! missing symbol is a documented `AdapterError`, never a fake window. This
//! host is only compile-checked here: this macOS host has no X server. A
//! 24/32-bit TrueColor visual is required; RGBA8 is packed through the
//! visual's masks into an `XImage` and presented with `XPutImage`.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::ptr::{null, null_mut};

use crate::{
    app_lifecycle::LifecycleTracker,
    ime::ImeStatus,
    input_map,
    x11_pixels::{checked_len, rgba_to_xpixel},
    AdapterError, Capabilities, FrameView, InputEvent, Lifecycle, PlatformAdapter, Size,
};

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
struct XAnyEvent {
    kind: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut Display,
    window: Window,
}

#[repr(C)]
#[derive(Copy, Clone)]
#[allow(dead_code)]
union XEvent {
    kind: c_int,
    any: XAnyEvent,
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
type XPendingFn = unsafe extern "C" fn(*mut Display) -> c_int;
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
type XOpenIMFn =
    unsafe extern "C" fn(*mut Display, *mut c_void, *const c_char, *const c_char) -> *mut c_void;
// `Status` is `int` in Xlib; the return value is ignored here.
type XCloseIMFn = unsafe extern "C" fn(*mut c_void) -> c_int;
// XCreateIC is variadic and takes a NULL-terminated name/value list.
type XCreateICFn = unsafe extern "C" fn(*mut c_void, ...) -> *mut c_void;
type XDestroyICFn = unsafe extern "C" fn(*mut c_void);
type XSetICFocusFn = unsafe extern "C" fn(*mut c_void);
type Xutf8LookupStringFn = unsafe extern "C" fn(
    *mut c_void,
    *mut XKeyEvent,
    *mut c_char,
    c_int,
    *mut c_long,
    *mut c_int,
) -> c_int;
type XDisplayWidthFn = unsafe extern "C" fn(*mut Display, c_int) -> c_int;

const ZPIXMAP: c_int = 2;
const TRUE_COLOR: c_int = 4;
const KEY_PRESS: c_int = 2;
const KEY_RELEASE: c_int = 3;
const BUTTON_PRESS: c_int = 4;
const BUTTON_RELEASE: c_int = 5;
const MOTION_NOTIFY: c_int = 6;
const EXPOSE: c_int = 12;
const DESTROY_NOTIFY: c_int = 17;
const CONFIGURE_NOTIFY: c_int = 22;
const FOCUS_IN: c_int = 9;
const FOCUS_OUT: c_int = 10;
const KEY_PRESS_MASK: c_long = 1 << 0;
const KEY_RELEASE_MASK: c_long = 1 << 1;
const BUTTON_PRESS_MASK: c_long = 1 << 2;
const BUTTON_RELEASE_MASK: c_long = 1 << 3;
const POINTER_MOTION_MASK: c_long = 1 << 6;
const EXPOSURE_MASK: c_long = 1 << 15;
const STRUCTURE_NOTIFY_MASK: c_long = 1 << 17;
const FOCUS_CHANGE_MASK: c_long = 1 << 21;
/// `Button1Mask` in an event's `state` field: the primary button is held.
const BUTTON1_MASK: c_uint = 1 << 8;
/// `XIMPreeditNothing | XIMStatusNothing`: the input method draws its own
/// preedit and status windows, so the host needs no XIM callbacks.
const XIM_NOTHING_STYLE: c_ulong = 0x0008 | 0x0400;
const XN_INPUT_STYLE: &[u8] = b"inputStyle\0";
const XN_CLIENT_WINDOW: &[u8] = b"clientWindow\0";
/// Bytes per composed key press; a longer composition is dropped rather than
/// resized (`shortcut`: grow on `XBufferOverflow` when real IMEs hit it).
const LOOKUP_BUFFER: usize = 64;
const XIM_OPEN_ERROR: &str = "XOpenIM failed: the locale names no supported input method";
const XIM_IC_ERROR: &str = "XCreateIC failed: the server accepts no supported input style";
const X11_CLOSED_ERROR: &str = "the X11 display is closed";
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
    pending: XPendingFn,
    close_display: XCloseDisplayFn,
    create_image: XImageCreateFn,
    put_image: XPutImageFn,
    free: XFreeFn,
    open_im: XOpenIMFn,
    close_im: XCloseIMFn,
    create_ic: XCreateICFn,
    destroy_ic: XDestroyICFn,
    set_ic_focus: XSetICFocusFn,
    utf8_lookup: Xutf8LookupStringFn,
    display_width: XDisplayWidthFn,
    display_width_mm: XDisplayWidthFn,
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
            pending: symbol(handle, b"XPending\0")?,
            close_display: symbol(handle, b"XCloseDisplay\0")?,
            create_image: symbol(handle, b"XCreateImage\0")?,
            put_image: symbol(handle, b"XPutImage\0")?,
            free: symbol(handle, b"XFree\0")?,
            open_im: symbol(handle, b"XOpenIM\0")?,
            close_im: symbol(handle, b"XCloseIM\0")?,
            create_ic: symbol(handle, b"XCreateIC\0")?,
            destroy_ic: symbol(handle, b"XDestroyIC\0")?,
            set_ic_focus: symbol(handle, b"XSetICFocus\0")?,
            utf8_lookup: symbol(handle, b"Xutf8LookupString\0")?,
            display_width: symbol(handle, b"XDisplayWidth\0")?,
            display_width_mm: symbol(handle, b"XDisplayWidthMM\0")?,
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

/// Raw X coordinates to logical layout pixels for one window's scale.
fn logical_xy(scale: f32, x: f32, y: f32) -> (f32, f32) {
    (
        input_map::logical_coord(x, scale),
        input_map::logical_coord(y, scale),
    )
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
    xim: *mut c_void,
    xic: *mut c_void,
    ime: ImeStatus,
    scale: f32,
    lifecycle: LifecycleTracker,
    lib: Xlib,
}

/// Open a window's input method: a locale or server that refuses leaves raw
/// keys only, reported as the typed status. Shared by the single- and
/// multi-window hosts.
/// shortcut: `XFilterEvent` is not wired; with the Nothing styles the input
/// method draws its own windows, so no client-side filtering is due.
fn setup_ime(
    lib: &Xlib,
    display: *mut Display,
    window: Window,
    xim_out: &mut *mut c_void,
    xic_out: &mut *mut c_void,
    status: &mut ImeStatus,
) {
    if display.is_null() || window == 0 {
        return;
    }
    // SAFETY: the display is live and the variadic list is NULL-terminated.
    let xim = unsafe { (lib.open_im)(display, null_mut(), null(), null()) };
    if xim.is_null() {
        *status = ImeStatus::Unavailable(XIM_OPEN_ERROR);
        return;
    }
    let xic = unsafe {
        (lib.create_ic)(
            xim,
            XN_INPUT_STYLE.as_ptr().cast::<c_char>(),
            XIM_NOTHING_STYLE,
            XN_CLIENT_WINDOW.as_ptr().cast::<c_char>(),
            window,
            std::ptr::null::<c_char>(),
        )
    };
    if xic.is_null() {
        unsafe { (lib.close_im)(xim) };
        *status = ImeStatus::Unavailable(XIM_IC_ERROR);
        return;
    }
    unsafe { (lib.set_ic_focus)(xic) };
    *xim_out = xim;
    *xic_out = xic;
    *status = ImeStatus::Ready;
}

/// Compose one key press through the input method into `events`: bytes that
/// form text become `Input` beside the raw `Key`.
fn lookup_text(
    lib: &Xlib,
    xic: *mut c_void,
    mut key: XKeyEvent,
    events: &mut VecDeque<InputEvent>,
) {
    let mut buffer = [0u8; LOOKUP_BUFFER];
    let mut keysym: c_long = 0;
    let mut status: c_int = 0;
    // SAFETY: the input context and the event are live objects of this host,
    // and the buffer outlives the call.
    let written = unsafe {
        (lib.utf8_lookup)(
            xic,
            &mut key,
            buffer.as_mut_ptr().cast::<c_char>(),
            LOOKUP_BUFFER as c_int,
            &mut keysym,
            &mut status,
        )
    };
    if written <= 0 {
        return;
    }
    let text = String::from_utf8_lossy(&buffer[..written as usize]).into_owned();
    if !text.is_empty() {
        events.push_back(InputEvent::Input {
            node_id: u32::MAX,
            text,
        });
    }
}

/// One window's content scale from the screen's physical pixel density.
fn screen_scale(lib: &Xlib, display: *mut Display, screen: c_int) -> f32 {
    input_map::dpi_from_millimeters(
        unsafe { (lib.display_width)(display, screen) } as u32,
        unsafe { (lib.display_width_mm)(display, screen) } as u32,
    )
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
            xim: null_mut(),
            xic: null_mut(),
            ime: ImeStatus::Unavailable(XIM_OPEN_ERROR),
            scale: 1.0,
            lifecycle: LifecycleTracker::new(),
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
            | BUTTON_RELEASE_MASK
            | POINTER_MOTION_MASK
            | EXPOSURE_MASK
            | STRUCTURE_NOTIFY_MASK
            | FOCUS_CHANGE_MASK;
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
        setup_ime(
            &host.lib,
            display,
            host.window,
            &mut host.xim,
            &mut host.xic,
            &mut host.ime,
        );
        // The ABI's degradation for platforms without scale events: one Scale
        // at window build, from the screen's physical pixel density.
        let scale = screen_scale(&host.lib, display, screen);
        host.scale = scale;
        host.viewport = input_map::logical_size(host.viewport, scale);
        host.events.push_back(InputEvent::Scale { scale });
        host.events.push_back(InputEvent::Resize {
            size: host.viewport,
        });
        Ok(host)
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        // SAFETY: handles are null-checked and owned by this host.
        unsafe {
            if !self.display.is_null() {
                if !self.xic.is_null() {
                    (self.lib.destroy_ic)(self.xic);
                    self.xic = null_mut();
                }
                if !self.xim.is_null() {
                    (self.lib.close_im)(self.xim);
                    self.xim = null_mut();
                }
                if !self.gc.is_null() {
                    (self.lib.free_gc)(self.display, self.gc);
                }
                if self.window != 0 {
                    (self.lib.destroy_window)(self.display, self.window);
                }
                (self.lib.close_display)(self.display);
            }
        }
        self.ime = ImeStatus::Unavailable("the X11 display is closed");
        self.gc = null_mut();
        self.window = 0;
        self.display = null_mut();
        self.closed = true;
    }

    /// Open the window's input method: a locale or server that refuses leaves
    /// the host with raw keys only, reported as a typed status.
    /// shortcut: `XFilterEvent` is not wired; with the Nothing styles the
    /// input method draws its own windows, so no client-side filtering is due.
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
                if kind == KEY_PRESS && !self.xic.is_null() {
                    lookup_text(&self.lib, self.xic, key, &mut self.events);
                }
            }
            BUTTON_PRESS => {
                let button = unsafe { event.button };
                let (x, y) = logical_xy(self.scale, button.x as f32, button.y as f32);
                match input_map::x11_scroll_notch(button.button) {
                    Some(delta) => self.events.push_back(InputEvent::Wheel { x, y, delta }),
                    None => self.events.push_back(InputEvent::Tap {
                        x,
                        y,
                        node_id: u32::MAX,
                    }),
                }
            }
            BUTTON_RELEASE => {
                let button = unsafe { event.button };
                // A scroll notch reports no release of its own in the ABI.
                if input_map::x11_scroll_notch(button.button).is_none() {
                    let (x, y) = logical_xy(self.scale, button.x as f32, button.y as f32);
                    self.events.push_back(InputEvent::PointerUp { x, y });
                }
            }
            MOTION_NOTIFY => {
                // A motion event shares the button event's field layout
                // through `state`, which is all this arm reads.
                let motion = unsafe { event.button };
                let (x, y) = logical_xy(self.scale, motion.x as f32, motion.y as f32);
                self.events.push_back(input_map::pointer_move(
                    motion.state & BUTTON1_MASK != 0,
                    x,
                    y,
                ));
            }
            CONFIGURE_NOTIFY => {
                let configure = unsafe { event.configure };
                let width = u32::try_from(configure.width).unwrap_or(0);
                let height = u32::try_from(configure.height).unwrap_or(0);
                self.events.push_back(InputEvent::Resize {
                    size: input_map::logical_size(Size { width, height }, self.scale),
                });
            }
            FOCUS_IN => {
                for state in self.lifecycle.report(Lifecycle::Resumed) {
                    self.events.push_back(InputEvent::Lifecycle(state));
                }
            }
            FOCUS_OUT => {
                for state in self.lifecycle.report(Lifecycle::Inactive) {
                    self.events.push_back(InputEvent::Lifecycle(state));
                }
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
                for state in self.lifecycle.report(Lifecycle::Destroyed) {
                    self.events.push_back(InputEvent::Lifecycle(state));
                }
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

/// A real multi-window X11 host: one display connection, 2..4 windows and a
/// single event pump that dispatches by `XAnyEvent.window`, with the
/// `WindowSet` model's index, broadcast and per-window present shape.
/// `multi_window` reports true.
///
/// shortcut: compiled and unit-tested for the Linux target only, never
/// executed here; the per-event mapping mirrors the single-window pump.
pub struct X11MultiHost {
    lib: Xlib,
    display: *mut Display,
    windows: BTreeMap<Window, X11Window>,
    order: Vec<Window>,
    closed: bool,
}

struct X11Window {
    window: Window,
    gc: GC,
    masks: (u32, u32, u32, u32),
    depth: c_uint,
    viewport: Size,
    /// Physical backing size of `last_frame`; the viewport stays logical.
    frame_size: Size,
    events: VecDeque<InputEvent>,
    last_frame: Vec<u8>,
    ime: ImeStatus,
    xim: *mut c_void,
    xic: *mut c_void,
    scale: f32,
    lifecycle: LifecycleTracker,
    alive: bool,
}

impl X11MultiHost {
    /// Open 2..4 real windows on one display, one per size, in index order.
    pub fn open<const N: usize>(sizes: [Size; N]) -> Result<Self, AdapterError> {
        if !(2..=4).contains(&N) {
            return Err(AdapterError::Backend(
                crate::window_set::INVALID_WINDOW_COUNT,
            ));
        }
        let mut sizes = sizes.into_iter();
        let first = sizes.next().expect("2..=4 checked above");
        let first_host = X11Host::open(first.width, first.height)?;
        let display = first_host.display;
        let lib = unsafe { std::ptr::read(&first_host.lib) };
        std::mem::forget(first_host);
        let screen = unsafe { (lib.default_screen)(display) };
        let root = unsafe { (lib.root_window)(display, screen) };
        let border = unsafe { (lib.black_pixel)(display, screen) };
        let background = unsafe { (lib.white_pixel)(display, screen) };
        let visual = unsafe { (lib.default_visual)(display, screen) };
        let depth = unsafe { (lib.default_depth)(display, screen) } as c_uint;
        let masks = {
            let info = unsafe { &*visual };
            let red = info.red_mask as u32;
            let green = info.green_mask as u32;
            let blue = info.blue_mask as u32;
            (red, green, blue, 0xffff_ffff_u32 & !(red | green | blue))
        };
        let mut host = Self {
            lib,
            display,
            windows: BTreeMap::new(),
            order: Vec::new(),
            closed: false,
        };
        let rest: Vec<Size> = sizes.collect();
        for size in std::iter::once(first).chain(rest) {
            let result = (|| -> Result<Window, AdapterError> {
                checked_len(size.width, size.height)?;
                let window = unsafe {
                    (host.lib.create_simple_window)(
                        display,
                        root,
                        0,
                        0,
                        size.width,
                        size.height,
                        0,
                        border,
                        background,
                    )
                };
                if window == 0 {
                    return Err(AdapterError::Backend("XCreateSimpleWindow failed"));
                }
                let gc = unsafe { (host.lib.create_gc)(display, window, 0, null_mut()) };
                if gc.is_null() {
                    unsafe { (host.lib.destroy_window)(display, window) };
                    return Err(AdapterError::Backend("XCreateGC failed"));
                }
                let mask = KEY_PRESS_MASK
                    | KEY_RELEASE_MASK
                    | BUTTON_PRESS_MASK
                    | BUTTON_RELEASE_MASK
                    | POINTER_MOTION_MASK
                    | EXPOSURE_MASK
                    | STRUCTURE_NOTIFY_MASK
                    | FOCUS_CHANGE_MASK;
                let ready = unsafe {
                    (host.lib.select_input)(display, window, mask) != 0
                        && (host.lib.map_window)(display, window) != 0
                };
                if !ready {
                    unsafe {
                        (host.lib.free_gc)(display, gc);
                        (host.lib.destroy_window)(display, window);
                    }
                    return Err(AdapterError::Backend("XSelectInput or XMapWindow failed"));
                }
                let mut xim = null_mut();
                let mut xic = null_mut();
                let mut ime = ImeStatus::Unavailable(XIM_OPEN_ERROR);
                setup_ime(&host.lib, display, window, &mut xim, &mut xic, &mut ime);
                let scale = screen_scale(&host.lib, display, screen);
                let mut events = VecDeque::new();
                events.push_back(InputEvent::Scale { scale });
                events.push_back(InputEvent::Resize {
                    size: input_map::logical_size(size, scale),
                });
                host.windows.insert(
                    window,
                    X11Window {
                        window,
                        gc,
                        masks,
                        depth,
                        viewport: input_map::logical_size(size, scale),
                        frame_size: size,
                        events,
                        last_frame: Vec::new(),
                        ime,
                        xim,
                        xic,
                        scale,
                        lifecycle: LifecycleTracker::new(),
                        alive: true,
                    },
                );
                host.order.push(window);
                Ok(window)
            })();
            if let Err(error) = result {
                host.close();
                return Err(error);
            }
        }
        unsafe { (host.lib.flush)(display) };
        Ok(host)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether this window can compose text through the input method.
    pub fn ime(&self, index: usize) -> Result<ImeStatus, AdapterError> {
        Ok(self.window(index)?.ime)
    }

    /// Raw framebuffer size of every window in index order.
    pub fn viewports(&self) -> Vec<Size> {
        self.order
            .iter()
            .filter_map(|id| self.windows.get(id).map(|w| w.viewport))
            .collect()
    }

    fn window(&self, index: usize) -> Result<&X11Window, AdapterError> {
        let id = self.order.get(index).ok_or(AdapterError::Backend(
            crate::window_set::INVALID_WINDOW_INDEX,
        ))?;
        self.windows
            .get(id)
            .ok_or(AdapterError::Backend(X11_CLOSED_ERROR))
    }

    /// Deliver the same normalized input to every window.
    pub fn broadcast_event(&mut self, event: InputEvent) {
        for window in self.windows.values_mut() {
            window.events.push_back(event.clone());
        }
    }

    pub fn push_event(&mut self, index: usize, event: InputEvent) -> Result<(), AdapterError> {
        self.window(index)?;
        let id = self.order[index];
        if let Some(window) = self.windows.get_mut(&id) {
            window.events.push_back(event);
        }
        Ok(())
    }

    pub fn poll_event(&mut self, index: usize) -> Result<Option<InputEvent>, AdapterError> {
        self.window(index)?;
        let id = self.order[index];
        let event = {
            let window = self
                .windows
                .get_mut(&id)
                .ok_or(AdapterError::Backend(X11_CLOSED_ERROR))?;
            window.events.pop_front()
        };
        if let Some(InputEvent::Resize { size }) = &event {
            if let Some(window) = self.windows.get_mut(&id) {
                window.viewport = *size;
            }
        }
        Ok(event)
    }

    pub fn viewport(&self, index: usize) -> Result<Size, AdapterError> {
        Ok(self.window(index)?.viewport)
    }

    pub fn present(&mut self, index: usize, frame: FrameView<'_>) -> Result<(), AdapterError> {
        self.window(index)?;
        let expected = checked_len(frame.width, frame.height)?;
        if frame.pixels.len() != expected {
            return Err(AdapterError::InvalidFrame);
        }
        let id = self.order[index];
        let window = self
            .windows
            .get_mut(&id)
            .ok_or(AdapterError::Backend(X11_CLOSED_ERROR))?;
        let packed = rgba_to_xpixel(frame.pixels, window.masks)?;
        window.frame_size = Size {
            width: frame.width,
            height: frame.height,
        };
        window.last_frame = packed.clone();
        let (lib, display) = (&self.lib, self.display);
        draw_window(lib, display, window, frame.width, frame.height, &packed)
    }

    /// Consume one X event, dispatching it to the window named by
    /// `XAnyEvent.window`. An event for an unknown window is dropped.
    pub fn pump(&mut self) -> Result<(), AdapterError> {
        if self.closed {
            return Err(AdapterError::Backend(X11_CLOSED_ERROR));
        }
        let mut event: XEvent = unsafe { std::mem::zeroed() };
        unsafe { (self.lib.next_event)(self.display, &mut event) };
        let target = unsafe { event.any.window };
        let Some(window) = self.windows.get_mut(&target) else {
            return Ok(());
        };
        dispatch_window(&self.lib, self.display, window, event)?;
        Ok(())
    }

    /// Run until no window remains, pumping while `XPending` reports work and
    /// sleeping one millisecond when idle.
    pub fn run(&mut self) -> Result<(), AdapterError> {
        while !self.closed && self.order.iter().any(|id| self.windows.contains_key(id)) {
            if unsafe { (self.lib.pending)(self.display) } > 0 {
                self.pump()?;
            } else {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        Ok(())
    }

    pub fn capabilities(&self) -> Capabilities {
        let dpi_scale = self
            .order
            .first()
            .and_then(|id| self.windows.get(id))
            .map(|window| window.scale)
            .unwrap_or(1.0);
        Capabilities {
            gpu: false,
            native_embed: false,
            multi_window: true,
            lifecycle: true,
            dpi_scale,
        }
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        let ids: Vec<Window> = self.order.clone();
        for id in ids {
            if let Some(window) = self.windows.remove(&id) {
                destroy_window(&self.lib, self.display, window);
            }
        }
        if !self.display.is_null() {
            unsafe { (self.lib.close_display)(self.display) };
            self.display = null_mut();
        }
        self.closed = true;
    }
}

impl Drop for X11MultiHost {
    fn drop(&mut self) {
        self.close();
    }
}

/// Destroy one window's gc, input-method objects and window.
fn destroy_window(lib: &Xlib, display: *mut Display, window: X11Window) {
    unsafe {
        if !display.is_null() {
            if !window.xic.is_null() {
                (lib.destroy_ic)(window.xic);
            }
            if !window.xim.is_null() {
                (lib.close_im)(window.xim);
            }
            if !window.gc.is_null() {
                (lib.free_gc)(display, window.gc);
            }
            if window.window != 0 {
                (lib.destroy_window)(display, window.window);
            }
        }
    }
}

/// Draw one packed frame into its window and flush.
fn draw_window(
    lib: &Xlib,
    display: *mut Display,
    window: &X11Window,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<(), AdapterError> {
    let row_bytes = (width as usize)
        .checked_mul(4)
        .filter(|&row| row <= c_int::MAX as usize)
        .ok_or(AdapterError::InvalidFrame)? as c_int;
    let image = unsafe {
        (lib.create_image)(
            display,
            null_mut(),
            window.depth,
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
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), info.data.cast::<u8>(), pixels.len());
            let put = (lib.put_image)(
                display,
                window.window,
                window.gc,
                image,
                0,
                0,
                0,
                0,
                width,
                height,
            );
            let flushed = (lib.flush)(display);
            if put == 0 {
                Err(AdapterError::Backend("XPutImage failed"))
            } else if flushed == 0 {
                Err(AdapterError::Backend("XFlush failed"))
            } else {
                Ok(())
            }
        }
    };
    unsafe { destroy_image(lib, image) };
    result
}

/// Route one X event to the window it names: key/button/motion/wheel input,
/// resize, expose (redraw) and destroy (destroy + mark dead).
fn dispatch_window(
    lib: &Xlib,
    display: *mut Display,
    window: &mut X11Window,
    event: XEvent,
) -> Result<(), AdapterError> {
    let kind = unsafe { event.kind };
    match kind {
        KEY_PRESS | KEY_RELEASE => {
            let key = unsafe { event.key };
            window.events.push_back(InputEvent::Key {
                code: key.keycode,
                pressed: kind == KEY_PRESS,
            });
            if kind == KEY_PRESS && !window.xic.is_null() {
                lookup_text(lib, window.xic, key, &mut window.events);
            }
        }
        BUTTON_PRESS => {
            let button = unsafe { event.button };
            let (x, y) = logical_xy(window.scale, button.x as f32, button.y as f32);
            match input_map::x11_scroll_notch(button.button) {
                Some(delta) => window.events.push_back(InputEvent::Wheel { x, y, delta }),
                None => window.events.push_back(InputEvent::Tap {
                    x,
                    y,
                    node_id: u32::MAX,
                }),
            }
        }
        BUTTON_RELEASE => {
            let button = unsafe { event.button };
            if input_map::x11_scroll_notch(button.button).is_none() {
                let (x, y) = logical_xy(window.scale, button.x as f32, button.y as f32);
                window.events.push_back(InputEvent::PointerUp { x, y });
            }
        }
        MOTION_NOTIFY => {
            let motion = unsafe { event.button };
            let (x, y) = logical_xy(window.scale, motion.x as f32, motion.y as f32);
            window.events.push_back(input_map::pointer_move(
                motion.state & BUTTON1_MASK != 0,
                x,
                y,
            ));
        }
        CONFIGURE_NOTIFY => {
            let configure = unsafe { event.configure };
            let client = Size {
                width: u32::try_from(configure.width).unwrap_or(0),
                height: u32::try_from(configure.height).unwrap_or(0),
            };
            window.events.push_back(InputEvent::Resize {
                size: input_map::logical_size(client, window.scale),
            });
        }
        FOCUS_IN => {
            for state in window.lifecycle.report(Lifecycle::Resumed) {
                window.events.push_back(InputEvent::Lifecycle(state));
            }
        }
        FOCUS_OUT => {
            for state in window.lifecycle.report(Lifecycle::Inactive) {
                window.events.push_back(InputEvent::Lifecycle(state));
            }
        }
        EXPOSE => {
            if !window.last_frame.is_empty() {
                draw_window(
                    lib,
                    display,
                    window,
                    window.frame_size.width,
                    window.frame_size.height,
                    &window.last_frame.clone(),
                )?;
            }
        }
        DESTROY_NOTIFY => {
            for state in window.lifecycle.report(Lifecycle::Destroyed) {
                window.events.push_back(InputEvent::Lifecycle(state));
            }
            window.alive = false;
        }
        _ => {}
    }
    Ok(())
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
            dpi_scale: self.scale,
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
    use super::Xlib;

    #[test]
    fn runtime_loader_reports_missing_x11_or_resolves_all_symbols() {
        match Xlib::load() {
            Ok(lib) => assert!(!lib.handle.is_null()),
            Err(error) => assert!(error.to_string().contains("X11 unavailable"), "{error}"),
        }
    }
}
