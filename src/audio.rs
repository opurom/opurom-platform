//! The audio transport capability: playback, pause, seek, volume and repeat
//! mode, with one contract across every platform and a real per-platform
//! implementation where the OS provides one.
//!
//! The transport is a pure state machine — [`Transport`] — that every adapter
//! shares, so the observable playback state (position, volume, play mode) is
//! identical on every platform and fully deterministic in tests. An adapter
//! only binds that state to a real audio device.
//!
//! `HeadlessAudio` is the deterministic reference: it advances position from
//! an injected tick, with no wall clock and no device. The macOS adapter drives
//! a real `AVAudioPlayer`. Platforms with no implemented audio backend return
//! the typed [`AudioError::Unavailable`] reason and the caller keeps the pure
//! `Transport` (the documented degradation path), so the UI still shows and
//! controls state even where sound cannot play.

use std::fmt;
use std::time::Duration;

/// Playback status. `Paused` keeps the position; `Stopped` rewinds to 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

/// Repeat mode. `One` repeats the current track; `All` wraps the queue;
/// `Off` stops at the queue end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    One,
    All,
}

impl RepeatMode {
    /// Cycle Off -> All -> One -> Off, the order a single repeat button uses.
    pub fn cycle(self) -> RepeatMode {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RepeatMode::Off => "off",
            RepeatMode::One => "one",
            RepeatMode::All => "all",
        }
    }
}

/// Typed audio failures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    Unavailable(&'static str),
    InvalidPosition,
    InvalidVolume,
    NoTrack,
    Load(&'static str),
    Backend(&'static str),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::Unavailable(why) => write!(f, "audio unavailable: {why}"),
            AudioError::InvalidPosition => write!(f, "seek target is outside the track"),
            AudioError::InvalidVolume => write!(f, "volume must be within 0..=100"),
            AudioError::NoTrack => write!(f, "no track loaded"),
            AudioError::Load(why) => write!(f, "audio load failed: {why}"),
            AudioError::Backend(why) => write!(f, "audio backend error: {why}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// The pure, platform-shared playback state machine. Every field is a plain
/// value, so the state is copyable into a test and compared exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transport {
    state: PlaybackState,
    position_ms: u64,
    duration_ms: u64,
    volume: u8,
    repeat: RepeatMode,
    track_index: Option<usize>,
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            state: PlaybackState::Stopped,
            position_ms: 0,
            duration_ms: 0,
            volume: 80,
            repeat: RepeatMode::Off,
            track_index: None,
        }
    }
}

