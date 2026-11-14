//! End-to-end viewport state for plain resizes, geometry events and patches.

use opurom_platform::wayland_wire::{
    decode_batch, decode_event, decode_geometry_event, decode_layout_patch_event,
    encode_geometry_event, header, WireEvent, EVENT_LAYOUT_PATCH, EVENT_OBJECT_ID, HEADER_LEN,
    LAYOUT_PATCH_MESSAGE_LEN,
};
use opurom_platform::{
    layout_patch, resolve_layout, DisplayGeometry, FoldRegion, FrameView, GeometryError, Headless,
    InputEvent, LayoutPatch, PlatformAdapter, Posture, Rect, Size, WindowRole, WindowSet, DPI_UNIT,
};

fn size(width: u32, height: u32) -> Size {
    Size { width, height }
}

fn hinged(width: u32, height: u32, fold_x: u32) -> DisplayGeometry {
    DisplayGeometry::new(
        width,
        height,
        DPI_UNIT,
        Posture::Hinge { angle: 90 },
        WindowRole::Split,
        Some(FoldRegion {
            x: fold_x,
            width: 24,
        }),
    )
    .unwrap()
}

fn frame<'a>(width: u32, height: u32, pixels: &'a [u8]) -> FrameView<'a> {
    FrameView {
        width,
        height,
        pixels,
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

fn assert_outside_fold(content: Rect, avoid: Rect) {
    let end = content.x.saturating_add(content.width);
    let avoid_end = avoid.x.saturating_add(avoid.width);
    assert!(
        end <= avoid.x || content.x >= avoid_end,
        "content {content:?} straddles the fold {avoid:?}"
    );
}

#[test]
fn geometry_event_updates_the_decision_and_all_window_viewports() {
    let geometry = hinged(800, 4, 200);
    let decision = resolve_layout(geometry);
    let bytes = WireEvent::Geometry(geometry).encode().unwrap();
    let mut windows = WindowSet::new([size(2, 1), size(3, 1)]).unwrap();

    assert_eq!(
        windows.apply_wire_batch(&bytes, 1).unwrap(),
        vec![WireEvent::Geometry(geometry)]
    );
    assert_eq!(windows.viewports(), vec![size(800, 4); 2]);
    let layouts = windows.layouts();
    assert_eq!(layouts.len(), 2);
    for (index, layout) in layouts.iter().enumerate() {
        assert_eq!(layout.geometry, geometry);
        assert_eq!(layout.frame, size(800, 4));
        assert_eq!(layout.decision, decision);
        assert_outside_fold(layout.content, decision.avoid_fold.unwrap());
        assert_eq!(windows.layout(index).unwrap(), *layout);
        assert_eq!(windows.content_viewport(index).unwrap(), layout.content);
        assert_eq!(
            windows.poll_event(index),
            Ok(Some(InputEvent::Resize { size: size(800, 4) }))
        );
    }
    assert_eq!(windows.content_viewports(), vec![layouts[0].content; 2]);

    // Layout metadata never changes the raw RGBA8 framebuffer dimensions.
    let pixels = vec![0u8; (800 * 4 * 4) as usize];
    windows.present(0, frame(800, 4, &pixels)).unwrap();
    let stored = windows.frame(0).unwrap();
    assert_eq!((stored.width, stored.height), (800, 4));
}

#[test]
fn plain_resize_keeps_legacy_state_and_clears_a_previous_fold() {
    let resize = WireEvent::Resize {
        width: 7,
        height: 5,
    };
    let bytes = resize.encode().unwrap();
    assert_eq!(bytes.len(), 16);
    assert_eq!(decode_event(&bytes).unwrap(), Some((resize, bytes.len())));

    let mut decoded = Headless::new(2, 1);
    let mut direct = Headless::new(2, 1);
    assert_eq!(decoded.apply_wire_bytes(&bytes).unwrap(), Some(resize));
    direct.resize(size(7, 5));
    assert_eq!(decoded.viewport(), direct.viewport());
    assert_eq!(decoded.layout(), direct.layout());
    assert_eq!(decoded.content_viewport(), direct.content_viewport());
    assert_eq!(decoded.poll_event(), direct.poll_event());
    assert_eq!(decoded.layout(), LayoutPatch::from_size(size(7, 5)));
    assert_eq!(decoded.layout().decision.avoid_fold, None);
    assert_eq!(decoded.layout().decision.split_ratio, 0);

    let generic: &dyn PlatformAdapter = &decoded;
    assert_eq!(generic.layout(), decoded.layout());
    assert_eq!(generic.content_viewport(), decoded.content_viewport());

    let geometry = hinged(400, 4, 100);
    decoded
        .apply_wire_bytes(&WireEvent::Geometry(geometry).encode().unwrap())
        .unwrap();
    assert!(decoded.layout().decision.avoid_fold.is_some());
    decoded.apply_wire_bytes(&bytes).unwrap();
    assert_eq!(decoded.layout(), LayoutPatch::from_size(size(7, 5)));
    assert_eq!(decoded.viewport(), size(7, 5));
    assert_eq!(
        decoded.content_viewport(),
        Rect {
            x: 0,
            y: 0,
            width: 7,
            height: 5,
        }
    );
}

#[test]
fn fold_shifts_the_content_viewport_into_the_larger_pane() {
    let geometry = hinged(400, 4, 100);
    let decision = resolve_layout(geometry);
    let avoid = decision.avoid_fold.unwrap();
    let right_start = avoid.x.saturating_add(avoid.width);
    let right_width = 400u32.saturating_sub(right_start);
    assert!(right_width > avoid.x);

    let patch = layout_patch(geometry).unwrap();
    assert_eq!(patch.decision, decision);
    assert_eq!(
        patch.content,
        Rect {
            x: right_start,
            y: 0,
            width: right_width,
            height: 4,
        }
    );
    assert_outside_fold(patch.content, avoid);

    let mut host = Headless::new(1, 1);
    host.apply_wire_event(WireEvent::LayoutPatch(patch))
        .unwrap();
    assert_eq!(host.viewport(), size(400, 4));
    assert_eq!(host.content_viewport(), patch.content);
    assert_eq!(host.layout().decision.split_ratio, decision.split_ratio);
    assert_eq!(
        host.poll_event(),
        Some(InputEvent::Resize { size: size(400, 4) })
    );
}

#[test]
fn layout_patch_round_trips_the_resolved_decision() {
    let geometry = hinged(1200, 6, 400);
    let patch = layout_patch(geometry).unwrap();
    let expected = resolve_layout(geometry);
    assert_eq!(patch.decision, expected);

    let event = WireEvent::LayoutPatch(patch);
    let bytes = event.encode().unwrap();
    assert_eq!(LAYOUT_PATCH_MESSAGE_LEN, 100);
    assert_eq!(bytes.len(), LAYOUT_PATCH_MESSAGE_LEN);
    assert_eq!(decode_event(&bytes).unwrap(), Some((event, bytes.len())));
    assert_eq!(decode_batch(&bytes, 1), Ok(vec![event]));

    let (decoded, used) = decode_layout_patch_event(&bytes).unwrap().unwrap();
    assert_eq!(decoded, patch);
    assert_eq!(used, bytes.len());
    assert_eq!(decoded.decision, expected);
    assert_eq!(decoded.decision.split_ratio, expected.split_ratio);
    assert_eq!(decoded.decision.avoid_fold, expected.avoid_fold);
}

#[test]
fn malformed_layout_patches_and_geometry_events_are_rejected() {
    let geometry = hinged(800, 4, 200);
    let patch = layout_patch(geometry).unwrap();
    let good = WireEvent::LayoutPatch(patch).encode().unwrap();
    assert_eq!(decode_layout_patch_event(&good).unwrap().unwrap().0, patch);

    let mut bad_magic = good.clone();
    set_word(&mut bad_magic, 0, 0);
    assert_eq!(
        decode_layout_patch_event(&bad_magic),
        Err(GeometryError::InvalidLayoutPatch)
    );

    let mutations = [
        ("magic", 0, 0),
        ("zero width", 1, 0),
        ("bad geometry flags", 9, 2),
        ("unknown tier", 10, 99),
        ("wrong columns", 11, 999),
        ("wrong gutter", 12, 999),
        ("wrong split ratio", 13, 999),
        ("bad avoid flag", 14, 2),
        ("missing avoid", 14, 0),
        ("wrong avoid x", 15, 0),
        ("wrong avoid y", 16, 1),
        ("wrong avoid width", 17, 0),
        ("wrong avoid height", 18, 0),
        ("wrong content x", 19, 0),
        ("wrong content y", 20, 1),
        ("wrong content width", 21, 0),
        ("wrong content height", 22, 0),
    ];
    let mut cases: Vec<(&str, Vec<u8>)> = mutations
        .into_iter()
        .map(|(name, index, value)| {
            let mut bytes = good.clone();
            set_word(&mut bytes, index, value);
            (name, bytes)
        })
        .collect();
    cases.push((
        "short body",
        with_header(
            &good,
            EVENT_OBJECT_ID,
            EVENT_LAYOUT_PATCH,
            (LAYOUT_PATCH_MESSAGE_LEN - 4) as u16,
        ),
    ));
    cases.push((
        "wrong object",
        with_header(
            &good,
            EVENT_OBJECT_ID + 1,
            EVENT_LAYOUT_PATCH,
            LAYOUT_PATCH_MESSAGE_LEN as u16,
        ),
    ));
    cases.push((
        "wrong opcode",
        with_header(
            &good,
            EVENT_OBJECT_ID,
            EVENT_LAYOUT_PATCH + 1,
            LAYOUT_PATCH_MESSAGE_LEN as u16,
        ),
    ));

    for (name, bytes) in cases {
        let mut host = Headless::new(2, 1);
        let before = host.layout();
        assert!(decode_layout_patch_event(&bytes).is_err(), "{name}");
        assert!(decode_event(&bytes).is_err(), "{name}");
        assert!(decode_batch(&bytes, 1).is_err(), "{name}");
        assert!(host.apply_wire_bytes(&bytes).is_err(), "{name}");
        assert_eq!(host.layout(), before, "{name} must not mutate state");
    }

    for cut in 0..good.len() {
        let prefix = &good[..cut];
        assert!(matches!(decode_layout_patch_event(prefix), Ok(None)));
        assert_eq!(decode_event(prefix), Ok(None));
        if cut == 0 {
            assert_eq!(decode_batch(prefix, 1), Ok(Vec::new()));
        } else {
            assert!(decode_batch(prefix, 1).is_err(), "cut {cut}");
        }
    }

    let mut bad_geometry = encode_geometry_event(geometry).unwrap();
    set_word(&mut bad_geometry, 8, 2);
    assert!(decode_geometry_event(&bad_geometry).is_err());
    assert!(decode_event(&bad_geometry).is_err());
    assert!(decode_batch(&bad_geometry, 1).is_err());
    let mut host = Headless::new(2, 1);
    let before = host.layout();
    assert!(host.apply_wire_bytes(&bad_geometry).is_err());
    assert_eq!(host.layout(), before);
}

#[test]
fn mixed_wire_batch_updates_every_window_through_one_path() {
    let geometry = hinged(800, 4, 200);
    let patch = layout_patch(geometry).unwrap();
    let events = vec![
        WireEvent::Resize {
            width: 640,
            height: 4,
        },
        WireEvent::Focus { focused: true },
        WireEvent::Geometry(geometry),
        WireEvent::LayoutPatch(patch),
    ];
    let mut bytes = Vec::new();
    for event in &events {
        bytes.extend_from_slice(&event.encode().unwrap());
    }

    let mut windows = WindowSet::new([size(1, 1); 3]).unwrap();
    assert_eq!(
        windows.apply_wire_batch(&bytes, events.len()).unwrap(),
        events
    );
    assert_eq!(windows.viewports(), vec![size(800, 4); 3]);
    assert_eq!(windows.content_viewports(), vec![patch.content; 3]);
    assert_eq!(windows.layouts(), vec![patch; 3]);
    assert_eq!(
        windows.layout(0).unwrap().decision.split_ratio,
        patch.decision.split_ratio
    );

    for index in 0..windows.len() {
        assert_eq!(
            windows.poll_event(index),
            Ok(Some(InputEvent::Resize { size: size(640, 4) }))
        );
        assert_eq!(
            windows.poll_event(index),
            Ok(Some(InputEvent::Resize { size: size(800, 4) }))
        );
        assert_eq!(
            windows.poll_event(index),
            Ok(Some(InputEvent::Resize { size: size(800, 4) }))
        );
        assert_eq!(windows.poll_event(index), Ok(None));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn appkit_host_reads_the_same_geometry_layout() {
    use opurom_platform::MacHost;

    let path =
        std::env::temp_dir().join(format!("opurom-platform-layout-{}.png", std::process::id()));
    let mut host = MacHost::new(2, 1, &path);
    let geometry = hinged(800, 4, 200);
    host.apply_wire_bytes(&WireEvent::Geometry(geometry).encode().unwrap())
        .unwrap();
    let expected = layout_patch(geometry).unwrap();
    assert_eq!(host.layout(), expected);
    assert_eq!(host.content_viewport(), expected.content);
    assert_eq!(host.viewport(), size(800, 4));
    let adapter: &dyn PlatformAdapter = &host;
    assert_eq!(adapter.layout(), expected);
    assert_eq!(adapter.content_viewport(), expected.content);
}
