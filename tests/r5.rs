use opurom_platform::{AdapterError, FrameView, InputEvent, Size, WindowSet, APPKIT_UNAVAILABLE};

fn frame<'a>(width: u32, height: u32, pixels: &'a [u8]) -> FrameView<'a> {
    FrameView {
        width,
        height,
        pixels,
    }
}

fn size(width: u32, height: u32) -> Size {
    Size { width, height }
}

fn temp_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "opurom-platform-r5-{}-{label}.png",
        std::process::id()
    ))
}

#[test]
fn window_set_enforces_two_to_four_viewports() {
    let viewport = size(2, 2);
    let invalid = AdapterError::Backend("WindowSet requires 2..4 viewports");
    assert_eq!(WindowSet::new([viewport]).err(), Some(invalid));
    assert_eq!(WindowSet::new([viewport; 5]).err(), Some(invalid));
    assert_eq!(WindowSet::new([viewport; 2]).unwrap().len(), 2);
    assert_eq!(WindowSet::new([viewport; 4]).unwrap().len(), 4);
}

#[test]
fn window_set_isolates_resize_events_and_frames() {
    let mut windows = WindowSet::new([size(2, 1), size(1, 2), size(3, 1)]).unwrap();
    windows
        .push_event(
            0,
            InputEvent::Key {
                code: 1,
                pressed: true,
            },
        )
        .unwrap();
    windows
        .push_event(
            1,
            InputEvent::Key {
                code: 2,
                pressed: true,
            },
        )
        .unwrap();

    windows.resize(0, size(4, 4)).unwrap();
    assert_eq!(windows.viewport(0).unwrap(), size(4, 4));
    assert_eq!(windows.viewport(1).unwrap(), size(1, 2));
    assert_eq!(windows.viewport(2).unwrap(), size(3, 1));
    assert_eq!(
        windows.poll_event(0),
        Ok(Some(InputEvent::Key {
            code: 1,
            pressed: true
        }))
    );
    assert_eq!(
        windows.poll_event(0),
        Ok(Some(InputEvent::Resize { size: size(4, 4) }))
    );
    assert_eq!(windows.poll_event(0), Ok(None));
    assert_eq!(
        windows.poll_event(1),
        Ok(Some(InputEvent::Key {
            code: 2,
            pressed: true
        }))
    );
    assert_eq!(windows.poll_event(2), Ok(None));

    let red = [255u8, 0, 0, 255];
    let blue = [0u8, 0, 255, 255];
    windows.present(0, frame(1, 1, &red)).unwrap();
    windows.present(2, frame(1, 1, &blue)).unwrap();
    assert_eq!(windows.frame(0).unwrap().pixels, &red[..]);
    assert!(windows.frame(1).unwrap().pixels.is_empty());
    assert_eq!(windows.frame(2).unwrap().pixels, &blue[..]);
}

#[test]
fn window_set_rejects_every_invalid_index_operation() {
    let mut windows = WindowSet::new([size(1, 1); 2]).unwrap();
    assert!(matches!(
        windows.viewport(2),
        Err(AdapterError::Backend("invalid window index"))
    ));
    assert!(matches!(
        windows.resize(2, size(2, 2)),
        Err(AdapterError::Backend("invalid window index"))
    ));
    assert!(matches!(
        windows.push_event(
            2,
            InputEvent::Key {
                code: 9,
                pressed: true
            }
        ),
        Err(AdapterError::Backend("invalid window index"))
    ));
    assert!(matches!(
        windows.poll_event(2),
        Err(AdapterError::Backend("invalid window index"))
    ));
    assert!(matches!(
        windows.present(2, frame(1, 1, &[0u8; 4])),
        Err(AdapterError::Backend("invalid window index"))
    ));
    assert!(matches!(
        windows.frame(2),
        Err(AdapterError::Backend("invalid window index"))
    ));
}

#[test]
fn non_macos_fallback_is_a_documented_error() {
    assert!(APPKIT_UNAVAILABLE.starts_with("AppKitUnavailable"));
    #[cfg(not(target_os = "macos"))]
    {
        let pixels = [1u8, 2, 3, 255];
        let path = temp_path("non-macos-fallback");
        assert_eq!(
            opurom_platform::frame_to_png(frame(1, 1, &pixels), &path),
            Err(AdapterError::Backend(APPKIT_UNAVAILABLE))
        );
        assert!(opurom_platform::spec_by_name("appkit").is_none());
        assert_eq!(opurom_platform::host_platforms().len(), 2);
        assert!(!path.exists());
    }
}

