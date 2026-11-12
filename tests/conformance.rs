use opurom_platform::{
    all_platforms, registry, spec_by_name, AdapterError, Capabilities, FrameView, Headless,
    InputEvent, Lifecycle, PlatformAdapter, Size, Status, HEADLESS_SPEC,
};

/// The 14 target names required by ADAPTER_CONTRACT.md.
const REQUIRED: [&str; 14] = [
    "android",
    "ios",
    "harmony-mobile",
    "harmony-pc",
    "windows",
    "macos",
    "linux",
    "wechat",
    "alipay",
    "qq",
    "douyin",
    "baidu",
    "kuaishou",
    "web",
];

fn frame<'a>(width: u32, height: u32, pixels: &'a [u8]) -> FrameView<'a> {
    FrameView {
        width,
        height,
        pixels,
    }
}

/// Mock adapter that returns `Err` three times in a row: the exact condition that
/// makes the kernel degrade to the headless oracle (Fault isolation, ADAPTER_CONTRACT.md).
struct Flaky {
    errors_left: u32,
}

impl PlatformAdapter for Flaky {
    fn name(&self) -> &str {
        "flaky"
    }

    fn capabilities(&self) -> Capabilities {
        HEADLESS_SPEC.capabilities
    }

    fn viewport(&self) -> Size {
        Size {
            width: 1,
            height: 1,
        }
    }

    fn poll_event(&mut self) -> Option<InputEvent> {
        None
    }

