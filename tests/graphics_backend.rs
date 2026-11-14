//! Headless conformance for the deterministic graphics backend model.

use opurom_platform::wayland_wire::{
    decode_batch, decode_event, decode_graphics_backend_event, decode_header,
    encode_graphics_backend_event, header, WireEvent, DISPLAY_ID, EVENT_GRAPHICS_BACKEND,
    EVENT_MOVE, EVENT_OBJECT_ID, GRAPHICS_BACKEND_BODY_LEN, GRAPHICS_BACKEND_MESSAGE_LEN,
    HEADER_LEN,
};
use opurom_platform::{
    select_backend, select_backend_with_reason, BackendError, BackendSelection, FallbackReason,
    GraphicsBackend, GraphicsBackendCapabilities, GraphicsCapabilities, Headless, HostDescriptor,
    HostOs, PlatformAdapter, SelectionReason, Terminal, EFFECT_BLEND, EFFECT_BLUR, EFFECT_COMPUTE,
    EFFECT_MASK, EFFECT_SHADOW, FALLBACK_REASON_BASE, MAX_COMMAND_BUFFERS, MAX_MSAA_SAMPLES,
    MAX_TEXTURE_SIZE, MIN_TEXTURE_SIZE,
};

fn expected_capabilities(backend: GraphicsBackend) -> GraphicsCapabilities {
    match backend {
        GraphicsBackend::Metal => GraphicsCapabilities {
            max_texture_size: 16_384,
            msaa_samples: 8,
            supported_effects: EFFECT_MASK,
            requires_live_device: true,
            max_command_buffers: 3,
        },
        GraphicsBackend::Vulkan => GraphicsCapabilities {
            max_texture_size: 16_384,
            msaa_samples: 16,
            supported_effects: EFFECT_MASK,
            requires_live_device: true,
            max_command_buffers: MAX_COMMAND_BUFFERS,
        },
        GraphicsBackend::AngleGlEs => GraphicsCapabilities {
            max_texture_size: 8_192,
            msaa_samples: 4,
            supported_effects: EFFECT_BLEND | EFFECT_BLUR | EFFECT_SHADOW,
            requires_live_device: true,
            max_command_buffers: 1,
        },
        GraphicsBackend::D3D11 => GraphicsCapabilities {
            max_texture_size: 16_384,
            msaa_samples: 8,
            supported_effects: EFFECT_BLEND | EFFECT_BLUR | EFFECT_SHADOW,
            requires_live_device: true,
            max_command_buffers: 4,
        },
        GraphicsBackend::D3D12 => GraphicsCapabilities {
            max_texture_size: 16_384,
            msaa_samples: 16,
            supported_effects: EFFECT_MASK,
            requires_live_device: true,
            max_command_buffers: MAX_COMMAND_BUFFERS,
        },
        GraphicsBackend::Software => GraphicsCapabilities {
            max_texture_size: 4_096,
            msaa_samples: 1,
            supported_effects: EFFECT_BLEND,
            requires_live_device: false,
            max_command_buffers: 1,
        },
    }
}

fn host_for(backend: GraphicsBackend) -> HostOs {
    match backend {
        GraphicsBackend::Metal => HostOs::MacOs,
        GraphicsBackend::D3D11 | GraphicsBackend::D3D12 => HostOs::Windows,
        _ => HostOs::Linux,
    }
}

fn set_word(bytes: &mut [u8], index: usize, value: u32) {
    let start = HEADER_LEN + index * 4;
    bytes[start..start + 4].copy_from_slice(&value.to_ne_bytes());
}

fn with_header(bytes: &[u8], object_id: u32, opcode: u16, size: u16) -> Vec<u8> {
    let mut copy = bytes.to_vec();
    copy[..HEADER_LEN].copy_from_slice(&header(object_id, opcode, size));
    copy
}

fn native<'a>(little: &'a [u8], big: &'a [u8]) -> &'a [u8] {
    if cfg!(target_endian = "little") {
        little
    } else {
        big
    }
}

