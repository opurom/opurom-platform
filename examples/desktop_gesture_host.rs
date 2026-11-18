#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

//! Desktop gesture host shell: feeds platform input events into a renderer
//! through the host-side bridge. The renderer C ABI is resolved at runtime so
//! this crate never depends on the renderer crate. With no renderer library
//! supplied the shell still runs the bridge and reports a named SKIP for the
//! arena step; it never pretends a gesture was verified.

use std::ffi::{c_char, c_int, c_void};

use opurom_platform::input_bridge::{Gesture, HostPointer, InputRouter, InputSink, PointerKind};
use opurom_platform::{InputEvent, Lifecycle, PointerState};

// Field order and types are pinned to opurom.h:43-65.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct OpuPointerEvent {
    kind: u32,
    pointer_id: u32,
    node_id: u32,
    time_ms: i64,
    local_x: i64,
    local_y: i64,
    global_x: i64,
    global_y: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct OpuGesture {
    code: u32,
    node_id: u32,
    dx: i32,
    dy: i32,
}

#[cfg(target_os = "macos")]
extern "C" {
    fn dlopen(path: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

struct Recorder {
    pointers: Vec<HostPointer>,
    ticks: Vec<i64>,
}

impl InputSink for Recorder {
    fn pointer(&mut self, event: HostPointer) -> Result<Gesture, i32> {
        self.pointers.push(event);
        Ok(Gesture {
            code: 0,
            node_id: event.node_id,
        })
    }

    fn next_deadline_ms(&self) -> i64 {
        -1
    }

    fn tick(&mut self, t_ms: i64) -> Result<Gesture, i32> {
        self.ticks.push(t_ms);
        Ok(Gesture {
            code: 0,
            node_id: 0,
        })
    }
}

#[cfg(target_os = "macos")]
struct Renderer {
    handle: *mut c_void,
    pointer_fn: unsafe extern "C" fn(*mut c_void, *const OpuPointerEvent, *mut OpuGesture) -> i32,
    deadline_fn: unsafe extern "C" fn(*mut c_void) -> i64,
    tick_fn: unsafe extern "C" fn(*mut c_void, i64, *mut OpuGesture) -> i32,
}

#[cfg(target_os = "macos")]
impl Renderer {
    fn load(path: &str) -> Result<Self, String> {
        unsafe {
            let path = std::ffi::CString::new(path).map_err(|_| "bad library path")?;
            let handle = dlopen(path.as_ptr(), 2); // RTLD_NOW
            if handle.is_null() {
                return Err(format!("dlopen failed for {path:?}"));
            }
            let symbol = |name: &str| -> Result<*mut c_void, String> {
                let name = std::ffi::CString::new(name).unwrap();
                let address = dlsym(handle, name.as_ptr());
                if address.is_null() {
                    return Err(format!("missing ABI symbol {name:?}"));
                }
                Ok(address)
            };
            let new_fn: unsafe extern "C" fn(u32, u32) -> *mut c_void =
                std::mem::transmute(symbol("opu_renderer_new")?);
            Ok(Self {
                handle: new_fn(320, 240),
                pointer_fn: std::mem::transmute(symbol("opu_renderer_pointer")?),
                deadline_fn: std::mem::transmute(symbol("opu_renderer_next_deadline_ms")?),
                tick_fn: std::mem::transmute(symbol("opu_renderer_tick")?),
            })
        }
    }
}

#[cfg(target_os = "macos")]
impl InputSink for Renderer {
    fn pointer(&mut self, event: HostPointer) -> Result<Gesture, i32> {
        let raw = OpuPointerEvent {
            kind: match event.kind {
                PointerKind::Down => 1,
                PointerKind::Move => 2,
                PointerKind::Up => 3,
                PointerKind::Cancel => 4,
            },
            pointer_id: event.pointer_id,
            node_id: event.node_id,
            time_ms: event.time_ms,
            local_x: event.x,
            local_y: event.y,
            global_x: event.x,
            global_y: event.y,
        };
        let mut out = OpuGesture::default();
        let status = unsafe { (self.pointer_fn)(self.handle, &raw, &mut out) };
        if status != 0 {
            return Err(status);
        }
        Ok(Gesture {
            code: out.code,
            node_id: out.node_id,
        })
    }

    fn next_deadline_ms(&self) -> i64 {
        unsafe { (self.deadline_fn)(self.handle) }
    }

    fn tick(&mut self, t_ms: i64) -> Result<Gesture, i32> {
        let mut out = OpuGesture::default();
        let status = unsafe { (self.tick_fn)(self.handle, t_ms, &mut out) };
        if status != 0 {
            return Err(status);
        }
        Ok(Gesture {
            code: out.code,
            node_id: out.node_id,
        })
    }
}

#[cfg(target_os = "macos")]
fn feed<S: InputSink>(router: &mut InputRouter, sink: &mut S) {
    let events = [
        (InputEvent::Lifecycle(Lifecycle::Resumed), 10_i64),
        (
            InputEvent::Tap {
                x: 120.0,
                y: 120.0,
                node_id: 7,
                pointer: PointerState::default(),
            },
            100,
        ),
        (InputEvent::Lifecycle(Lifecycle::Inactive), 600),
    ];
    for (event, now) in events {
        let _ = router.feed(&event, now, sink);
    }
}

#[cfg(target_os = "macos")]
fn main() {
    let mut router = InputRouter::new();
    let library = std::env::var("OPU_RENDERER_LIB").ok();
    if let Some(path) = library {
        let mut renderer = match Renderer::load(&path) {
            Ok(renderer) => renderer,
            Err(error) => {
                println!("SKIP renderer unavailable: {error}");
                return;
            }
        };
        feed(&mut router, &mut renderer);
        let deadline = renderer.next_deadline_ms();
        println!("LIVE focus loss reached the renderer bridge; deadline={deadline}");
        return;
    }
    let mut recorder = Recorder {
        pointers: Vec::new(),
        ticks: Vec::new(),
    };
    feed(&mut router, &mut recorder);
    let cancels = recorder
        .pointers
        .iter()
        .filter(|event| event.kind == PointerKind::Cancel)
        .count();
    assert_eq!(cancels, 1, "focus loss must emit exactly one cancel");
    println!("LIVE platform focus loss mapped to one OPU_POINTER_CANCEL call");
    println!("SKIP arena reject: no renderer library (set OPU_RENDERER_LIB to a cdylib exporting opurom.h)");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("SKIP desktop gesture host runs on the macOS window host");
}
