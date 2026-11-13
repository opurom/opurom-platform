//! Runtime bridge from the live AppKit layer to the GPU crate's Metal presentation cdylib.
//!
//! The platform binary never links Metal or the GPU crate. It resolves the
//! `opr_present_display_list` symbol with `dlopen`/`dlsym`, exactly as the X11 adapter
//! resolves Xlib, and maps every failure to an honest verdict.

use std::env;
use std::ffi::c_void;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
use std::ffi::{c_char, c_int, CStr, CString};
#[cfg(target_os = "macos")]
use std::mem;
#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;

pub const GPU_DYLIB_ENV: &str = "OPUROM_GPU_DYLIB";

// Status values are the C ABI contract with `opurom-gpu/src/presentation.rs`.
pub const PRESENT_OK: i32 = 0;
pub const PRESENT_METAL_UNAVAILABLE: i32 = 10;
pub const PRESENT_NO_DRAWABLE: i32 = 11;
pub const PRESENT_ERROR: i32 = 12;
pub const PRESENT_INVALID_ARGUMENT: i32 = 13;
pub const PRESENT_UNSUPPORTED: i32 = 14;

/// Absolute umbrella root, independent of the caller's working directory.
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Path to the runtime-loaded GPU presentation library.
pub fn gpu_library_path() -> PathBuf {
    if let Some(path) = env::var_os(GPU_DYLIB_ENV) {
        return PathBuf::from(path);
    }
    workspace_root().join("opurom-gpu/target/debug/libopurom_gpu.dylib")
}

/// Result of one FFI presentation call; never a fabricated PASS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresentationOutcome {
    Presented,
    Skipped(String),
    Failed(String),
}

/// Classify a GPU status with its concrete reason.
pub fn classify(status: i32, reason: &str) -> PresentationOutcome {
    if status == PRESENT_OK {
        return if reason.is_empty() {
            PresentationOutcome::Presented
        } else {
            PresentationOutcome::Failed(format!(
                "GPU presentation reported success with a reason: {reason}"
            ))
        };
    }
    let detail = if reason.is_empty() {
        format!("GPU presentation returned status {status}")
    } else {
        reason.to_owned()
    };
    if matches!(
        status,
        PRESENT_METAL_UNAVAILABLE | PRESENT_NO_DRAWABLE | PRESENT_UNSUPPORTED
    ) {
        PresentationOutcome::Skipped(detail)
    } else {
        PresentationOutcome::Failed(detail)
    }
}

#[cfg(target_os = "macos")]
const RTLD_NOW: c_int = 0x2;
#[cfg(target_os = "macos")]
const RTLD_LOCAL: c_int = 0x4;

#[cfg(target_os = "macos")]
extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

#[cfg(target_os = "macos")]
type PresentFn = unsafe extern "C" fn(*const c_char, *mut c_void, *mut c_char, usize) -> c_int;

#[cfg(target_os = "macos")]
struct GpuLibrary {
    handle: *mut c_void,
}

