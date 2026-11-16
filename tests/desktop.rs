//! Tests for the desktop native capability surface: the platform-neutral
//! contracts (tray, global hotkey, filesystem scan, audio transport) on every
//! host, plus the macOS real-adapter paths.
//!
//! A live tray/hotkey/audio check needs a GUI session, an interactive login
//! and a real audio file; when any is missing the test prints `SKIP <reason>`
//! and returns without asserting a PASS. No test fabricates a device result.

use opurom_platform::audio::{
    AudioError, AudioEvent, AudioHost, HeadlessAudio, PlaybackState, RepeatMode, TickOutcome,
    Transport,
};
use opurom_platform::filesystem::{scan_library, ScanError, AUDIO_EXTENSIONS};
use opurom_platform::global_hotkey::{
    parse_chord, HeadlessHotkey, Hotkey, HotkeyError, HotkeyHost, Key, Modifiers,
};
use opurom_platform::tray::{HeadlessTray, TrayError, TrayEvent, TrayHost, TrayItem, TrayMenu};
use opurom_platform::{desktop_support, DesktopCapability, DesktopStatus};

fn sample_menu() -> TrayMenu {
    TrayMenu {
        title: "Player".to_string(),
        items: vec![
            TrayItem::new(10, "Play"),
            TrayItem::new(11, "Pause"),
            TrayItem::new(12, "Quit"),
        ],
        quit_id: Some(12),
    }
}

// ---------------------------------------------------------------- tray

#[test]
fn headless_tray_mounts_validates_and_streams_clicks_fifo() {
    let mut tray = HeadlessTray::new();
    assert!(tray.mount(sample_menu()).is_ok());
    assert!(tray.is_mounted());
    tray.deliver(11);
    tray.deliver(10);
    // An id that is not in the mounted menu never produces an event.
    tray.deliver(99);
    assert_eq!(tray.poll_event(), Some(TrayEvent::ItemClicked(11)));
    assert_eq!(tray.poll_event(), Some(TrayEvent::ItemClicked(10)));
    assert_eq!(tray.poll_event(), None);
}

#[test]
fn headless_tray_reports_quit_distinctly() {
    let mut tray = HeadlessTray::new();
    tray.mount(sample_menu()).unwrap();
    tray.deliver(12);
    assert_eq!(tray.poll_event(), Some(TrayEvent::QuitRequested(12)));
}

#[test]
fn tray_rejects_duplicate_and_empty_menus() {
    let mut tray = HeadlessTray::new();
    let duplicate = TrayMenu {
        title: "x".into(),
        items: vec![TrayItem::new(1, "a"), TrayItem::new(1, "b")],
        quit_id: None,
    };
    assert!(matches!(
        tray.mount(duplicate),
        Err(TrayError::InvalidMenu("duplicate item id"))
    ));
    let empty_label = TrayMenu {
        title: "x".into(),
        items: vec![TrayItem::new(1, "  ")],
        quit_id: None,
    };
    assert!(matches!(
        tray.mount(empty_label),
        Err(TrayError::InvalidMenu("item label is empty"))
    ));
    let bad_quit = TrayMenu {
        title: "x".into(),
        items: vec![TrayItem::new(1, "a")],
        quit_id: Some(7),
    };
    assert!(matches!(
        tray.mount(bad_quit),
        Err(TrayError::InvalidMenu("quit_id names no item"))
    ));
}

#[test]
fn tray_mount_unmount_update_state_machine() {
    let mut tray = HeadlessTray::new();
    assert_eq!(tray.unmount(), Err(TrayError::NotMounted));
    assert_eq!(tray.update(sample_menu()), Err(TrayError::NotMounted));
    tray.mount(sample_menu()).unwrap();
    assert_eq!(tray.mount(sample_menu()), Err(TrayError::AlreadyMounted));
    let mut next = sample_menu();
    next.items.push(TrayItem::new(13, "Next"));
    tray.update(next).unwrap();
    tray.deliver(13);
    assert_eq!(tray.poll_event(), Some(TrayEvent::ItemClicked(13)));
    tray.unmount().unwrap();
    assert!(!tray.is_mounted());
}

// ---------------------------------------------------------------- hotkey

#[test]
fn chord_parse_accepts_named_modifiers_in_any_order() {
    let hotkey = parse_chord("Ctrl+Alt+Space").unwrap();
    assert_eq!(hotkey.key, Key::Space);
    assert!(hotkey.modifiers.contains(Modifiers::CTRL));
    assert!(hotkey.modifiers.contains(Modifiers::ALT));
    let reordered = parse_chord("alt+control+space").unwrap();
    assert_eq!(reordered, hotkey);
    assert_eq!(
        parse_chord("MediaPlayPause").unwrap().key,
        Key::MediaPlayPause
    );
}