#[test]
fn every_backend_has_deterministic_capabilities() {
    assert_eq!(
        (
            EFFECT_BLEND,
            EFFECT_BLUR,
            EFFECT_SHADOW,
            EFFECT_COMPUTE,
            EFFECT_MASK
        ),
        (1, 2, 4, 8, 15)
    );
    assert_eq!(
        (
            MIN_TEXTURE_SIZE,
            MAX_TEXTURE_SIZE,
            MAX_MSAA_SAMPLES,
            MAX_COMMAND_BUFFERS
        ),
        (256, 16_384, 16, 8)
    );
    assert_eq!(GraphicsBackend::ALL.len(), 6);

    for (index, &backend) in GraphicsBackend::ALL.iter().enumerate() {
        let tag = index as u32 + 1;
        let capabilities = expected_capabilities(backend);
        assert_eq!(backend.tag(), tag);
        assert_eq!(GraphicsBackend::from_tag(tag), Ok(backend));
        assert_eq!(backend.capabilities(), capabilities);
        assert_eq!(capabilities.validate_for(backend), Ok(()));
        assert!(backend.supports_os(host_for(backend)));
    }

    for os in [HostOs::MacOs, HostOs::Windows, HostOs::Linux, HostOs::Other] {
        assert_eq!(HostOs::from_tag(os.tag()), Ok(os));
        assert!(GraphicsBackend::Vulkan.supports_os(os));
        assert!(GraphicsBackend::AngleGlEs.supports_os(os));
        assert!(GraphicsBackend::Software.supports_os(os));
    }
    assert!(GraphicsBackend::Metal.supports_os(HostOs::MacOs));
    assert!(!GraphicsBackend::Metal.supports_os(HostOs::Windows));
    assert!(GraphicsBackend::D3D11.supports_os(HostOs::Windows));
    assert!(GraphicsBackend::D3D12.supports_os(HostOs::Windows));
    assert!(!GraphicsBackend::D3D12.supports_os(HostOs::Linux));

    assert_eq!(SelectionReason::HostDefault.tag(), 1);
    assert_eq!(SelectionReason::Requested.tag(), 2);
    assert_eq!(SelectionReason::Preferred.tag(), 3);
    assert_eq!(SelectionReason::Available.tag(), 4);
    assert_eq!(
        SelectionReason::SoftwareFallback(FallbackReason::NoSupportedBackend).tag(),
        FALLBACK_REASON_BASE + 1
    );
}

#[test]
fn invalid_capability_sets_have_typed_errors() {
    let software = GraphicsBackend::Software.capabilities();
    assert_eq!(software.validate(), Ok(()));
    assert_eq!(
        GraphicsCapabilities {
            supported_effects: 0x80,
            ..software
        }
        .validate(),
        Err(BackendError::UnknownEffectMask(0x80))
    );
    assert_eq!(
        GraphicsCapabilities {
            max_texture_size: 0,
            ..software
        }
        .validate(),
        Err(BackendError::InvalidTextureSize(0))
    );
    assert_eq!(
        GraphicsCapabilities {
            msaa_samples: 3,
            ..software
        }
        .validate(),
        Err(BackendError::InvalidMsaaSamples(3))
    );
    assert_eq!(
        GraphicsCapabilities {
            max_command_buffers: 0,
            ..software
        }
        .validate(),
        Err(BackendError::InvalidCommandBuffers(0))
    );
    assert_eq!(
        GraphicsCapabilities {
            supported_effects: EFFECT_BLEND | EFFECT_COMPUTE,
            ..software
        }
        .validate(),
        Err(BackendError::InconsistentCapabilitySet)
    );
    assert_eq!(
        GraphicsCapabilities {
            max_texture_size: 8_192,
            ..software
        }
        .validate_for(GraphicsBackend::Software),
        Err(BackendError::UnexpectedCapabilities(
            GraphicsBackend::Software
        ))
    );
    assert_eq!(
        GraphicsBackendCapabilities::new(
            HostOs::Linux,
            GraphicsBackend::D3D12,
            SelectionReason::HostDefault
        )
        .validate(),
        Err(BackendError::UnsupportedOnHost {
            os: HostOs::Linux,
            backend: GraphicsBackend::D3D12,
        })
    );
    assert_eq!(HostOs::from_tag(0), Err(BackendError::UnknownHostOs(0)));
    assert_eq!(
        GraphicsBackend::from_tag(99),
        Err(BackendError::UnknownBackend(99))
    );
    assert_eq!(
        SelectionReason::from_tag(99),
        Err(BackendError::UnknownSelectionReason(99))
    );
    assert!(BackendError::UnknownEffectMask(0x80)
        .to_string()
        .contains("effect mask"));
}

