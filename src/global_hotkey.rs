//! The global hotkey capability: register a chord, receive its trigger, and
//! unregister it, with one adapter per desktop platform behind one contract.
//!
//! The chord model is platform-neutral so the same registration means the
//! same thing everywhere. A [`Modifiers`] set is a bitmask of the four
//! standard modifiers; [`Hotkey`] pairs it with a [`Key`] (a named virtual
//! key, not a raw OS scan code). [`parse_chord`] reads the `"Ctrl+Alt+Space"`
//! form an application writes in its source.
//!
//! `HeadlessHotkey` is the deterministic reference. The macOS adapter installs
//! real Carbon `RegisterEventHotKey` handlers; the Windows adapter installs
//! `RegisterHotKey` on a dedicated message window. Platforms with no
//! implemented protocol return the typed [`HotkeyError::Unavailable`] reason.

use std::fmt;

/// The four standard modifier keys, as a bitmask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const SHIFT: Modifiers = Modifiers(1 << 0);
    pub const CTRL: Modifiers = Modifiers(1 << 1);
    pub const ALT: Modifiers = Modifiers(1 << 2);
    pub const META: Modifiers = Modifiers(1 << 3);

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn union(self, other: Modifiers) -> Modifiers {
        Modifiers(self.0 | other.0)
    }
}

/// A named virtual key. The small closed set covers the media and editing keys
/// a player binds; adding one is a contract change, not a per-platform choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Space,
    Left,
    Right,
    Up,
    Down,
    Enter,
    Escape,
    MediaPlayPause,
    MediaNext,
    MediaPrev,
}

impl Key {
    /// The stable name a chord uses, matching the parse table.
    pub fn as_str(self) -> &'static str {
        match self {
            Key::Space => "Space",
            Key::Left => "Left",
            Key::Right => "Right",
            Key::Up => "Up",
            Key::Down => "Down",
            Key::Enter => "Enter",
            Key::Escape => "Escape",
            Key::MediaPlayPause => "MediaPlayPause",
            Key::MediaNext => "MediaNext",
            Key::MediaPrev => "MediaPrev",
        }
    }

    fn parse(name: &str) -> Option<Key> {
        // Both key and modifier names accept either case, so `ctrl+space` and
        // `Ctrl+Space` name the same chord.
        Some(match name.to_ascii_lowercase().as_str() {
            "space" => Key::Space,
            "left" => Key::Left,
            "right" => Key::Right,
            "up" => Key::Up,
            "down" => Key::Down,
            "enter" => Key::Enter,
            "escape" => Key::Escape,
            "mediaplaypause" => Key::MediaPlayPause,
            "medianext" => Key::MediaNext,
            "mediaprev" => Key::MediaPrev,
            _ => return None,
        })
    }
}

/// One global hotkey: a modifier set plus a virtual key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl Hotkey {
    pub const fn new(modifiers: Modifiers, key: Key) -> Self {
        Self { modifiers, key }
    }
}

/// Parse the `"Ctrl+Alt+Space"` chord form. The key names the last segment;
/// the modifiers precede it in any order. An empty modifier set is allowed,
/// but a chord with no key or an unknown segment is an error.
pub fn parse_chord(text: &str) -> Result<Hotkey, HotkeyError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(HotkeyError::InvalidChord("empty chord"));
    }
    let mut modifiers = Modifiers::NONE;
    let mut key: Option<Key> = None;
    for segment in trimmed.split('+') {
        let segment = segment.trim();
        let modifier = match segment.to_ascii_lowercase().as_str() {
            "shift" => Some(Modifiers::SHIFT),
            "ctrl" | "control" => Some(Modifiers::CTRL),
            "alt" | "option" => Some(Modifiers::ALT),
            "meta" | "cmd" | "win" => Some(Modifiers::META),
            _ => None,
        };
        if let Some(modifier) = modifier {
            if key.is_some() {
                return Err(HotkeyError::InvalidChord("modifier after the key segment"));
            }
            modifiers = modifiers.union(modifier);
            continue;
        }
        if key.is_some() {
            return Err(HotkeyError::InvalidChord("more than one key segment"));
        }
        key = Some(
            Key::parse(segment).ok_or(HotkeyError::InvalidChord("unknown key or modifier name"))?,
        );
    }
    let key = key.ok_or(HotkeyError::InvalidChord("missing key segment"))?;
    Ok(Hotkey { modifiers, key })
}