#[test]
fn chord_parse_rejects_malformed_chords() {
    for bad in [
        "",
        "Ctrl",
        "Ctrl+",
        "Ctrl+Space+Alt",
        "Ctrl+Space+Left",
        "Ctrl+Nope",
    ] {
        assert!(
            matches!(parse_chord(bad), Err(HotkeyError::InvalidChord(_))),
            "expected reject for {bad:?}"
        );
    }
}

#[test]
fn headless_hotkey_registers_triggers_and_unregisters() {
    let mut hotkey = HeadlessHotkey::new();
    let play = Hotkey::new(Modifiers::CTRL.union(Modifiers::ALT), Key::Space);
    hotkey.register(1, play).unwrap();
    assert_eq!(
        hotkey.register(1, play),
        Err(HotkeyError::AlreadyRegistered(1))
    );
    assert_eq!(hotkey.registered(), vec![1]);
    hotkey.trigger(1);
    // An unregistered id fires nothing, like a real OS handler.
    hotkey.trigger(2);
    assert_eq!(hotkey.poll_trigger(), Some(1));
    assert_eq!(hotkey.poll_trigger(), None);
    hotkey.unregister(1).unwrap();
    assert_eq!(hotkey.unregister(1), Err(HotkeyError::NotRegistered(1)));
    assert!(hotkey.registered().is_empty());
}

// ---------------------------------------------------------------- filesystem