#[test]
fn selection_honors_requested_preferences_and_availability() {
    let host = HostDescriptor::new(
        HostOs::Windows,
        [
            GraphicsBackend::AngleGlEs,
            GraphicsBackend::D3D11,
            GraphicsBackend::D3D12,
        ],
        [GraphicsBackend::D3D12, GraphicsBackend::D3D11],
    );
    assert_eq!(
        select_backend(&host, Some(GraphicsBackend::D3D11)),
        Ok(GraphicsBackend::D3D11)
    );
    assert_eq!(
        select_backend_with_reason(&host, Some(GraphicsBackend::D3D11)),
        Ok(BackendSelection {
            backend: GraphicsBackend::D3D11,
            reason: SelectionReason::Requested,
        })
    );
    assert_eq!(select_backend(&host, None), Ok(GraphicsBackend::D3D12));
    assert_eq!(
        select_backend_with_reason(&host, None),
        Ok(BackendSelection {
            backend: GraphicsBackend::D3D12,
            reason: SelectionReason::Preferred,
        })
    );
    assert_eq!(
        select_backend(&host, Some(GraphicsBackend::Software)),
        Ok(GraphicsBackend::Software)
    );

    let missing_preferred = HostDescriptor::new(
        HostOs::Windows,
        [GraphicsBackend::AngleGlEs, GraphicsBackend::D3D11],
        [GraphicsBackend::D3D12, GraphicsBackend::D3D11],
    );
    assert_eq!(
        select_backend_with_reason(&missing_preferred, None),
        Ok(BackendSelection {
            backend: GraphicsBackend::D3D11,
            reason: SelectionReason::Preferred,
        })
    );

    let available_only = HostDescriptor::new(
        HostOs::Windows,
        [GraphicsBackend::AngleGlEs],
        [GraphicsBackend::D3D12, GraphicsBackend::D3D11],
    );
    assert_eq!(
        select_backend_with_reason(&available_only, None),
        Ok(BackendSelection {
            backend: GraphicsBackend::AngleGlEs,
            reason: SelectionReason::Available,
        })
    );

    let unsupported_preference = HostDescriptor::new(
        HostOs::Windows,
        [GraphicsBackend::Metal, GraphicsBackend::D3D12],
        [GraphicsBackend::Metal, GraphicsBackend::D3D12],
    );
    let expected = BackendSelection {
        backend: GraphicsBackend::D3D12,
        reason: SelectionReason::Preferred,
    };
    assert_eq!(
        select_backend_with_reason(&unsupported_preference, None),
        Ok(expected)
    );
    assert_eq!(
        select_backend_with_reason(&unsupported_preference, None),
        Ok(expected),
        "selection is deterministic"
    );
}

#[test]
fn selection_falls_back_to_software_with_typed_reasons() {
    let cases = [
        (
            HostDescriptor::new(HostOs::Windows, [], [GraphicsBackend::D3D12]),
            None,
            FallbackReason::NoAvailableBackends,
        ),
        (
            HostDescriptor::new(HostOs::Windows, [], [GraphicsBackend::D3D12]),
            Some(GraphicsBackend::D3D12),
            FallbackReason::RequestedBackendUnavailable,
        ),
        (
            HostDescriptor::new(HostOs::Linux, [GraphicsBackend::D3D12], []),
            Some(GraphicsBackend::D3D12),
            FallbackReason::RequestedBackendUnsupported,
        ),
        (
            HostDescriptor::new(
                HostOs::Linux,
                [GraphicsBackend::Metal, GraphicsBackend::D3D12],
                [GraphicsBackend::Metal, GraphicsBackend::D3D12],
            ),
            None,
            FallbackReason::NoSupportedBackend,
        ),
        (
            HostDescriptor::new(HostOs::Windows, [GraphicsBackend::Software], []),
            None,
            FallbackReason::NoAvailableBackends,
        ),
    ];
    for (host, requested, fallback) in cases {
        let expected = BackendSelection {
            backend: GraphicsBackend::Software,
            reason: SelectionReason::SoftwareFallback(fallback),
        };
        assert_eq!(
            select_backend(&host, requested),
            Ok(GraphicsBackend::Software)
        );
        assert_eq!(select_backend_with_reason(&host, requested), Ok(expected));
        assert_eq!(
            select_backend_with_reason(&host, requested),
            Ok(expected),
            "fallback is deterministic"
        );
        let capabilities = expected.capabilities(host.os);
        assert_eq!(capabilities.backend, GraphicsBackend::Software);
        assert!(!capabilities.capabilities.requires_live_device);
        assert_eq!(capabilities.validate(), Ok(()));
    }
}

