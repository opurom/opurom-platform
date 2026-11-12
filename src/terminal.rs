//! Terminal host adapter (R3_ADDENDUM.md section 6): deterministic truecolor ANSI
//! half-block presentation of an RGBA frame. This is a thin host adapter that
//! demonstrates the platform boundary; it is NOT a Metal/Vulkan/ANGLE GPU backend.

use std::collections::VecDeque;
use std::fmt::Write as _;

use crate::{
    AdapterError, Capabilities, FrameView, InputEvent, PlatformAdapter, Size, TERMINAL_SPEC,
};

/// Default logical viewport (pixel columns x pixel rows) used by `with_defaults`.
const DEFAULT_WIDTH: u32 = 80;
const DEFAULT_HEIGHT: u32 = 24;

/// A fully implemented host adapter that turns a borrowed RGBA8 `FrameView` into a
/// deterministic truecolor ANSI string: one U+2580 upper-half-block character per
/// pixel column and two pixel rows per character row. No TTY, threads or wall clock;
/// `last_output()` exposes the exact bytes so host tests need no terminal.
#[derive(Debug)]
pub struct Terminal {
    viewport: Size,
    events: VecDeque<InputEvent>,
    output: String,
}

impl Terminal {
    /// Create an adapter whose `viewport()` reflects `width` pixel columns and
    /// `height` pixel rows (presented as `ceil(height / 2)` character rows).
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            viewport: Size { width, height },
            events: VecDeque::new(),
            output: String::new(),
        }
    }

    /// Create an adapter with the conventional 80x24 logical viewport.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_WIDTH, DEFAULT_HEIGHT)
    }

    /// Queue a parsed key event; `poll_event()` drains FIFO.
    pub fn push_key(&mut self, code: u32, pressed: bool) {
        self.events.push_back(InputEvent::Key { code, pressed });
    }

    /// Queue any normalized input event (`Key`/`Input`/`Tap`/...); drains FIFO.
    pub fn push_input(&mut self, event: InputEvent) {
        self.events.push_back(event);
    }

    /// Alias matching the headless oracle's `push_event` name.
    pub fn push_event(&mut self, event: InputEvent) {
        self.push_input(event);
    }

    /// The exact ANSI text produced by the last successful `present`, without a TTY.
    pub fn last_output(&self) -> &str {
        &self.output
    }

    /// Alias for `last_output()`.
    pub fn render_text(&self) -> &str {
        self.last_output()
    }
}

impl PlatformAdapter for Terminal {
    fn name(&self) -> &str {
        TERMINAL_SPEC.name
    }

    fn capabilities(&self) -> Capabilities {
        TERMINAL_SPEC.capabilities
    }

    fn viewport(&self) -> Size {
        self.viewport
    }

    fn poll_event(&mut self) -> Option<InputEvent> {
        self.events.pop_front()
    }

    fn present(&mut self, frame: FrameView<'_>) -> Result<(), AdapterError> {
        // Validate before touching state: a rejected frame leaves `output` intact.
        let expected = (frame.width as usize)
            .checked_mul(frame.height as usize)
            .and_then(|count| count.checked_mul(4))
            .ok_or(AdapterError::InvalidFrame)?;
        if frame.pixels.len() != expected {
            return Err(AdapterError::InvalidFrame);
        }
        self.output = render(&frame);
        Ok(())
    }
}

/// RGB of pixel (x, y); alpha is ignored because truecolor ANSI has no alpha channel.
fn rgb(frame: &FrameView<'_>, x: usize, y: usize) -> (u8, u8, u8) {
    let index = (y * frame.width as usize + x) * 4;
    (
        frame.pixels[index],
        frame.pixels[index + 1],
        frame.pixels[index + 2],
    )
}

/// Deterministic truecolor half-block text; pure function of the frame bytes.
fn render(frame: &FrameView<'_>) -> String {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let rows = height.div_ceil(2);
    let mut out = String::new();
    for row in 0..rows {
        let top = 2 * row;
        let bottom = top + 1;
        for x in 0..width {
            let (tr, tg, tb) = rgb(frame, x, top);
            // Odd heights pad the missing bottom row with transparent/black.
            let (br, bg, bb) = if bottom < height {
                rgb(frame, x, bottom)
            } else {
                (0, 0, 0)
            };
            // fg = top pixel, bg = bottom pixel, glyph = upper half block.
            write!(
                out,
                "\x1b[38;2;{tr};{tg};{tb}m\x1b[48;2;{br};{bg};{bb}m\u{2580}"
            )
            .expect("writing to a String never fails");
        }
        out.push_str("\x1b[0m\n"); // reset at line end
    }
    out
}
