//! Headless conformance for fold-aware display geometry and its wire event.

use opurom_platform::wayland_wire::{
    decode_batch, decode_event, decode_geometry_event, encode_geometry_event, header, WireEvent,
    DISPLAY_ID, EVENT_GEOMETRY, EVENT_MOVE, EVENT_OBJECT_ID, GEOMETRY_BODY_LEN,
    GEOMETRY_MESSAGE_LEN, HEADER_LEN,
};
use opurom_platform::{
    resolve_layout, DisplayGeometry, FoldRegion, GeometryError, LayoutTier, Posture, WindowRole,
    COMPACT_LIMIT, DEFAULT_FOLD_WIDTH, DEFAULT_SPLIT_RATIO, DPI_UNIT, EXPANDED_LIMIT,
    HINGE_GUTTER_BONUS, MAX_DPI_SCALE, MAX_GUTTER, MAX_HINGE_ANGLE, MAX_SPLIT_RATIO, MEDIUM_LIMIT,
    MIN_DPI_SCALE, MIN_SPLIT_RATIO, SPLIT_RATIO_UNIT, WIDE_LIMIT,
};

fn geometry(
    width: u32,
    height: u32,
    posture: Posture,
    role: WindowRole,
    fold: Option<FoldRegion>,
) -> DisplayGeometry {
    DisplayGeometry::new(width, height, DPI_UNIT, posture, role, fold).unwrap()
}

fn hinged(width: u32, role: WindowRole) -> DisplayGeometry {
    let fold_width = DEFAULT_FOLD_WIDTH.min(width);
    let fold = FoldRegion {
        x: (width - fold_width) / 2,
        width: fold_width,
    };
    geometry(width, 600, Posture::Hinge { angle: 90 }, role, Some(fold))
}

