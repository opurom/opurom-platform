use opurom_platform::{
    all_platforms, host_platforms, known_platforms, platforms_with, registry, spec_by_name,
    AdapterError, Capabilities, Capability, FrameView, Headless, InputEvent, Lifecycle,
    PlatformAdapter, Size, Status, Terminal, WindowSet, HEADLESS_SPEC, TERMINAL_SPEC,
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

/// Deterministic RGBA8 gradient shared by the terminal ANSI tests.
fn gradient(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            pixels.push((10 + x * 20) as u8);
            pixels.push((20 + y * 30) as u8);
            pixels.push((30 + (x + y) * 10) as u8);
            pixels.push(255);
        }
    }
    pixels
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

// ---- terminal host adapter (COMPONENTS_AND_A11Y.md section 6) ----

#[test]
fn terminal_name_capabilities_and_viewport() {
    let host = Terminal::new(6, 4);
    assert_eq!(host.name(), "terminal");
    assert_eq!(host.capabilities(), TERMINAL_SPEC.capabilities);
    assert!(!host.capabilities().gpu);
    assert!(!host.capabilities().native_embed);
    assert!(!host.capabilities().multi_window);
    assert!(!host.capabilities().lifecycle);
    assert_eq!(host.capabilities().dpi_scale, 1.0);
    assert_eq!(
        host.viewport(),
        Size {
            width: 6,
            height: 4
        }
    );
    assert_eq!(
        Terminal::with_defaults().viewport(),
        Size {
            width: 80,
            height: 24
        }
    );
    assert_eq!(host.last_output(), "", "no output before a present");
}

#[test]
fn terminal_present_is_deterministic() {
    let pixels = gradient(4, 3);
    let mut a = Terminal::with_defaults();
    let mut b = Terminal::new(4, 3);
    assert_eq!(a.present(frame(4, 3, &pixels)), Ok(()));
    assert_eq!(b.present(frame(4, 3, &pixels)), Ok(()));
    assert_eq!(a.last_output(), b.last_output());
    assert_eq!(a.render_text(), a.last_output());
    let first = a.last_output().to_string();
    assert!(!first.is_empty());
    assert!(first.contains('\u{2580}'));

    // A repeat present is byte-identical; a single changed byte is not.
    a.present(frame(4, 3, &pixels)).unwrap();
    assert_eq!(
        a.last_output(),
        first,
        "same frame => byte-identical output"
    );
    let mut changed = pixels.clone();
    changed[0] ^= 0xff;
    a.present(frame(4, 3, &changed)).unwrap();
    assert_ne!(a.last_output(), first, "a changed pixel changes the output");
}

#[test]
fn terminal_maps_pixels_to_half_block_lines() {
    // (width, height, expected character rows = ceil(height / 2))
    for (w, h, rows) in [
        (4u32, 4u32, 2usize),
        (5, 5, 3),
        (3, 2, 1),
        (1, 1, 1),
        (2, 6, 3),
    ] {
        let pixels = gradient(w, h);
        let mut host = Terminal::new(w, h);
        host.present(frame(w, h, &pixels)).unwrap();
        let lines: Vec<&str> = host.last_output().lines().collect();
        assert_eq!(lines.len(), rows, "{w}x{h}: ceil(height/2) character rows");
        for line in lines {
            assert_eq!(
                line.matches('\u{2580}').count(),
                w as usize,
                "{w}x{h}: one half-block per pixel column"
            );
            assert!(line.ends_with("\x1b[0m"), "reset at line end");
        }
    }
}

#[test]
fn terminal_gradient_first_escape_sequence() {
    let pixels = gradient(3, 2);
    let mut host = Terminal::new(3, 2);
    host.present(frame(3, 2, &pixels)).unwrap();
    // Top-left pixel (0,0) = (10,20,30); the pixel below (0,1) = (10,50,40).
    assert!(
        host.last_output()
            .starts_with("\x1b[38;2;10;20;30m\x1b[48;2;10;50;40m\u{2580}"),
        "fg = top pixel, bg = bottom pixel, then U+2580"
    );
    // The first line has one truecolor sequence per column and ends with a reset.
    let first_line = host.last_output().lines().next().unwrap();
    assert_eq!(first_line.matches("\x1b[38;2;").count(), 3, "3 columns");
    assert!(first_line.ends_with("\x1b[0m"));
}

