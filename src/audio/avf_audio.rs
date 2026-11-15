//! The macOS audio adapter: a real `AVAudioPlayer` bound to the shared
//! `Transport`. Loading decodes the file and reads its real duration; play,
//! pause, seek and volume are forwarded to the player and the transport state
//! is kept in step. `AVAudioPlayer` and `NSURL` are usable off the main
//! thread, so this adapter needs no main-thread marker.
//!
//! There is no fabricated playback: with no real audio file the load fails
//! with a typed [`AudioError::Load`] and the transport stays `NoTrack`.

use std::path::Path;

use objc2::rc::Retained;
use objc2::AllocAnyThread;
use objc2_avf_audio::AVAudioPlayer;
use objc2_foundation::{NSString, NSURL};

use super::{AudioError, AudioEvent, AudioHost, PlaybackState, Transport};

/// The live AVAudioPlayer audio host.
#[derive(Default)]
pub struct AVAudioHost {
    transport: Transport,
    player: Option<Retained<AVAudioPlayer>>,
    events: std::collections::VecDeque<AudioEvent>,
}

impl AVAudioHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode `path` into an `AVAudioPlayer`, load `index` into the transport
    /// using the file's real duration. A decode failure is `AudioError::Load`.
    pub fn load_file(&mut self, index: usize, path: &Path) -> Result<(), AudioError> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let player =
            unsafe { AVAudioPlayer::initWithContentsOfURL_error(AVAudioPlayer::alloc(), &url) }
                .map_err(|_| AudioError::Load("AVAudioPlayer could not decode the file"))?;
        unsafe { player.prepareToPlay() };
        let duration_ms = (unsafe { player.duration() } * 1000.0).max(0.0) as u64;
        self.transport.load(index, duration_ms);
        self.player = Some(player);
        self.events.push_back(AudioEvent::Loaded { duration_ms });
        Ok(())
    }
}

impl AudioHost for AVAudioHost {
    fn transport(&self) -> &Transport {
        &self.transport
    }

    fn transport_mut(&mut self) -> &mut Transport {
        &mut self.transport
    }

    fn sync(&mut self) -> Result<(), AudioError> {
        let Some(player) = &self.player else {
            return Err(AudioError::NoTrack);
        };
        match self.transport.state() {
            PlaybackState::Playing => {
                if !unsafe { player.isPlaying() } && !unsafe { player.play() } {
                    return Err(AudioError::Backend("AVAudioPlayer.play() returned false"));
                }
            }
            PlaybackState::Paused | PlaybackState::Stopped => {
                if unsafe { player.isPlaying() } {
                    unsafe { player.pause() };
                }
            }
        }
        let seconds = self.transport.position_ms() as f64 / 1000.0;
        unsafe {
            player.setCurrentTime(seconds);
            player.setVolume(self.transport.volume() as f32 / 100.0);
        }
        Ok(())
    }

    fn poll(&mut self) -> Option<AudioEvent> {
        self.events.pop_front()
    }
}