#[cfg(target_os = "macos")]
mod appkit {
    use super::{frame, size, temp_path, AdapterError, InputEvent};
    use opurom_platform::{
        appkit_capabilities, appkit_spec, frame_to_png, host_platforms, registry, spec_by_name,
        MacHost, PlatformAdapter, Status, APPKIT_SPEC,
    };
    use std::fs::{self, File};

    #[test]
    fn host_description_uses_real_objc_classes_and_window_style() {
        let descriptor = MacHost::describe();
        assert_eq!(descriptor.application_class, "NSApplication");
        assert_eq!(descriptor.window_class, "NSWindow");
        assert_eq!(descriptor.view_class, "NSView");
        assert_eq!(descriptor.surface_class, "CAMetalLayer");
        assert_eq!(
            descriptor.window_style,
            "Titled|Closable|Miniaturizable|Resizable"
        );
        assert_eq!(descriptor.window_style_mask, 15);
        assert_eq!(
            descriptor.to_string(),
            "NSApplication | NSWindow | NSView | CAMetalLayer | Titled|Closable|Miniaturizable|Resizable (15)"
        );
        println!("MacHost::describe() = {descriptor}");
    }

    #[test]
    fn appkit_capabilities_report_native_host_and_dpi_scale() {
        let capabilities = appkit_capabilities();
        assert!(!capabilities.gpu);
        assert!(capabilities.native_embed);
        assert!(capabilities.multi_window);
        assert!(!capabilities.lifecycle);
        assert!(capabilities.dpi_scale.is_finite() && capabilities.dpi_scale >= 1.0);

        let host = MacHost::new(4, 3, temp_path("capabilities"));
        let host_capabilities = host.capabilities();
        assert_eq!(host_capabilities.gpu, capabilities.gpu);
        assert_eq!(host_capabilities.native_embed, capabilities.native_embed);
        assert_eq!(host_capabilities.multi_window, capabilities.multi_window);
        assert!(host_capabilities.dpi_scale.is_finite() && host_capabilities.dpi_scale >= 1.0);
        assert_eq!(host.name(), "appkit");
        assert_eq!(host.viewport(), size(4, 3));
    }

    #[test]
    fn frame_to_png_writes_signature_ihdr_and_exact_pixels() {
        let pixels = [
            255u8, 0, 0, 255, // opaque red
            0, 255, 0, 128, // half-transparent green
            0, 0, 255, 255, // opaque blue
            10, 20, 30, 40, // arbitrary RGBA
        ];
        let path = temp_path("frame-to-png");
        assert_eq!(frame_to_png(frame(2, 2, &pixels), &path), Ok(()));

        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], &b"\x89PNG\r\n\x1a\n"[..]);
        assert_eq!(&bytes[12..16], &b"IHDR"[..]);
        assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 2);
        assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 2);
        assert_eq!(bytes[24], 8, "8 bits per channel");
        assert_eq!(bytes[25], 6, "IHDR colour type 6 is RGBA");

        let decoder = png::Decoder::new(File::open(&path).unwrap());
        let mut reader = decoder.read_info().unwrap();
        let mut decoded = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut decoded).unwrap();
        assert_eq!(info.width, 2);
        assert_eq!(info.height, 2);
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        assert_eq!(&decoded[..info.buffer_size()], &pixels);
        println!(
            "PNG verification: signature=89504e470d0a1a0a IHDR=2x2 bit_depth=8 color_type=6 decoded={:?}",
            &decoded[..info.buffer_size()]
        );
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn invalid_frames_are_rejected_without_mutating_host_state() {
        let path = temp_path("invalid-frame");
        let mut host = MacHost::new(2, 2, &path);
        let pixels = [7u8; 16];
        assert_eq!(host.present(frame(2, 2, &pixels)), Ok(()));
        let digest = host.digest().to_string();
        host.push_event(InputEvent::Key {
            code: 7,
            pressed: true,
        });
        assert_eq!(
            host.poll_event(),
            Some(InputEvent::Key {
                code: 7,
                pressed: true
            })
        );

        assert_eq!(
            host.present(frame(2, 2, &[0u8; 15])),
            Err(AdapterError::InvalidFrame)
        );
        assert_eq!(
            host.present(frame(u32::MAX, u32::MAX, &[])),
            Err(AdapterError::InvalidFrame)
        );
        assert_eq!(
            host.present(frame(0, 0, &[])),
            Err(AdapterError::InvalidFrame)
        );
        assert_eq!(host.digest(), digest);
        assert_eq!(host.frame().pixels, &pixels[..]);
        assert!(path.exists(), "the previously valid PNG remains");
        fs::remove_file(&path).unwrap();

        let bad_path = temp_path("bad-frame");
        assert_eq!(
            frame_to_png(frame(2, 2, &[0u8; 15]), &bad_path),
            Err(AdapterError::InvalidFrame)
        );
        assert_eq!(
            frame_to_png(frame(0, 0, &[]), &bad_path),
            Err(AdapterError::InvalidFrame)
        );
        assert!(
            !bad_path.exists(),
            "no file is created for an invalid frame"
        );
    }

    #[test]
    fn appkit_is_a_verified_host_without_changing_target_registry() {
        let spec = spec_by_name("appkit").expect("appkit is registered on macOS");
        assert_eq!(spec.status, Status::Verified);
        assert_eq!(spec.emitter, "appkit");
        assert!(spec.capabilities.native_embed);
        assert!(spec.capabilities.multi_window);
        assert!(!spec.capabilities.gpu);
        assert!(spec.capabilities.dpi_scale.is_finite() && spec.capabilities.dpi_scale >= 1.0);
        let mut expected = APPKIT_SPEC.clone();
        expected.capabilities.dpi_scale = spec.capabilities.dpi_scale;
        assert_eq!(spec, expected);
        assert_eq!(appkit_spec().name, "appkit");
        assert_eq!(appkit_spec().status, Status::Verified);

        let hosts = host_platforms();
        assert_eq!(hosts.len(), 3);
        for name in ["headless", "terminal", "appkit"] {
            assert!(hosts
                .iter()
                .any(|candidate| candidate.name == name && candidate.status == Status::Verified));
        }
        assert_eq!(registry().len(), 14);
        assert!(!registry()
            .iter()
            .any(|candidate| candidate.name == "appkit"));
        assert_eq!(opurom_platform::all_platforms().len(), 15);
    }
}