    fn present(&mut self, _frame: FrameView<'_>) -> Result<(), AdapterError> {
        if self.errors_left > 0 {
            self.errors_left -= 1;
            Err(AdapterError::Backend("mock backend failure"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn three_consecutive_errors_degrade_to_headless() {
    let pixels = [7u8; 4]; // 1x1 RGBA8
    let mut adapter: Box<dyn PlatformAdapter> = Box::new(Flaky { errors_left: 3 });

    let mut consecutive_errors = 0;
    for _ in 0..3 {
        assert!(adapter.present(frame(1, 1, &pixels)).is_err());
        consecutive_errors += 1;
    }
    assert_eq!(
        consecutive_errors, 3,
        "three consecutive Errs are representable"
    );
    assert_eq!(adapter.name(), "flaky");

    // The kernel swaps in the verified headless adapter; the app keeps running.
    let mut fallback = Headless::new(1, 1);
    assert_eq!(fallback.present(frame(1, 1, &pixels)), Ok(()));
    fallback.push_event(InputEvent::Lifecycle(Lifecycle::Resumed));
    assert_eq!(
        fallback.poll_event(),
        Some(InputEvent::Lifecycle(Lifecycle::Resumed))
    );
}

#[test]
fn headless_present_and_digest_are_deterministic() {
    assert_eq!(
        Headless::new(1, 1).digest(),
        "cbf29ce484222325",
        "FNV-1a offset basis over an empty buffer"
    );

    let pixels = [1u8, 2, 3, 4, 5, 6, 7, 8]; // 2x1 RGBA8
    let mut host = Headless::new(2, 1);
    host.present(frame(2, 1, &pixels)).unwrap();
    let digest = host.digest().to_string();
    assert_eq!(digest.len(), 16);
    assert_eq!(host.frame().pixels, &pixels);
    assert_eq!(host.frame().width, 2);
    assert_eq!(host.frame().height, 1);

    // Same bytes => same digest, even on a fresh adapter and after a repeat present.
    host.present(frame(2, 1, &pixels)).unwrap();
    assert_eq!(host.digest(), digest);
    let mut other = Headless::new(2, 1);
    other.present(frame(2, 1, &pixels)).unwrap();
    assert_eq!(other.digest(), digest);

    // A single changed byte => different digest.
    let mut changed = pixels;
    changed[3] ^= 0x80;
    host.present(frame(2, 1, &changed)).unwrap();
    assert_ne!(host.digest(), digest);
}

#[test]
fn headless_events_drain_fifo_including_resize() {
    let mut host = Headless::new(4, 4);
    host.push_event(InputEvent::Tap {
        x: 1.0,
        y: 2.0,
        node_id: 3,
    });
    host.push_event(InputEvent::Input {
        node_id: 3,
        text: "hi".to_string(),
    });
    host.push_event(InputEvent::Key {
        code: 13,
        pressed: true,
    });
    host.push_event(InputEvent::Lifecycle(Lifecycle::Resumed));
    host.resize(Size {
        width: 8,
        height: 6,
    });

    assert_eq!(
        host.viewport(),
        Size {
            width: 8,
            height: 6
        }
    );
    assert_eq!(
        host.poll_event(),
        Some(InputEvent::Tap {
            x: 1.0,
            y: 2.0,
            node_id: 3
        })
    );
    assert_eq!(
        host.poll_event(),
        Some(InputEvent::Input {
            node_id: 3,
            text: "hi".to_string()
        })
    );
    assert_eq!(
        host.poll_event(),
        Some(InputEvent::Key {
            code: 13,
            pressed: true
        })
    );
    assert_eq!(
        host.poll_event(),
        Some(InputEvent::Lifecycle(Lifecycle::Resumed))
    );
    assert_eq!(
        host.poll_event(),
        Some(InputEvent::Resize {
            size: Size {
                width: 8,
                height: 6
            }
        })
    );
    assert_eq!(host.poll_event(), None, "queue is fully drained");
}

#[test]
fn headless_rejects_wrong_frame_length() {
    let mut host = Headless::new(2, 1);
    let before = host.digest().to_string();
    assert_eq!(
        host.present(frame(2, 1, &[0u8; 7])),
        Err(AdapterError::InvalidFrame)
    );
    assert_eq!(
        host.present(frame(2, 1, &[0u8; 9])),
        Err(AdapterError::InvalidFrame)
    );
    assert_eq!(
        host.digest(),
        before,
        "a rejected frame never mutates state"
    );
    assert_eq!(host.frame().pixels, &[]);

    assert_eq!(host.present(frame(2, 1, &[0u8; 8])), Ok(()));
    assert_ne!(host.digest(), before);
}

#[test]
fn registry_has_14_unique_entries_and_one_verified() {
    let specs = registry();
    assert_eq!(specs.len(), 14);

    let names: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 14, "all 14 registry names are unique");
    for name in REQUIRED {
        assert_eq!(
            names.iter().filter(|candidate| **candidate == name).count(),
            1,
            "{name} must appear exactly once"
        );
    }
    assert!(
        specs
            .iter()
            .all(|spec| matches!(spec.status, Status::Scaffold | Status::Planned)),
        "no target row may claim Verified in v0.1"
    );

    // The whole platform set has exactly one Verified platform: headless.
    let all = all_platforms();
    assert_eq!(all.len(), 15);
    let verified: Vec<&str> = all
        .iter()
        .filter(|spec| spec.status == Status::Verified)
        .map(|spec| spec.name)
        .collect();
    assert_eq!(verified, vec!["headless"]);
}

#[test]
fn registry_is_queryable_by_name() {
    for name in REQUIRED {
        assert_eq!(spec_by_name(name).unwrap().name, name);
    }
    assert_eq!(spec_by_name("linux").unwrap().emitter, "native");
    assert_eq!(spec_by_name("headless").unwrap().status, Status::Verified);
    assert!(spec_by_name("no-such-platform").is_none());
}

#[test]
fn capabilities_are_sane() {
    for spec in all_platforms() {
        assert!(
            spec.capabilities.dpi_scale.is_finite() && spec.capabilities.dpi_scale > 0.0,
            "{} has a non-positive dpi_scale",
            spec.name
        );
        assert!(
            !spec.capabilities.gpu,
            "{}: GPU presentation is a later round",
            spec.name
        );
        assert!(
            !spec.capabilities.native_embed,
            "{}: native widget embedding is a later round",
            spec.name
        );
    }
}