impl Transport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> PlaybackState {
        self.state
    }

    pub fn position_ms(&self) -> u64 {
        self.position_ms
    }

    pub fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    pub fn volume(&self) -> u8 {
        self.volume
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    pub fn track_index(&self) -> Option<usize> {
        self.track_index
    }

    /// Load a track by queue `index` with a known duration, paused at zero.
    pub fn load(&mut self, index: usize, duration_ms: u64) {
        self.track_index = Some(index);
        self.duration_ms = duration_ms;
        self.position_ms = 0;
        self.state = PlaybackState::Paused;
    }

    /// Start or resume playback. A stopped transport with no track is
    /// `NoTrack`; a track that reached its end restarts from zero.
    pub fn play(&mut self) -> Result<(), AudioError> {
        if self.track_index.is_none() {
            return Err(AudioError::NoTrack);
        }
        if self.position_ms >= self.duration_ms && self.duration_ms > 0 {
            self.position_ms = 0;
        }
        self.state = PlaybackState::Playing;
        Ok(())
    }

    pub fn pause(&mut self) {
        if self.state == PlaybackState::Playing {
            self.state = PlaybackState::Paused;
        }
    }

    /// Stop and rewind to zero, keeping the loaded track.
    pub fn stop(&mut self) {
        self.state = PlaybackState::Stopped;
        self.position_ms = 0;
    }

    /// Seek to an absolute millisecond position clamped to `[0, duration]`.
    /// A position beyond `duration + 1` second is an `InvalidPosition` rather
    /// than a silently clamped move, so a UI slider bug is visible.
    pub fn seek(&mut self, position_ms: u64) -> Result<(), AudioError> {
        if position_ms > self.duration_ms.saturating_add(1000) {
            return Err(AudioError::InvalidPosition);
        }
        self.position_ms = position_ms.min(self.duration_ms);
        Ok(())
    }

    /// Set volume as a whole percent. Values above 100 are rejected.
    pub fn set_volume(&mut self, percent: u8) -> Result<(), AudioError> {
        if percent > 100 {
            return Err(AudioError::InvalidVolume);
        }
        self.volume = percent;
        Ok(())
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.repeat = repeat;
    }

    pub fn cycle_repeat(&mut self) -> RepeatMode {
        self.repeat = self.repeat.cycle();
        self.repeat
    }

    /// Advance the playhead by `elapsed` while playing. Returns the queue
    /// action the caller must take at the end of the track: `RepeatCurrent`
    /// when `RepeatMode::One`, `Advance` for `All`/`Off` at the natural end,
    /// and `None` while still mid-track. `Off` at the end of the queue is the
    /// caller's `Advance` decision to make (it knows the queue length).
    pub fn tick(&mut self, elapsed: Duration) -> TickOutcome {
        if self.state != PlaybackState::Playing || self.duration_ms == 0 {
            return TickOutcome::None;
        }
        let advance = elapsed.as_millis() as u64;
        self.position_ms = self.position_ms.saturating_add(advance);
        if self.position_ms < self.duration_ms {
            return TickOutcome::None;
        }
        self.position_ms = self.duration_ms;
        self.state = PlaybackState::Paused;
        match self.repeat {
            RepeatMode::One => TickOutcome::RepeatCurrent,
            RepeatMode::All => TickOutcome::Advance,
            RepeatMode::Off => TickOutcome::TrackEnded,
        }
    }
}

/// What `Transport::tick` decided when the playhead reached the track end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickOutcome {
    None,
    RepeatCurrent,
    Advance,
    TrackEnded,
}

/// The one audio ABI. `transport` exposes the shared state so a caller can
/// drive the UI from it directly; `poll` drains backend events (e.g. a natural
/// end the device reported) that the pure tick could not see.
pub trait AudioHost {
    fn transport(&self) -> &Transport;
    /// Mutable state access for the caller-facing transport operations.
    fn transport_mut(&mut self) -> &mut Transport;
    /// Push the current transport state to the device (position/volume/state).
    fn sync(&mut self) -> Result<(), AudioError>;
    fn poll(&mut self) -> Option<AudioEvent>;
}

/// Backend events a device reports that the pure transport cannot infer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioEvent {
    Loaded { duration_ms: u64 },
    Ended,
    Error(&'static str),
}

/// The deterministic reference audio host: pure state plus a device-poll queue,
/// with no wall clock and no output device.
#[derive(Default)]
pub struct HeadlessAudio {
    transport: Transport,
    events: std::collections::VecDeque<AudioEvent>,
}

impl HeadlessAudio {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a backend event as if the device had reported it.
    pub fn deliver(&mut self, event: AudioEvent) {
        self.events.push_back(event);
    }
}

impl AudioHost for HeadlessAudio {
    fn transport(&self) -> &Transport {
        &self.transport
    }

    fn transport_mut(&mut self) -> &mut Transport {
        &mut self.transport
    }

    fn sync(&mut self) -> Result<(), AudioError> {
        Ok(())
    }

    fn poll(&mut self) -> Option<AudioEvent> {
        self.events.pop_front()
    }
}

#[cfg(target_os = "macos")]
mod avf_audio;
#[cfg(target_os = "macos")]
pub use avf_audio::AVAudioHost;

/// The running platform's real audio adapter, or the typed reason it has
/// none. Linux/Windows have no implemented backend in 0.1.0, so the caller
/// keeps a pure `Transport` (the documented degradation path).
pub fn platform_audio() -> Result<Box<dyn AudioHost>, AudioError> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(AVAudioHost::new()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(AudioError::Unavailable(
            "no audio backend implemented on this platform in 0.1.0",
        ))
    }
}
