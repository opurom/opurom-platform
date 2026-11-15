//! The system tray capability: a platform-neutral menu contract with one
//! adapter per desktop platform, exposed through the capability registry
//! (`desktop::desktop_support`). The menu model and the event stream are
//! identical on every platform, so one application sees one tray behavior.
//!
//! `HeadlessTray` is the deterministic reference: events are delivered only
//! through its own queue, with no wall clock and no OS surface. The macOS
//! adapter drives a real `NSStatusItem`; the Windows adapter drives a real
//! `Shell_NotifyIconW` message window. On platforms whose tray protocol is
//! not implemented in 0.1.0, [`platform_tray`] returns a typed
//! [`TrayError::Unavailable`] reason instead of a fabricated surface.

use std::fmt;

/// One entry in a tray menu. `id` is the application-assigned event key and
/// `quit_id` marks the one item that asks the host to exit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayItem {
    pub id: u32,
    pub label: String,
    pub enabled: bool,
}

impl TrayItem {
    pub fn new(id: u32, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            enabled: true,
        }
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// The whole tray menu. `title` labels the status item; `quit_id`, when
/// present, must match one item id and is reported as `QuitRequested`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrayMenu {
    pub title: String,
    pub items: Vec<TrayItem>,
    pub quit_id: Option<u32>,
}

impl TrayMenu {
    /// Structural validation shared by every adapter: ids are unique and
    /// nonzero, labels are non-empty, `quit_id` names a real item.
    pub fn validate(&self) -> Result<(), TrayError> {
        let mut seen = 0u64;
        for item in &self.items {
            if item.id == 0 {
                return Err(TrayError::InvalidMenu("item id 0 is reserved"));
            }
            if item.label.trim().is_empty() {
                return Err(TrayError::InvalidMenu("item label is empty"));
            }
            let bit = 1u64 << (item.id % 64);
            if seen & bit != 0 {
                return Err(TrayError::InvalidMenu("duplicate item id"));
            }
            seen |= bit;
        }
        if let Some(quit) = self.quit_id {
            if !self.items.iter().any(|item| item.id == quit) {
                return Err(TrayError::InvalidMenu("quit_id names no item"));
            }
        }
        Ok(())
    }
}

/// Events a tray host delivers, in FIFO order. `ItemClicked` carries the
/// item id; `QuitRequested` is a click on the item named by `quit_id`, kept
/// distinct so the application's quit path is not a special-cased item body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    ItemClicked(u32),
    QuitRequested(u32),
}

/// Typed tray failures. `Unavailable` always carries the concrete reason the
/// platform has no tray surface in this build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayError {
    Unavailable(&'static str),
    InvalidMenu(&'static str),
    NotMounted,
    AlreadyMounted,
    Backend(&'static str),
}

impl fmt::Display for TrayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrayError::Unavailable(why) => write!(f, "tray unavailable: {why}"),
            TrayError::InvalidMenu(why) => write!(f, "invalid tray menu: {why}"),
            TrayError::NotMounted => write!(f, "tray not mounted"),
            TrayError::AlreadyMounted => write!(f, "tray already mounted"),
            TrayError::Backend(why) => write!(f, "tray backend error: {why}"),
        }
    }
}

impl std::error::Error for TrayError {}

/// The one tray ABI. A host mounts one tray surface at a time; `poll_event`
/// drains clicks FIFO and never blocks.
pub trait TrayHost {
    fn mount(&mut self, menu: TrayMenu) -> Result<(), TrayError>;
    fn update(&mut self, menu: TrayMenu) -> Result<(), TrayError>;
    fn poll_event(&mut self) -> Option<TrayEvent>;
    fn unmount(&mut self) -> Result<(), TrayError>;
    fn is_mounted(&self) -> bool;
}

/// The deterministic reference tray: no OS surface, events only through
/// `deliver`, so tests drive the exact stream the real adapters promise.
#[derive(Default)]
pub struct HeadlessTray {
    menu: Option<TrayMenu>,
    events: std::collections::VecDeque<TrayEvent>,
}

impl HeadlessTray {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a click as if the user had picked the item. IDs outside the
    /// mounted menu are dropped: a real adapter cannot click a missing item.
    pub fn deliver(&mut self, id: u32) {
        let Some(menu) = &self.menu else {
            return;
        };
        if menu.items.iter().any(|item| item.id == id) {
            let event = if menu.quit_id == Some(id) {
                TrayEvent::QuitRequested(id)
            } else {
                TrayEvent::ItemClicked(id)
            };
            self.events.push_back(event);
        }
    }

    /// Draining helper for tests: every queued event, FIFO.
    pub fn drain(&mut self) -> Vec<TrayEvent> {
        self.events.drain(..).collect()
    }
}

impl TrayHost for HeadlessTray {
    fn mount(&mut self, menu: TrayMenu) -> Result<(), TrayError> {
        if self.menu.is_some() {
            return Err(TrayError::AlreadyMounted);
        }
        menu.validate()?;
        self.menu = Some(menu);
        Ok(())
    }

    fn update(&mut self, menu: TrayMenu) -> Result<(), TrayError> {
        if self.menu.is_none() {
            return Err(TrayError::NotMounted);
        }
        menu.validate()?;
        self.menu = Some(menu);
        Ok(())
    }

    fn poll_event(&mut self) -> Option<TrayEvent> {
        self.events.pop_front()
    }

    fn unmount(&mut self) -> Result<(), TrayError> {
        self.menu.take().map(|_| ()).ok_or(TrayError::NotMounted)
    }

    fn is_mounted(&self) -> bool {
        self.menu.is_some()
    }
}

#[cfg(target_os = "macos")]
mod appkit_tray;
#[cfg(target_os = "macos")]
pub use appkit_tray::AppKitTray;

#[cfg(target_os = "windows")]
mod win32_tray;
#[cfg(target_os = "windows")]
pub use win32_tray::Win32Tray;

/// The running platform's real tray adapter, or the typed reason it has
/// none. Linux and Wayland have no implemented tray protocol in 0.1.0, so
/// they degrade with an explicit reason instead of a silent no-op.
pub fn platform_tray() -> Result<Box<dyn TrayHost>, TrayError> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(AppKitTray::new()))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(Win32Tray::new()))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(TrayError::Unavailable(
            "no tray protocol implemented on this platform in 0.1.0",
        ))
    }
}