#[test]
fn wire_events_round_trip_for_every_backend() {
    let reasons = [
        SelectionReason::HostDefault,
        SelectionReason::Requested,
        SelectionReason::Preferred,
        SelectionReason::Available,
        SelectionReason::SoftwareFallback(FallbackReason::NoAvailableBackends),
        SelectionReason::SoftwareFallback(FallbackReason::NoSupportedBackend),
    ];
    for (index, &backend) in GraphicsBackend::ALL.iter().enumerate() {
        let capabilities =
            GraphicsBackendCapabilities::new(host_for(backend), backend, reasons[index]);
        let event = WireEvent::GraphicsBackend(capabilities);
        let bytes = event.encode().unwrap();
        assert_eq!(bytes.len(), GRAPHICS_BACKEND_MESSAGE_LEN);
        assert_eq!(encode_graphics_backend_event(capabilities).unwrap(), bytes);
        let parsed = decode_header(&bytes).unwrap();
        assert_eq!(parsed.object_id, EVENT_OBJECT_ID);
        assert_eq!(parsed.opcode, EVENT_GRAPHICS_BACKEND);
        assert_eq!(usize::from(parsed.size), GRAPHICS_BACKEND_MESSAGE_LEN);
        assert_eq!(
            decode_event(&bytes).unwrap(),
            Some((event, bytes.len())),
            "{backend:?} did not round-trip"
        );
        assert_eq!(
            decode_graphics_backend_event(&bytes).unwrap(),
            Some((capabilities, bytes.len()))
        );
        assert_eq!(decode_batch(&bytes, 1), Ok(vec![event]));
    }

    let capabilities = GraphicsBackendCapabilities::new(
        HostOs::Windows,
        GraphicsBackend::D3D12,
        SelectionReason::Preferred,
    );
    let mixed = vec![
        WireEvent::Resize {
            width: 320,
            height: 240,
        },
        WireEvent::GraphicsBackend(capabilities),
        WireEvent::Focus { focused: true },
    ];
    let mut bytes = Vec::new();
    for &event in &mixed {
        bytes.extend_from_slice(&event.encode().unwrap());
    }
    assert_eq!(decode_batch(&bytes, mixed.len()), Ok(mixed));
}

#[test]
fn graphics_backend_wire_framing_is_fixed() {
    assert_eq!(GRAPHICS_BACKEND_BODY_LEN, 36);
    assert_eq!(GRAPHICS_BACKEND_MESSAGE_LEN, 44);
    let capabilities = GraphicsBackendCapabilities::new(
        HostOs::Windows,
        GraphicsBackend::AngleGlEs,
        SelectionReason::Requested,
    );
    let bytes = WireEvent::GraphicsBackend(capabilities).encode().unwrap();
    let little = [
        0x00, 0x00, 0x00, 0xff, 0x07, 0x00, 0x2c, 0x00, // object, size 44, opcode 7
        0x42, 0x47, 0x50, 0x4f, // OPGB
        0x02, 0x00, 0x00, 0x00, // Windows
        0x03, 0x00, 0x00, 0x00, // ANGLE / OpenGL ES
        0x00, 0x20, 0x00, 0x00, // max texture 8192
        0x04, 0x00, 0x00, 0x00, // 4 MSAA samples
        0x07, 0x00, 0x00, 0x00, // blend, blur and shadow
        0x01, 0x00, 0x00, 0x00, // live device required
        0x01, 0x00, 0x00, 0x00, // one command buffer
        0x02, 0x00, 0x00, 0x00, // requested
    ];
    let big = [
        0xff, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x07, // object, size 44, opcode 7
        0x4f, 0x50, 0x47, 0x42, // OPGB
        0x00, 0x00, 0x00, 0x02, // Windows
        0x00, 0x00, 0x00, 0x03, // ANGLE / OpenGL ES
        0x00, 0x00, 0x20, 0x00, // max texture 8192
        0x00, 0x00, 0x00, 0x04, // 4 MSAA samples
        0x00, 0x00, 0x00, 0x07, // blend, blur and shadow
        0x00, 0x00, 0x00, 0x01, // live device required
        0x00, 0x00, 0x00, 0x01, // one command buffer
        0x00, 0x00, 0x00, 0x02, // requested
    ];
    assert_eq!(bytes.as_slice(), native(&little, &big));
    assert_eq!(
        decode_graphics_backend_event(&bytes).unwrap().unwrap().0,
        capabilities
    );
}