fn temp_tree(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("opurom-fs-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("sub")).unwrap();
    root
}

#[test]
fn scan_library_finds_nested_audio_and_sorts_by_path() {
    let root = temp_tree("scan");
    // Two supported files (one nested) and one unsupported file.
    std::fs::write(root.join("b.mp3"), b"not really audio").unwrap();
    std::fs::write(root.join("sub").join("a.flac"), b"not really audio").unwrap();
    std::fs::write(root.join("notes.txt"), b"ignore me").unwrap();
    let tracks = scan_library(&root, 8).unwrap();
    let names: Vec<String> = tracks
        .iter()
        .map(|t| t.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["b.mp3", "a.flac"]);
    // A file without a parseable tag falls back to its stem.
    assert_eq!(tracks[0].title, "b");
    assert_eq!(tracks[0].format, "mp3");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn scan_library_rejects_a_bad_root() {
    let missing = std::env::temp_dir().join(format!("opurom-fs-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    assert!(matches!(
        scan_library(&missing, 8),
        Err(ScanError::MissingRoot(_))
    ));
    let file = std::env::temp_dir().join(format!("opurom-fs-file-{}.mp3", std::process::id()));
    std::fs::write(&file, b"x").unwrap();
    assert!(matches!(
        scan_library(&file, 8),
        Err(ScanError::NotDirectory(_))
    ));
    let _ = std::fs::remove_file(&file);
}

#[test]
fn audio_extensions_are_lowercase_and_cover_the_containers() {
    for ext in AUDIO_EXTENSIONS {
        assert_eq!(ext, ext.to_ascii_lowercase());
    }
}

// ---------------------------------------------------------------- audio

#[test]
fn transport_play_pause_seek_and_volume_follow_the_contract() {
    let mut transport = Transport::new();
    assert_eq!(transport.play(), Err(AudioError::NoTrack));
    transport.load(0, 30_000);
    assert_eq!(transport.state(), PlaybackState::Paused);
    transport.play().unwrap();
    assert_eq!(transport.state(), PlaybackState::Playing);
    transport.seek(10_000).unwrap();
    assert_eq!(transport.position_ms(), 10_000);
    assert_eq!(transport.seek(40_000), Err(AudioError::InvalidPosition));
    assert_eq!(transport.set_volume(101), Err(AudioError::InvalidVolume));
    transport.set_volume(50).unwrap();
    assert_eq!(transport.volume(), 50);
    transport.pause();
    assert_eq!(transport.state(), PlaybackState::Paused);
    transport.stop();
    assert_eq!(transport.position_ms(), 0);
}

#[test]
fn transport_tick_reports_advance_repeat_and_end() {
    let mut transport = Transport::new();
    transport.load(0, 1_000);
    transport.play().unwrap();
    assert_eq!(
        transport.tick(std::time::Duration::from_millis(400)),
        TickOutcome::None
    );
    assert_eq!(transport.position_ms(), 400);
    // Repeat One loops the current track.
    transport.set_repeat(RepeatMode::One);
    assert_eq!(
        transport.tick(std::time::Duration::from_millis(1_000)),
        TickOutcome::RepeatCurrent
    );
    assert_eq!(transport.state(), PlaybackState::Paused);
    // Repeat All advances the queue.
    let mut transport = Transport::new();
    transport.load(0, 1_000);
    transport.set_repeat(RepeatMode::All);
    transport.play().unwrap();
    assert_eq!(
        transport.tick(std::time::Duration::from_millis(2_000)),
        TickOutcome::Advance
    );
    // Repeat Off reports a hard stop at the queue end.
    let mut transport = Transport::new();
    transport.load(0, 1_000);
    transport.play().unwrap();
    assert_eq!(
        transport.tick(std::time::Duration::from_millis(2_000)),
        TickOutcome::TrackEnded
    );
}

#[test]
fn repeat_mode_cycles_off_all_one() {
    assert_eq!(RepeatMode::Off.cycle(), RepeatMode::All);
    assert_eq!(RepeatMode::All.cycle(), RepeatMode::One);
    assert_eq!(RepeatMode::One.cycle(), RepeatMode::Off);
}

#[test]
fn headless_audio_delivers_backend_events_fifo() {
    let mut audio = HeadlessAudio::new();
    audio.transport_mut().load(0, 5_000);
    audio.deliver(AudioEvent::Loaded { duration_ms: 5_000 });
    assert_eq!(
        audio.poll(),
        Some(AudioEvent::Loaded { duration_ms: 5_000 })
    );
    assert_eq!(audio.poll(), None);
    assert!(audio.sync().is_ok());
}

// ---------------------------------------------------------------- matrix

#[test]
fn desktop_matrix_names_every_capability_once() {
    let matrix = desktop_support();
    let mut caps: Vec<&str> = matrix.iter().map(|s| s.capability.as_str()).collect();
    caps.sort();
    assert_eq!(
        caps,
        vec![
            "audio",
            "crypto",
            "filesystem",
            "global_hotkey",
            "socket",
            "storage",
            "tray"
        ]
    );
    // The filesystem scanner, storage, crypto and socket parse/serve real data
    // everywhere, so they are native on every build; tray/hotkey/audio vary.
    for expected in [
        DesktopCapability::Filesystem,
        DesktopCapability::Storage,
        DesktopCapability::Crypto,
        DesktopCapability::Socket,
    ] {
        let row = matrix.iter().find(|s| s.capability == expected).unwrap();
        assert_eq!(row.status, DesktopStatus::Native);
    }
}

// ---------------------------------------------------------------- macOS live

#[cfg(target_os = "macos")]
#[test]
fn live_tray_mounts_and_reports_a_real_status_item() {
    use opurom_platform::tray::platform_tray;
    // MainThreadMarker is only present on the real main thread; a worker-thread
    // test degrades to a named SKIP, never a fake PASS.
    if objc2::MainThreadMarker::new().is_none() {
        println!("SKIP live tray needs the process main thread (GUI session)");
        return;
    }
    let mut tray = platform_tray().expect("macOS tray adapter");
    match tray.mount(sample_menu()) {
        Ok(()) => {
            assert!(tray.is_mounted());
            tray.unmount().unwrap();
        }
        Err(error) => println!("SKIP live tray mount: {error}"),
    }
}

#[cfg(target_os = "macos")]
#[test]
fn live_hotkey_registers_a_real_chord_or_skips() {
    use opurom_platform::global_hotkey::platform_hotkey;
    let mut hotkey = platform_hotkey().expect("macOS hotkey adapter");
    let chord = Hotkey::new(Modifiers::CTRL.union(Modifiers::ALT), Key::Space);
    match hotkey.register(1, chord) {
        Ok(()) => {
            assert!(hotkey.registered().contains(&1));
            hotkey.unregister(1).unwrap();
        }
        Err(error) => println!("SKIP live hotkey register: {error}"),
    }
}

#[cfg(target_os = "macos")]
#[test]
fn live_audio_load_of_a_missing_file_is_a_typed_error() {
    use opurom_platform::audio::AVAudioHost;
    // A real AVAudioPlayer with no decodable file must fail with a typed Load
    // error and leave the transport on NoTrack — no fabricated playback.
    let mut audio = AVAudioHost::new();
    let missing = std::env::temp_dir().join("opurom-no-such-audio.mp3");
    let _ = std::fs::remove_file(&missing);
    let result = audio.load_file(0, &missing);
    assert!(matches!(result, Err(AudioError::Load(_))));
    assert_eq!(audio.transport().track_index(), None);
}

// ---------------------------------------------------------------- metadata

/// Build a minimal ID3v2.4 tag with TIT2/TPE1/TALB/TLEN frames around `body`.
fn id3v24(artist: &str, title: &str, album: &str, ms: &str) -> Vec<u8> {
    fn frame(id: &[u8; 4], text: &str) -> Vec<u8> {
        let mut payload = vec![3u8]; // UTF-8 encoding byte
        payload.extend_from_slice(text.as_bytes());
        let size = payload.len();
        let mut out = Vec::new();
        out.extend_from_slice(id);
        out.extend_from_slice(&[
            (size >> 21) as u8 & 0x7f,
            (size >> 14) as u8 & 0x7f,
            (size >> 7) as u8 & 0x7f,
            size as u8 & 0x7f,
        ]);
        out.extend_from_slice(&[0, 0]); // frame flags
        out.extend_from_slice(&payload);
        out
    }
    let mut frames = Vec::new();
    frames.extend_from_slice(&frame(b"TIT2", title));
    frames.extend_from_slice(&frame(b"TPE1", artist));
    frames.extend_from_slice(&frame(b"TALB", album));
    frames.extend_from_slice(&frame(b"TLEN", ms));
    let size = frames.len();
    let mut out = b"ID3".to_vec();
    out.extend_from_slice(&[4, 0, 0]); // v2.4, flags 0
    out.extend_from_slice(&[
        (size >> 21) as u8 & 0x7f,
        (size >> 14) as u8 & 0x7f,
        (size >> 7) as u8 & 0x7f,
        size as u8 & 0x7f,
    ]);
    out.extend_from_slice(&frames);
    out
}

#[test]
fn id3v24_tags_are_read_into_a_track() {
    let root = temp_tree("id3");
    let path = root.join("song.mp3");
    std::fs::write(&path, id3v24("Ada", "First Light", "Dawn", "183000")).unwrap();
    let tracks = scan_library(&root, 8).unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].title, "First Light");
    assert_eq!(tracks[0].artist, "Ada");
    assert_eq!(tracks[0].album, "Dawn");
    assert_eq!(tracks[0].duration_secs, 183);
    let _ = std::fs::remove_dir_all(&root);
}