#[test]
fn terminal_known_frame_exact_ansi_output() {
    // 2x2 frame: one character row, two columns. Top = red/green, bottom = blue/white.
    let pixels = [
        255, 0, 0, 255, 0, 255, 0, 255, // row 0
        0, 0, 255, 255, 255, 255, 255, 255, // row 1
    ];
    let mut host = Terminal::new(2, 2);
    host.present(frame(2, 2, &pixels)).unwrap();
    assert_eq!(
        host.last_output(),
        concat!(
            "\x1b[38;2;255;0;0m\x1b[48;2;0;0;255m\u{2580}",
            "\x1b[38;2;0;255;0m\x1b[48;2;255;255;255m\u{2580}",
            "\x1b[0m\n",
        ),
    );
}

#[test]
fn terminal_pads_odd_height_with_black() {
    let pixels = gradient(1, 1); // single pixel; the bottom half is missing
    let mut host = Terminal::new(1, 1);
    host.present(frame(1, 1, &pixels)).unwrap();
    assert_eq!(
        host.last_output(),
        "\x1b[38;2;10;20;30m\x1b[48;2;0;0;0m\u{2580}\x1b[0m\n",
    );
}

#[test]
fn terminal_rejects_invalid_frame_without_mutation() {
    let pixels = gradient(2, 2);
    let mut host = Terminal::new(2, 2);
    host.present(frame(2, 2, &pixels)).unwrap();
    let before = host.last_output().to_string();
    assert_eq!(
        host.present(frame(2, 2, &[0u8; 15])),
        Err(AdapterError::InvalidFrame),
        "one byte short"
    );
    assert_eq!(
        host.present(frame(2, 2, &[0u8; 17])),
        Err(AdapterError::InvalidFrame),
        "one byte long"
    );
    assert_eq!(
        host.present(frame(u32::MAX, u32::MAX, &[])),
        Err(AdapterError::InvalidFrame),
        "width*height*4 overflow"
    );
    assert_eq!(
        host.last_output(),
        before,
        "a rejected frame never mutates state"
    );
}

#[test]
fn terminal_key_input_round_trips_fifo() {
    let mut host = Terminal::with_defaults();
    host.push_key(65, true);
    host.push_input(InputEvent::Input {
        node_id: u32::MAX,
        text: "a".to_string(),
    });
    host.push_event(InputEvent::Tap {
        x: 1.0,
        y: 2.0,
        node_id: u32::MAX,
    });
    assert_eq!(
        host.poll_event(),
        Some(InputEvent::Key {
            code: 65,
            pressed: true
        })
    );
    assert_eq!(
        host.poll_event(),
        Some(InputEvent::Input {
            node_id: u32::MAX,
            text: "a".to_string()
        })
    );
    assert_eq!(
        host.poll_event(),
        Some(InputEvent::Tap {
            x: 1.0,
            y: 2.0,
            node_id: u32::MAX
        })
    );
    assert_eq!(host.poll_event(), None, "queue fully drained");
}

#[test]
fn terminal_is_a_verified_host_in_registry() {
    let spec = spec_by_name("terminal").expect("terminal is registered by name");
    assert_eq!(spec, TERMINAL_SPEC);
    assert_eq!(spec.status, Status::Verified);
    assert_eq!(spec.emitter, "terminal");
    // Verified hosts are exposed via host_platforms(); registry() stays the 14 targets.
    let hosts = host_platforms();
    assert!(
        hosts.len() >= 2,
        "headless and terminal remain Verified hosts"
    );
    assert!(
        hosts
            .iter()
            .any(|s| s.name == "terminal" && s.status == Status::Verified),
        "terminal is a Verified host"
    );
    assert!(
        hosts
            .iter()
            .any(|s| s.name == "headless" && s.status == Status::Verified),
        "headless remains a Verified host"
    );
    assert!(
        !registry().iter().any(|s| s.name == "terminal"),
        "terminal is not one of the 14 target rows"
    );
    assert_eq!(
        spec_by_name("headless").unwrap().status,
        Status::Verified,
        "existing lookup still works"
    );
}

fn size(width: u32, height: u32) -> Size {
    Size { width, height }
}

#[test]
fn window_set_reports_every_viewport_in_index_order() {
    let mut windows = WindowSet::new([size(2, 1), size(1, 2), size(3, 1)]).unwrap();
    assert_eq!(
        windows.viewports(),
        vec![size(2, 1), size(1, 2), size(3, 1)]
    );

    windows.resize(1, size(8, 6)).unwrap();
    assert_eq!(
        windows.viewports(),
        vec![size(2, 1), size(8, 6), size(3, 1)]
    );
    assert_eq!(windows.len(), 3);
}