#[cfg(target_os = "macos")]
mod cli {
    use std::path::PathBuf;
    use std::process::Command;

    fn binary() -> PathBuf {
        option_env!("CARGO_BIN_EXE_opurom_platform")
            .or_else(|| option_env!("CARGO_BIN_EXE_opurom-platform"))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/opurom-platform")
            })
    }

    fn run(args: &[&str]) -> String {
        let output = Command::new(binary())
            .args(args)
            .output()
            .expect("run opurom-platform binary");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("stdout is utf-8")
    }

    #[test]
    fn png_flag_writes_a_deterministic_4x4_png() {
        let path = super::temp_path("cli-png");
        let path_str = path.to_str().unwrap();
        assert_eq!(run(&["--png", path_str]), "");

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], &b"\x89PNG\r\n\x1a\n"[..]);
        assert_eq!(&bytes[12..16], &b"IHDR"[..]);
        assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 4);
        assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 4);
        assert_eq!(bytes[24], 8);
        assert_eq!(bytes[25], 6);

        let decoder = png::Decoder::new(std::fs::File::open(&path).unwrap());
        let mut reader = decoder.read_info().unwrap();
        let mut decoded = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut decoded).unwrap();
        assert_eq!(info.width, 4);
        assert_eq!(info.height, 4);
        assert_eq!(info.buffer_size(), 4 * 4 * 4);
        assert_eq!(&decoded[..4], &[0, 0, 0, 255]);
        assert_eq!(&decoded[60..64], &[192, 192, 192, 255]);
        println!(
            "CLI --png verification: 4x4 RGBA8 PNG, first={:?}, last={:?}",
            &decoded[..4],
            &decoded[60..64]
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn hosts_flag_lists_appkit_and_default_registry_is_unchanged() {
        let hosts = run(&["--hosts"]);
        let lines: Vec<&str> = hosts.lines().collect();
        assert_eq!(
            lines[0],
            "name | emitter | status | gpu | native_embed | multi_window"
        );
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[3], "appkit | appkit | Verified | false | true | true");

        let default = run(&[]);
        assert_eq!(default.lines().count(), 15);
        assert!(!default.contains("appkit"));
        assert_eq!(
            default.lines().next().unwrap(),
            "name | emitter | status | gpu | native_embed | multi_window"
        );
    }
}