fn framed_geometry() -> (DisplayGeometry, Vec<u8>) {
    let value = DisplayGeometry::new(
        1200,
        800,
        2 * DPI_UNIT,
        Posture::Hinge { angle: 90 },
        WindowRole::Split,
        Some(FoldRegion { x: 588, width: 24 }),
    )
    .unwrap();
    let bytes = encode_geometry_event(value).unwrap();
    (value, bytes)
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
fn geometry_validation_rejects_invalid_values_with_typed_errors() {
    let base = geometry(320, 240, Posture::Flat, WindowRole::Primary, None);
    let invalid = [
        (
            DisplayGeometry { width: 0, ..base },
            GeometryError::NonPositiveDimension,
        ),
        (
            DisplayGeometry { height: 0, ..base },
            GeometryError::NonPositiveDimension,
        ),
        (
            DisplayGeometry {
                dpi_scale: MIN_DPI_SCALE - 1,
                ..base
            },
            GeometryError::DpiScaleOutOfRange,
        ),
        (
            DisplayGeometry {
                dpi_scale: MAX_DPI_SCALE + 1,
                ..base
            },
            GeometryError::DpiScaleOutOfRange,
        ),
        (
            DisplayGeometry {
                posture: Posture::Hinge { angle: -1 },
                ..base
            },
            GeometryError::HingeAngleOutOfRange,
        ),
        (
            DisplayGeometry {
                posture: Posture::Hinge {
                    angle: MAX_HINGE_ANGLE + 1,
                },
                ..base
            },
            GeometryError::HingeAngleOutOfRange,
        ),
        (
            DisplayGeometry {
                fold: Some(FoldRegion { x: 0, width: 0 }),
                ..base
            },
            GeometryError::EmptyFoldRegion,
        ),
        (
            DisplayGeometry {
                fold: Some(FoldRegion { x: 320, width: 1 }),
                ..base
            },
            GeometryError::FoldOutsideWidth,
        ),
        (
            DisplayGeometry {
                fold: Some(FoldRegion {
                    x: u32::MAX,
                    width: 2,
                }),
                ..base
            },
            GeometryError::FoldOutsideWidth,
        ),
    ];
    for (value, expected) in invalid {
        assert_eq!(value.validate(), Err(expected), "{value:?}");
        assert_eq!(encode_geometry_event(value), Err(expected));
        assert!(WireEvent::Geometry(value).encode().is_err());
    }

    assert!(DisplayGeometry::new(
        1,
        1,
        MIN_DPI_SCALE,
        Posture::Hinge { angle: 0 },
        WindowRole::Split,
        Some(FoldRegion { x: 0, width: 1 }),
    )
    .is_ok());
    assert!(DisplayGeometry::new(
        4096,
        1,
        MAX_DPI_SCALE,
        Posture::Hinge {
            angle: MAX_HINGE_ANGLE
        },
        WindowRole::Secondary,
        Some(FoldRegion { x: 4095, width: 1 }),
    )
    .is_ok());
    for posture in [Posture::Flat, Posture::HalfOpened, Posture::Book] {
        let value = geometry(320, 240, posture, WindowRole::Primary, None);
        assert_eq!(value.validate(), Ok(()));
        assert!(!value.has_hinge());
        assert_eq!(resolve_layout(value).avoid_fold, None);
    }
    let hinge_without_fold = geometry(
        1000,
        600,
        Posture::Hinge { angle: 90 },
        WindowRole::Split,
        None,
    );
    assert!(hinge_without_fold.has_hinge());
    assert_eq!(
        hinge_without_fold.hinge_region(),
        Some(FoldRegion {
            x: (1000 - DEFAULT_FOLD_WIDTH) / 2,
            width: DEFAULT_FOLD_WIDTH,
        })
    );
    assert!(resolve_layout(hinge_without_fold).avoid_fold.is_some());

    // `resolve_layout` stays total even for an unvalidated struct literal.
    let raw = DisplayGeometry {
        width: 0,
        height: 0,
        dpi_scale: 0,
        posture: Posture::Hinge { angle: 999 },
        window_role: WindowRole::Split,
        fold: Some(FoldRegion {
            x: u32::MAX,
            width: u32::MAX,
        }),
    };
    let _ = resolve_layout(raw);
}

#[test]
fn layout_tier_boundaries_match_the_five_documented_breakpoints() {
    assert_eq!(
        (COMPACT_LIMIT, MEDIUM_LIMIT, EXPANDED_LIMIT, WIDE_LIMIT),
        (600, 900, 1200, 1600)
    );
    let cases = [
        (0, LayoutTier::Compact),
        (1, LayoutTier::Compact),
        (599, LayoutTier::Compact),
        (600, LayoutTier::Medium),
        (899, LayoutTier::Medium),
        (900, LayoutTier::Expanded),
        (1199, LayoutTier::Expanded),
        (1200, LayoutTier::Wide),
        (1599, LayoutTier::Wide),
        (1600, LayoutTier::Ultra),
        (4096, LayoutTier::Ultra),
        (u32::MAX, LayoutTier::Ultra),
    ];
    for (width, tier) in cases {
        assert_eq!(LayoutTier::from_width(width), tier, "width {width}");
        let decision = resolve_layout(geometry(
            width.max(1),
            1,
            Posture::Flat,
            WindowRole::Primary,
            None,
        ));
        assert_eq!(decision.tier, tier);
        assert_eq!(decision.columns, tier.columns());
        assert_eq!(decision.gutter, tier.gutter());
        assert_eq!(decision.avoid_fold, None);
        assert_eq!(decision.split_ratio, 0);
    }
    assert_eq!(LayoutTier::Ultra.as_str(), "ultra");
}

#[test]
fn fold_avoidance_covers_the_hinge_across_all_five_tiers() {
    let widths = [
        1,
        320,
        599,
        600,
        899,
        900,
        1199,
        1200,
        1599,
        1600,
        4096,
        u32::MAX,
    ];
    for width in widths {
        let tier = LayoutTier::from_width(width);
        let value = hinged(width, WindowRole::Split);
        let fold = value.fold.unwrap();
        let without_hinge = resolve_layout(DisplayGeometry {
            posture: Posture::Flat,
            window_role: WindowRole::Primary,
            fold: None,
            ..value
        });
        let decision = resolve_layout(value);
        assert_eq!(decision.tier, tier, "width {width}");
        assert_eq!(decision.columns, tier.columns());
        assert_eq!(without_hinge.avoid_fold, None);
        assert_eq!(
            decision.gutter,
            (tier.gutter() + HINGE_GUTTER_BONUS).min(MAX_GUTTER)
        );
        assert!(decision.gutter > without_hinge.gutter);
        let rect = decision.avoid_fold.expect("a hinge avoids the fold");
        let start = u64::from(rect.x);
        let end = start + u64::from(rect.width);
        let fold_start = u64::from(fold.x);
        let fold_end = fold_start + u64::from(fold.width);
        assert_eq!(rect.y, 0);
        assert_eq!(rect.height, value.height);
        assert!(start <= fold_start, "width {width}");
        assert!(end >= fold_end, "width {width}");
        assert!(end <= u64::from(width), "width {width}");
        assert_eq!(
            resolve_layout(value),
            decision,
            "width {width} is deterministic"
        );
    }
}

#[test]
fn split_ratio_is_deterministic_and_fixed_point() {
    let postures = [
        Posture::Flat,
        Posture::HalfOpened,
        Posture::Book,
        Posture::Hinge { angle: 0 },
        Posture::Hinge { angle: 90 },
        Posture::Hinge {
            angle: MAX_HINGE_ANGLE,
        },
    ];
    let roles = [
        WindowRole::Primary,
        WindowRole::Secondary,
        WindowRole::Split,
    ];
    for width in [1, 320, 600, 900, 1200, 1600, 4096] {
        for posture in postures {
            for role in roles {
                let fold = (width > 2 * DEFAULT_FOLD_WIDTH).then_some(FoldRegion {
                    x: width / 3,
                    width: DEFAULT_FOLD_WIDTH,
                });
                let value = geometry(width, 600, posture, role, fold);
                let decision = resolve_layout(value);
                assert_eq!(resolve_layout(value), decision, "{value:?}");
                assert!(decision.split_ratio <= SPLIT_RATIO_UNIT);
                if matches!(role, WindowRole::Split) {
                    assert!(decision.split_ratio >= MIN_SPLIT_RATIO);
                    assert!(decision.split_ratio <= MAX_SPLIT_RATIO);
                } else {
                    assert_eq!(decision.split_ratio, 0, "{value:?}");
                }
            }
        }
    }

    let fold = FoldRegion { x: 400, width: 24 };
    let aligned = geometry(1000, 600, Posture::Flat, WindowRole::Split, Some(fold));
    let expected = ((u64::from(fold.x) + u64::from(fold.width) / 2) * u64::from(SPLIT_RATIO_UNIT)
        / 1000) as u32;
    assert_eq!(resolve_layout(aligned).split_ratio, expected);
    let centered_hinge = geometry(
        1000,
        600,
        Posture::Hinge { angle: 90 },
        WindowRole::Split,
        None,
    );
    assert_eq!(
        resolve_layout(centered_hinge).split_ratio,
        DEFAULT_SPLIT_RATIO
    );
}

#[test]
fn geometry_events_round_trip_for_every_posture_and_role() {
    let postures = [
        Posture::Flat,
        Posture::HalfOpened,
        Posture::Book,
        Posture::Hinge { angle: 0 },
        Posture::Hinge { angle: 90 },
        Posture::Hinge {
            angle: MAX_HINGE_ANGLE,
        },
    ];
    let roles = [
        WindowRole::Primary,
        WindowRole::Secondary,
        WindowRole::Split,
    ];
    for (index, posture) in postures.into_iter().enumerate() {
        for role in roles {
            let offset = index as u32;
            let fold = (index % 2 == 0).then_some(FoldRegion {
                x: 40 + offset,
                width: 20,
            });
            let value =
                DisplayGeometry::new(1200 + offset, 800, DPI_UNIT + offset, posture, role, fold)
                    .unwrap();
            let event = WireEvent::Geometry(value);
            let bytes = event.encode().unwrap();
            assert_eq!(bytes.len(), GEOMETRY_MESSAGE_LEN);
            assert_eq!(encode_geometry_event(value).unwrap(), bytes);
            let (decoded_event, used) = decode_event(&bytes).unwrap().unwrap();
            assert_eq!(decoded_event, event, "{value:?}");
            assert_eq!(used, bytes.len());
            let (decoded_geometry, used) = decode_geometry_event(&bytes).unwrap().unwrap();
            assert_eq!(decoded_geometry, value);
            assert_eq!(used, bytes.len());
        }
    }
}

#[test]
fn geometry_event_framing_is_fixed() {
    assert_eq!(GEOMETRY_BODY_LEN, 36);
    assert_eq!(GEOMETRY_MESSAGE_LEN, 44);
    let value = DisplayGeometry::new(
        1440,
        900,
        DPI_UNIT,
        Posture::Hinge { angle: 90 },
        WindowRole::Split,
        Some(FoldRegion { x: 700, width: 40 }),
    )
    .unwrap();
    let bytes = WireEvent::Geometry(value).encode().unwrap();
    let little = [
        0x00, 0x00, 0x00, 0xff, 0x05, 0x00, 0x2c, 0x00, // object, size 44, opcode 5
        0xa0, 0x05, 0x00, 0x00, // width 1440
        0x84, 0x03, 0x00, 0x00, // height 900
        0x00, 0x04, 0x00, 0x00, // DPI 1.0
        0x03, 0x00, 0x00, 0x00, // posture Hinge
        0x5a, 0x00, 0x00, 0x00, // angle 90
        0x02, 0x00, 0x00, 0x00, // role Split
        0xbc, 0x02, 0x00, 0x00, // fold x 700
        0x28, 0x00, 0x00, 0x00, // fold width 40
        0x01, 0x00, 0x00, 0x00, // fold flag
    ];
    let big = [
        0xff, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x05, // object, size 44, opcode 5
        0x00, 0x00, 0x05, 0xa0, // width 1440
        0x00, 0x00, 0x03, 0x84, // height 900
        0x00, 0x00, 0x04, 0x00, // DPI 1.0
        0x00, 0x00, 0x00, 0x03, // posture Hinge
        0x00, 0x00, 0x00, 0x5a, // angle 90
        0x00, 0x00, 0x00, 0x02, // role Split
        0x00, 0x00, 0x02, 0xbc, // fold x 700
        0x00, 0x00, 0x00, 0x28, // fold width 40
        0x00, 0x00, 0x00, 0x01, // fold flag
    ];
    assert_eq!(bytes, native(&little, &big));
    assert_eq!(decode_geometry_event(&bytes).unwrap().unwrap().0, value);
}

#[test]
fn malformed_geometry_wire_is_rejected_without_panics() {
    let (good_geometry, good) = framed_geometry();
    assert_eq!(
        decode_geometry_event(&good).unwrap().unwrap().0,
        good_geometry
    );

    let mut cases: Vec<(&str, Vec<u8>, GeometryError)> = Vec::new();
    let mut bytes = good.clone();
    set_word(&mut bytes, 8, 2);
    cases.push(("unknown flag", bytes, GeometryError::InvalidWireFlags));
    let mut bytes = good.clone();
    set_word(&mut bytes, 8, 0);
    cases.push((
        "fold fields without flag",
        bytes,
        GeometryError::InvalidWireFlags,
    ));
    let mut bytes = good.clone();
    set_word(&mut bytes, 3, 9);
    cases.push(("unknown posture", bytes, GeometryError::InvalidPostureTag));
    let mut bytes = good.clone();
    set_word(&mut bytes, 5, 9);
    cases.push(("unknown role", bytes, GeometryError::InvalidRoleTag));
    let mut bytes = good.clone();
    set_word(&mut bytes, 3, 0);
    set_word(&mut bytes, 4, 90);
    cases.push((
        "angle without hinge",
        bytes,
        GeometryError::UnexpectedHingeAngle,
    ));
    let mut bytes = good.clone();
    set_word(&mut bytes, 4, 181);
    cases.push((
        "angle above range",
        bytes,
        GeometryError::HingeAngleOutOfRange,
    ));
    let mut bytes = good.clone();
    set_word(&mut bytes, 4, u32::MAX);
    cases.push(("angle overflow", bytes, GeometryError::HingeAngleOutOfRange));
    let mut bytes = good.clone();
    set_word(&mut bytes, 0, 0);
    cases.push(("zero width", bytes, GeometryError::NonPositiveDimension));
    let mut bytes = good.clone();
    set_word(&mut bytes, 1, 0);
    cases.push(("zero height", bytes, GeometryError::NonPositiveDimension));
    let mut bytes = good.clone();
    set_word(&mut bytes, 2, MIN_DPI_SCALE - 1);
    cases.push(("low DPI", bytes, GeometryError::DpiScaleOutOfRange));
    let mut bytes = good.clone();
    set_word(&mut bytes, 2, MAX_DPI_SCALE + 1);
    cases.push(("high DPI", bytes, GeometryError::DpiScaleOutOfRange));
    let mut bytes = good.clone();
    set_word(&mut bytes, 7, 0);
    cases.push(("empty fold", bytes, GeometryError::EmptyFoldRegion));
    let mut bytes = good.clone();
    set_word(&mut bytes, 6, 1200);
    cases.push(("fold outside width", bytes, GeometryError::FoldOutsideWidth));
    let mut bytes = good.clone();
    set_word(&mut bytes, 6, u32::MAX);
    cases.push((
        "fold addition overflow",
        bytes,
        GeometryError::FoldOutsideWidth,
    ));
    cases.push((
        "short body",
        with_header(&good, EVENT_OBJECT_ID, EVENT_GEOMETRY, 40),
        GeometryError::InvalidWireLength,
    ));
    cases.push((
        "misaligned header",
        with_header(&good, EVENT_OBJECT_ID, EVENT_GEOMETRY, 9),
        GeometryError::InvalidWireHeader,
    ));
    cases.push((
        "wrong object",
        with_header(
            &good,
            DISPLAY_ID,
            EVENT_GEOMETRY,
            GEOMETRY_MESSAGE_LEN as u16,
        ),
        GeometryError::InvalidWireObject,
    ));
    cases.push((
        "wrong opcode",
        with_header(
            &good,
            EVENT_OBJECT_ID,
            EVENT_MOVE,
            GEOMETRY_MESSAGE_LEN as u16,
        ),
        GeometryError::InvalidWireOpcode,
    ));

    for (name, bytes, expected) in cases {
        assert_eq!(
            decode_geometry_event(&bytes),
            Err(expected),
            "{name} must be rejected with a typed error"
        );
        assert!(
            decode_event(&bytes).is_err(),
            "{name} must not decode as an event"
        );
        assert!(
            decode_batch(&bytes, 4).is_err(),
            "{name} must not decode in a batch"
        );
    }
    assert!(GeometryError::InvalidWireFlags
        .to_string()
        .contains("flags"));
    assert!(GeometryError::InvalidWireLength
        .to_string()
        .contains("length"));

    // A claimed size beyond the buffer is a partial stream, and every prefix
    // of a valid message is read without indexing past the end.
    let long = with_header(&good, EVENT_OBJECT_ID, EVENT_GEOMETRY, 48);
    assert_eq!(decode_geometry_event(&long), Ok(None));
    assert_eq!(decode_event(&long), Ok(None));
    assert!(decode_batch(&long, 4).is_err());
    for cut in 0..good.len() {
        let prefix = &good[..cut];
        assert_eq!(decode_geometry_event(prefix), Ok(None), "cut {cut}");
        assert_eq!(decode_event(prefix), Ok(None), "cut {cut}");
        if cut == 0 {
            assert_eq!(decode_batch(prefix, 1), Ok(Vec::new()));
        } else {
            assert!(
                decode_batch(prefix, 1).is_err(),
                "cut {cut} is a partial message"
            );
        }
    }
}

#[test]
fn geometry_events_mix_with_existing_wire_events_in_a_batch() {
    let (first, _) = framed_geometry();
    let second = hinged(1600, WindowRole::Secondary);
    let cases = vec![
        WireEvent::Resize {
            width: 320,
            height: 240,
        },
        WireEvent::Geometry(first),
        WireEvent::Focus { focused: true },
        WireEvent::Geometry(second),
    ];
    let mut bytes = Vec::new();
    for event in &cases {
        bytes.extend_from_slice(&event.encode().unwrap());
    }
    assert_eq!(bytes.len(), 16 * 2 + GEOMETRY_MESSAGE_LEN * 2);
    assert_eq!(decode_batch(&bytes, cases.len()), Ok(cases.clone()));
    assert!(decode_batch(&bytes, cases.len() - 1).is_err());
}