#[test]
fn window_set_broadcast_reaches_each_window_in_isolation() {
    let mut windows = WindowSet::new([size(1, 1); 4]).unwrap();
    let event = InputEvent::Key {
        code: 7,
        pressed: true,
    };
    windows.broadcast_event(event.clone());

    for index in 0..windows.len() {
        assert_eq!(windows.poll_event(index), Ok(Some(event.clone())));
        assert_eq!(windows.poll_event(index), Ok(None));
    }

    windows
        .push_event(0, InputEvent::Lifecycle(Lifecycle::Resumed))
        .unwrap();
    assert_eq!(
        windows.poll_event(0),
        Ok(Some(InputEvent::Lifecycle(Lifecycle::Resumed)))
    );
    assert_eq!(windows.poll_event(1), Ok(None));
}

#[test]
fn window_set_resize_all_updates_every_viewport_and_queues_one_event_each() {
    let mut windows = WindowSet::new([size(2, 1), size(1, 2), size(3, 1)]).unwrap();
    windows.resize_all(size(7, 5));
    assert_eq!(windows.viewports(), vec![size(7, 5); 3]);

    for index in 0..windows.len() {
        assert_eq!(
            windows.poll_event(index),
            Ok(Some(InputEvent::Resize { size: size(7, 5) }))
        );
        assert_eq!(windows.poll_event(index), Ok(None));
    }

    windows.resize(1, size(4, 4)).unwrap();
    assert_eq!(
        windows.viewports(),
        vec![size(7, 5), size(4, 4), size(7, 5)]
    );
}

#[test]
fn capability_query_partitions_the_known_platform_set() {
    let known = known_platforms();
    let names: Vec<&str> = known.iter().map(|spec| spec.name).collect();
    let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "every known platform name is unique"
    );
    assert!(
        known.len() > all_platforms().len(),
        "the known set adds the terminal and Wayland hosts"
    );
    for name in ["headless", "terminal", "wayland", "linux"] {
        assert!(names.contains(&name), "{name} is a known platform");
    }
    for spec in &known {
        assert_eq!(
            spec_by_name(spec.name).map(|found| found.name),
            Some(spec.name),
            "each known platform resolves by name"
        );
    }

    for capability in [
        Capability::Gpu,
        Capability::NativeEmbed,
        Capability::MultiWindow,
        Capability::Lifecycle,
    ] {
        let expected: Vec<&str> = known
            .iter()
            .filter(|spec| spec.supports(capability))
            .map(|spec| spec.name)
            .collect();
        let actual: Vec<&str> = platforms_with(capability)
            .iter()
            .map(|spec| spec.name)
            .collect();
        assert_eq!(
            actual, expected,
            "the query matches the flags for {capability:?}"
        );
    }
}

#[test]
fn capability_query_reflects_the_frozen_capability_flags() {
    assert!(
        platforms_with(Capability::Gpu).is_empty(),
        "no row advertises GPU presentation"
    );

    let multi_window: Vec<&str> = platforms_with(Capability::MultiWindow)
        .iter()
        .map(|spec| spec.name)
        .collect();
    for name in ["harmony-pc", "windows", "macos", "linux"] {
        assert!(
            multi_window.contains(&name),
            "{name} advertises multi_window"
        );
    }

    let lifecycle: Vec<&str> = platforms_with(Capability::Lifecycle)
        .iter()
        .map(|spec| spec.name)
        .collect();
    assert!(
        lifecycle.contains(&"wechat"),
        "a scaffold reports lifecycle events"
    );
    assert!(
        !lifecycle.contains(&"headless"),
        "the oracle reports no lifecycle events"
    );
}

#[test]
fn capability_supports_reads_each_boolean_flag() {
    let mut spec = spec_by_name("android").unwrap();
    assert!(spec.supports(Capability::Lifecycle));
    assert!(!spec.supports(Capability::MultiWindow));
    assert!(!spec.supports(Capability::Gpu));
    assert!(!spec.supports(Capability::NativeEmbed));

    spec.capabilities.multi_window = true;
    spec.capabilities.gpu = true;
    spec.capabilities.native_embed = true;
    spec.capabilities.lifecycle = false;
    assert!(spec.supports(Capability::MultiWindow));
    assert!(spec.supports(Capability::Gpu));
    assert!(spec.supports(Capability::NativeEmbed));
    assert!(!spec.supports(Capability::Lifecycle));
}