#[test]
fn malformed_graphics_backend_wire_is_rejected_without_panics() {
    let capabilities = GraphicsBackendCapabilities::new(
        HostOs::Windows,
        GraphicsBackend::AngleGlEs,
        SelectionReason::Requested,
    );
    let good = WireEvent::GraphicsBackend(capabilities).encode().unwrap();
    assert_eq!(
        decode_graphics_backend_event(&good).unwrap().unwrap().0,
        capabilities
    );

    let mutate = |index: usize, value: u32| {
        let mut bytes = good.clone();
        set_word(&mut bytes, index, value);
        bytes
    };
    let mut cases: Vec<(&str, Vec<u8>, BackendError)> = vec![
        ("bad magic", mutate(0, 0), BackendError::InvalidWireMagic),
        (
            "unknown host OS",
            mutate(1, 99),
            BackendError::UnknownHostOs(99),
        ),
        (
            "unknown backend",
            mutate(2, 99),
            BackendError::UnknownBackend(99),
        ),
        (
            "zero texture size",
            mutate(3, 0),
            BackendError::InvalidTextureSize(0),
        ),
        (
            "non-power-of-two MSAA",
            mutate(4, 3),
            BackendError::InvalidMsaaSamples(3),
        ),
        (
            "unknown effect bit",
            mutate(5, 0x80),
            BackendError::UnknownEffectMask(0x80),
        ),
        (
            "no live device with hardware effects",
            mutate(6, 0),
            BackendError::InconsistentCapabilitySet,
        ),
        (
            "invalid live-device flag",
            mutate(6, 2),
            BackendError::InvalidWireFlag,
        ),
        (
            "zero command buffers",
            mutate(7, 0),
            BackendError::InvalidCommandBuffers(0),
        ),
        (
            "unknown selection reason",
            mutate(8, 99),
            BackendError::UnknownSelectionReason(99),
        ),
        (
            "non-default texture size",
            mutate(3, 16_384),
            BackendError::UnexpectedCapabilities(GraphicsBackend::AngleGlEs),
        ),
        (
            "wrong backend for capability row",
            mutate(2, GraphicsBackend::D3D12.tag()),
            BackendError::UnexpectedCapabilities(GraphicsBackend::D3D12),
        ),
    ];
    let mut unsupported = good.clone();
    set_word(&mut unsupported, 1, HostOs::Linux.tag());
    set_word(&mut unsupported, 2, GraphicsBackend::D3D12.tag());
    cases.push((
        "D3D12 on Linux",
        unsupported,
        BackendError::UnsupportedOnHost {
            os: HostOs::Linux,
            backend: GraphicsBackend::D3D12,
        },
    ));
    cases.push((
        "short body",
        with_header(
            &good,
            EVENT_OBJECT_ID,
            EVENT_GRAPHICS_BACKEND,
            (GRAPHICS_BACKEND_MESSAGE_LEN - 4) as u16,
        ),
        BackendError::InvalidWireLength,
    ));
    cases.push((
        "misaligned header",
        with_header(&good, EVENT_OBJECT_ID, EVENT_GRAPHICS_BACKEND, 9),
        BackendError::InvalidWireHeader,
    ));
    cases.push((
        "wrong object",
        with_header(
            &good,
            DISPLAY_ID,
            EVENT_GRAPHICS_BACKEND,
            GRAPHICS_BACKEND_MESSAGE_LEN as u16,
        ),
        BackendError::InvalidWireObject,
    ));
    cases.push((
        "wrong opcode",
        with_header(
            &good,
            EVENT_OBJECT_ID,
            EVENT_MOVE,
            GRAPHICS_BACKEND_MESSAGE_LEN as u16,
        ),
        BackendError::InvalidWireOpcode,
    ));

    let default = GraphicsBackendCapabilities::software_default();
    let mut host = Headless::new(2, 1);
    for (name, bytes, expected) in &cases {
        assert_eq!(
            decode_graphics_backend_event(bytes),
            Err(*expected),
            "{name} must be rejected with a typed error"
        );
        assert!(decode_event(bytes).is_err(), "{name}");
        assert!(decode_batch(bytes, 1).is_err(), "{name}");
        assert!(host.apply_wire_bytes(bytes).is_err(), "{name}");
        assert_eq!(
            host.graphics_capabilities(),
            default,
            "{name} mutated state"
        );
    }

    let long = with_header(
        &good,
        EVENT_OBJECT_ID,
        EVENT_GRAPHICS_BACKEND,
        (GRAPHICS_BACKEND_MESSAGE_LEN + 4) as u16,
    );
    assert_eq!(decode_graphics_backend_event(&long), Ok(None));
    assert_eq!(decode_event(&long), Ok(None));
    assert!(decode_batch(&long, 1).is_err());
    for cut in 0..good.len() {
        let prefix = &good[..cut];
        assert_eq!(decode_graphics_backend_event(prefix), Ok(None), "cut {cut}");
        assert_eq!(decode_event(prefix), Ok(None), "cut {cut}");
        if cut == 0 {
            assert_eq!(decode_batch(prefix, 1), Ok(Vec::new()));
        } else {
            assert!(decode_batch(prefix, 1).is_err(), "cut {cut}");
            assert!(host.apply_wire_bytes(prefix).is_err(), "cut {cut}");
        }
        assert_eq!(host.graphics_capabilities(), default, "cut {cut}");
    }
}

