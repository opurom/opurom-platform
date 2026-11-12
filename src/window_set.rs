//! Deterministic 2..4 viewport state shared by the R5 multi-window host.

use crate::{AdapterError, FrameView, Headless, InputEvent, PlatformAdapter, Size};

const INVALID_WINDOW_INDEX: &str = "invalid window index";
const INVALID_WINDOW_COUNT: &str = "WindowSet requires 2..4 viewports";

/// Independent viewports, each backed by its own `Headless` event FIFO and
/// RGBA8 frame buffer.
#[derive(Debug)]
pub struct WindowSet {
    windows: Vec<Headless>,
}

impl WindowSet {
    pub fn new<const N: usize>(sizes: [Size; N]) -> Result<Self, AdapterError> {
        if !(2..=4).contains(&N) {
            return Err(AdapterError::Backend(INVALID_WINDOW_COUNT));
        }
        Ok(Self {
            windows: sizes
                .into_iter()
                .map(|size| Headless::new(size.width, size.height))
                .collect(),
        })
    }

    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn viewport(&self, index: usize) -> Result<Size, AdapterError> {
        Ok(self.window(index)?.viewport())
    }

    pub fn resize(&mut self, index: usize, size: Size) -> Result<(), AdapterError> {
        self.window_mut(index)?.resize(size);
        Ok(())
    }

    pub fn push_event(&mut self, index: usize, event: InputEvent) -> Result<(), AdapterError> {
        self.window_mut(index)?.push_event(event);
        Ok(())
    }

    pub fn poll_event(&mut self, index: usize) -> Result<Option<InputEvent>, AdapterError> {
        Ok(self.window_mut(index)?.poll_event())
    }

    pub fn present(&mut self, index: usize, frame: FrameView<'_>) -> Result<(), AdapterError> {
        self.window_mut(index)?.present(frame)
    }

    pub fn frame(&self, index: usize) -> Result<FrameView<'_>, AdapterError> {
        Ok(self.window(index)?.frame())
    }

    fn window(&self, index: usize) -> Result<&Headless, AdapterError> {
        self.windows
            .get(index)
            .ok_or(AdapterError::Backend(INVALID_WINDOW_INDEX))
    }

    fn window_mut(&mut self, index: usize) -> Result<&mut Headless, AdapterError> {
        self.windows
            .get_mut(index)
            .ok_or(AdapterError::Backend(INVALID_WINDOW_INDEX))
    }
}