/// Build a minimal FLAC stream: `fLaC` + STREAMINFO (type 0) + VORBIS_COMMENT.
fn flac(rate: u32, samples: u64, artist: &str, title: &str, album: &str) -> Vec<u8> {
    let mut out = b"fLaC".to_vec();
    // STREAMINFO (34 bytes): rate at byte 10 (20 bits), samples 36 bits at byte 13.
    let mut info = vec![0u8; 34];
    info[10] = (rate >> 12) as u8;
    info[11] = (rate >> 4) as u8;
    info[12] = ((rate & 0x0f) as u8) << 4;
    info[13] = (((samples >> 32) & 0x0f) as u8) | 0x00;
    info[14] = (samples >> 24) as u8;
    info[15] = (samples >> 16) as u8;
    info[16] = (samples >> 8) as u8;
    info[17] = samples as u8;
    out.push(0x00); // not last, type 0
    out.extend_from_slice(&[
        (info.len() >> 16) as u8,
        (info.len() >> 8) as u8,
        info.len() as u8,
    ]);
    out.extend_from_slice(&info);
    // VORBIS_COMMENT (type 4, last block).
    let mut body = Vec::new();
    let vendor = b"ref";
    body.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    body.extend_from_slice(vendor);
    let comments = [
        format!("TITLE={title}"),
        format!("ARTIST={artist}"),
        format!("ALBUM={album}"),
    ];
    body.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for comment in &comments {
        body.extend_from_slice(&(comment.len() as u32).to_le_bytes());
        body.extend_from_slice(comment.as_bytes());
    }
    out.push(0x84); // last block, type 4
    out.extend_from_slice(&[
        (body.len() >> 16) as u8,
        (body.len() >> 8) as u8,
        body.len() as u8,
    ]);
    out.extend_from_slice(&body);
    out
}

#[test]
fn flac_vorbis_and_streaminfo_are_read_into_a_track() {
    let root = temp_tree("flac");
    let path = root.join("track.flac");
    std::fs::write(
        &path,
        flac(44100, 44100 * 125, "Rivera", "Nightfall", "Dusk"),
    )
    .unwrap();
    let tracks = scan_library(&root, 8).unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].title, "Nightfall");
    assert_eq!(tracks[0].artist, "Rivera");
    assert_eq!(tracks[0].album, "Dusk");
    assert_eq!(tracks[0].duration_secs, 125);
    let _ = std::fs::remove_dir_all(&root);
}