#[cfg(target_os = "macos")]
impl GpuLibrary {
    fn load(path: &Path) -> Result<Self, String> {
        let text = path.to_string_lossy();
        let c_path = CString::new(text.as_bytes())
            .map_err(|_| format!("the GPU dylib path {text} contains a NUL byte"))?;
        // SAFETY: `c_path` is NUL-terminated and owned for the whole call.
        let handle = unsafe { dlopen(c_path.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
        if handle.is_null() {
            return Err(format!(
                "dlopen({text}) failed; the Metal presentation dylib is unavailable"
            ));
        }
        Ok(Self { handle })
    }

    fn present_symbol(&self) -> Result<PresentFn, String> {
        // SAFETY: `handle` came from a successful dlopen and the symbol name
        // is NUL-terminated. A non-null symbol has the declared C signature.
        let symbol = unsafe {
            dlsym(
                self.handle,
                b"opr_present_display_list\0".as_ptr().cast::<c_char>(),
            )
        };
        if symbol.is_null() {
            return Err(
                "dlsym(opr_present_display_list) failed; the GPU dylib is missing the Metal presentation entry point"
                    .to_owned(),
            );
        }
        Ok(unsafe { mem::transmute::<*mut c_void, PresentFn>(symbol) })
    }
}

#[cfg(target_os = "macos")]
impl Drop for GpuLibrary {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `load` and is closed exactly once.
        unsafe {
            dlclose(self.handle);
        }
    }
}

#[cfg(target_os = "macos")]
fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build the dylib on first use so the CLI works from the umbrella root.
#[cfg(target_os = "macos")]
fn ensure_gpu_library() -> Result<PathBuf, String> {
    let path = gpu_library_path();
    if path.is_file() {
        return Ok(path);
    }
    if env::var_os(GPU_DYLIB_ENV).is_some() {
        return Err(format!(
            "the GPU dylib was not found at {} ({GPU_DYLIB_ENV})",
            path.display()
        ));
    }
    let manifest = workspace_root().join("opurom-gpu/Cargo.toml");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .args(["build", "--offline", "--manifest-path"])
        .arg(&manifest)
        .arg("--lib")
        .output()
        .map_err(|error| {
            format!(
                "the GPU dylib was not found at {} and cargo build failed to start: {error}",
                path.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "the GPU dylib was not found at {} and cargo build failed: {}",
            path.display(),
            one_line(&String::from_utf8_lossy(&output.stderr))
        ));
    }
    if !path.is_file() {
        return Err(format!(
            "cargo build succeeded but no dylib was produced at {}",
            path.display()
        ));
    }
    Ok(path)
}

/// Present one display list through the live layer using the runtime library.
#[cfg(target_os = "macos")]
pub fn present_display_list(json: &str, layer: *mut c_void) -> PresentationOutcome {
    let path = match ensure_gpu_library() {
        Ok(path) => path,
        Err(reason) => return PresentationOutcome::Skipped(reason),
    };
    let library = match GpuLibrary::load(&path) {
        Ok(library) => library,
        Err(reason) => return PresentationOutcome::Skipped(reason),
    };
    let present = match library.present_symbol() {
        Ok(present) => present,
        Err(reason) => return PresentationOutcome::Skipped(reason),
    };
    if layer.is_null() {
        return PresentationOutcome::Failed("the live window has no CAMetalLayer".to_owned());
    }
    let json = match CString::new(json) {
        Ok(json) => json,
        Err(_) => {
            return PresentationOutcome::Failed(
                "the generated display list contains a NUL byte".to_owned(),
            )
        }
    };
    let mut reason = [0 as c_char; 512];
    // SAFETY: the layer is retained by the caller, `json` is NUL-terminated,
    // and `reason` owns 512 writable bytes. The symbol was loaded from the
    // matching GPU cdylib.
    let status = unsafe { present(json.as_ptr(), layer, reason.as_mut_ptr(), reason.len()) };
    let reason = unsafe { CStr::from_ptr(reason.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    classify(status as i32, &reason)
}

/// Non-macOS hosts cannot present a Metal drawable and report a concrete SKIP.
#[cfg(not(target_os = "macos"))]
pub fn present_display_list(_json: &str, _layer: *mut c_void) -> PresentationOutcome {
    PresentationOutcome::Skipped("GPU presentation is macOS/Metal only".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_requires_status_zero_and_no_error_reason() {
        assert_eq!(classify(PRESENT_OK, ""), PresentationOutcome::Presented);
        assert!(matches!(
            classify(PRESENT_OK, "unexpected reason"),
            PresentationOutcome::Failed(_)
        ));
    }

    #[test]
    fn a_missing_drawable_is_a_concrete_skip() {
        let reason = "CAMetalLayer nextDrawable() returned nil; no on-screen drawable is available";
        assert_eq!(
            classify(PRESENT_NO_DRAWABLE, reason),
            PresentationOutcome::Skipped(reason.to_owned())
        );
        assert_eq!(
            classify(
                PRESENT_METAL_UNAVAILABLE,
                "MTLCreateSystemDefaultDevice() returned nil"
            ),
            PresentationOutcome::Skipped("MTLCreateSystemDefaultDevice() returned nil".to_owned())
        );
    }

    #[test]
    fn a_metal_error_is_never_a_pass_or_skip() {
        assert_eq!(
            classify(PRESENT_ERROR, "Metal error: bad pipeline"),
            PresentationOutcome::Failed("Metal error: bad pipeline".to_owned())
        );
        assert_eq!(
            classify(PRESENT_INVALID_ARGUMENT, "the CAMetalLayer pointer is null"),
            PresentationOutcome::Failed("the CAMetalLayer pointer is null".to_owned())
        );
    }

    #[test]
    fn an_unknown_status_still_has_a_concrete_reason() {
        assert_eq!(
            classify(99, ""),
            PresentationOutcome::Failed("GPU presentation returned status 99".to_owned())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_null_layer_is_a_failure_not_a_pass() {
        assert_eq!(
            present_display_list("{}", std::ptr::null_mut()),
            PresentationOutcome::Failed("the live window has no CAMetalLayer".to_owned())
        );
    }
}