/// Typed hotkey failures. `Unavailable` carries the concrete platform reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyError {
    Unavailable(&'static str),
    InvalidChord(&'static str),
    AlreadyRegistered(u32),
    NotRegistered(u32),
    Backend(&'static str),
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HotkeyError::Unavailable(why) => write!(f, "global hotkey unavailable: {why}"),
            HotkeyError::InvalidChord(why) => write!(f, "invalid chord: {why}"),
            HotkeyError::AlreadyRegistered(id) => write!(f, "hotkey {id} already registered"),
            HotkeyError::NotRegistered(id) => write!(f, "hotkey {id} not registered"),
            HotkeyError::Backend(why) => write!(f, "hotkey backend error: {why}"),
        }
    }
}

impl std::error::Error for HotkeyError {}

/// The one hotkey ABI. An application registers each chord under a stable
/// `id`; a trigger is reported as `Triggered(id)`, so the handler keyed by that
/// id runs. `poll_trigger` drains FIFO and never blocks.
pub trait HotkeyHost {
    fn register(&mut self, id: u32, hotkey: Hotkey) -> Result<(), HotkeyError>;
    fn unregister(&mut self, id: u32) -> Result<(), HotkeyError>;
    fn poll_trigger(&mut self) -> Option<u32>;
    fn registered(&self) -> Vec<u32>;
}

/// The deterministic reference hotkey host: registration bookkeeping only,
/// with `trigger` delivering the exact event stream the real adapters promise.
#[derive(Default)]
pub struct HeadlessHotkey {
    registrations: Vec<(u32, Hotkey)>,
    triggers: std::collections::VecDeque<u32>,
}

impl HeadlessHotkey {
    pub fn new() -> Self {
        Self::default()
    }

    /// Deliver a trigger for a registered id. An unknown id is ignored, as a
    /// real OS handler only fires for a hotkey it actually registered.
    pub fn trigger(&mut self, id: u32) {
        if self.registrations.iter().any(|(key, _)| *key == id) {
            self.triggers.push_back(id);
        }
    }
}

impl HotkeyHost for HeadlessHotkey {
    fn register(&mut self, id: u32, hotkey: Hotkey) -> Result<(), HotkeyError> {
        if id == 0 {
            return Err(HotkeyError::Backend("hotkey id 0 is reserved"));
        }
        if self.registrations.iter().any(|(key, _)| *key == id) {
            return Err(HotkeyError::AlreadyRegistered(id));
        }
        self.registrations.push((id, hotkey));
        Ok(())
    }

    fn unregister(&mut self, id: u32) -> Result<(), HotkeyError> {
        let before = self.registrations.len();
        self.registrations.retain(|(key, _)| *key != id);
        if self.registrations.len() == before {
            return Err(HotkeyError::NotRegistered(id));
        }
        Ok(())
    }

    fn poll_trigger(&mut self) -> Option<u32> {
        self.triggers.pop_front()
    }

    fn registered(&self) -> Vec<u32> {
        self.registrations.iter().map(|(id, _)| *id).collect()
    }
}

#[cfg(target_os = "macos")]
mod carbon_hotkey;
#[cfg(target_os = "macos")]
pub use carbon_hotkey::CarbonHotkey;

#[cfg(target_os = "windows")]
mod win32_hotkey;
#[cfg(target_os = "windows")]
pub use win32_hotkey::Win32Hotkey;

/// The running platform's real hotkey adapter, or the typed reason it has
/// none. Linux has no implemented global-hotkey protocol in 0.1.0.
pub fn platform_hotkey() -> Result<Box<dyn HotkeyHost>, HotkeyError> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(CarbonHotkey::new()))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(Win32Hotkey::new()))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(HotkeyError::Unavailable(
            "no global hotkey protocol implemented on this platform in 0.1.0",
        ))
    }
}