#[test]
fn headless_and_platform_adapter_expose_typed_backend_state() {
    let mut host = Headless::new(2, 1);
    let default = GraphicsBackendCapabilities::software_default();
    assert_eq!(host.graphics_backend(), GraphicsBackend::Software);
    assert_eq!(host.graphics_capabilities(), default);
    let adapter: &dyn PlatformAdapter = &host;
    assert_eq!(adapter.graphics_backend(), GraphicsBackend::Software);
    assert_eq!(adapter.graphics_capabilities(), default);

    let terminal = Terminal::with_defaults();
    assert_eq!(terminal.graphics_backend(), GraphicsBackend::Software);
    assert_eq!(terminal.graphics_capabilities(), default);

    let descriptor = HostDescriptor::new(
        HostOs::Windows,
        [GraphicsBackend::AngleGlEs, GraphicsBackend::D3D12],
        [GraphicsBackend::D3D12],
    );
    let selection = select_backend_with_reason(&descriptor, None).unwrap();
    assert_eq!(selection.backend, GraphicsBackend::D3D12);
    let capabilities = selection.capabilities(descriptor.os);
    let event = WireEvent::GraphicsBackend(capabilities);
    let before_viewport = host.viewport();
    let before_layout = host.layout();
    assert_eq!(
        host.apply_wire_bytes(&event.encode().unwrap()).unwrap(),
        Some(event)
    );
    assert_eq!(host.graphics_backend(), GraphicsBackend::D3D12);
    assert_eq!(host.graphics_capabilities(), capabilities);
    let adapter: &dyn PlatformAdapter = &host;
    assert_eq!(adapter.graphics_backend(), GraphicsBackend::D3D12);
    assert_eq!(adapter.graphics_capabilities(), capabilities);
    assert_eq!(host.viewport(), before_viewport);
    assert_eq!(host.layout(), before_layout);
    assert_eq!(host.poll_event(), None);

    let mut inconsistent = capabilities;
    inconsistent.capabilities.max_texture_size = 0;
    assert_eq!(
        inconsistent.validate(),
        Err(BackendError::InvalidTextureSize(0))
    );
    assert!(host
        .apply_wire_event(WireEvent::GraphicsBackend(inconsistent))
        .is_err());
    assert_eq!(host.graphics_capabilities(), capabilities);
}
