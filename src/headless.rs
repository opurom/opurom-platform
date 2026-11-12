use std::collections::VecDeque;

use crate::{
    AdapterError, Capabilities, FrameView, InputEvent, PlatformAdapter, Size, HEADLESS_SPEC,
};

/// Fully implemented v0.1 reference adapter and conformance oracle.
/// No windowing dependency; runs on any OS.
#[derive(Debug)]
pub struct Headless {
    viewport: Size,
    frame_size: Size,
    events: VecDeque<InputEvent>,
    pixels: Vec<u8>,
    digest: String,
}

impl Headless {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            viewport: Size { width, height },
            frame_size: Size {
                width: 0,
                height: 0,
            },
            events: VecDeque::new(),
            pixels: Vec::new(),
            digest: hex(fnv1a(&[])),
        }
    }

    /// Queue an input event; `poll_event` drains the queue FIFO.
    pub fn push_event(&mut self, event: InputEvent) {
        self.events.push_back(event);
    }

    /// Update the viewport and enqueue a `Resize` event.
    pub fn resize(&mut self, size: Size) {
        self.viewport = size;
        self.events.push_back(InputEvent::Resize { size });
    }

    /// The last presented frame, borrowed from the internal buffer.
    pub fn frame(&self) -> FrameView<'_> {
        FrameView {
            width: self.frame_size.width,
            height: self.frame_size.height,
            pixels: &self.pixels,
        }
    }

    /// 16-char lowercase hex FNV-1a digest of the presented RGBA bytes.
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

impl PlatformAdapter for Headless {
    fn name(&self) -> &str {
        HEADLESS_SPEC.name
    }

    fn capabilities(&self) -> Capabilities {
        HEADLESS_SPEC.capabilities
    }

    fn viewport(&self) -> Size {
        self.viewport
    }

    fn poll_event(&mut self) -> Option<InputEvent> {
        self.events.pop_front()
    }

    fn present(&mut self, frame: FrameView<'_>) -> Result<(), AdapterError> {
        let expected = (frame.width as usize)
            .checked_mul(frame.height as usize)
            .and_then(|count| count.checked_mul(4))
            .ok_or(AdapterError::InvalidFrame)?;
        if frame.pixels.len() != expected {
            return Err(AdapterError::InvalidFrame);
        }
        self.pixels.clear();
        self.pixels.extend_from_slice(frame.pixels);
        self.frame_size = Size {
            width: frame.width,
            height: frame.height,
        };
        self.digest = hex(fnv1a(&self.pixels));
        Ok(())
    }
}

// shortcut: FNV-1a 64 is a deterministic checksum, not a cryptographic hash;
// upgrade to SHA-256 when golden hashes need collision resistance.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn hex(hash: u64) -> String {
    format!("{hash:016x}")
}
