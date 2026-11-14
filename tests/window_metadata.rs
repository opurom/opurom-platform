use opurom_platform::{
    Headless, PlatformAdapter, Size, Terminal, WindowMetadata, HEADLESS_SPEC, TERMINAL_SPEC,
};

fn assert_metadata<T: PlatformAdapter>(host: &T, expected_title: &str) {
    let metadata = host.metadata();
    assert_eq!(metadata.title, expected_title);
    assert_eq!(metadata.dpi_scale, host.capabilities().dpi_scale);
    // The query is a pure read: asking again returns the identical answer.
    assert_eq!(metadata, host.metadata());
}

#[test]
fn headless_metadata_reports_its_title_and_dpi_scale() {
    let host = Headless::new(2, 2);
    assert_metadata(&host, "headless");
    assert_eq!(
        host.metadata(),
        WindowMetadata {
            title: "headless".to_owned(),
            dpi_scale: HEADLESS_SPEC.capabilities.dpi_scale,
        }
    );
}

#[test]
fn terminal_metadata_reports_its_title_and_dpi_scale() {
    let host = Terminal::with_defaults();
    assert_metadata(&host, "terminal");
    assert_eq!(
        host.metadata(),
        WindowMetadata {
            title: "terminal".to_owned(),
            dpi_scale: TERMINAL_SPEC.capabilities.dpi_scale,
        }
    );
}

#[test]
fn metadata_stays_stable_across_viewport_changes() {
    let mut host = Headless::new(2, 2);
    let before = host.metadata();
    host.resize(Size {
        width: 5,
        height: 4,
    });
    // A viewport change is not a window change: metadata is unchanged.
    assert_eq!(host.metadata(), before);
    assert_eq!(host.metadata().title, "headless");
}

#[cfg(target_os = "macos")]
#[test]
fn appkit_metadata_reports_its_title_and_dpi_scale() {
    use opurom_platform::MacHost;

    let path = std::env::temp_dir().join(format!(
        "opurom-platform-metadata-{}.png",
        std::process::id()
    ));
    let host = MacHost::new(2, 2, &path);
    assert_metadata(&host, "appkit");
    assert_eq!(host.metadata().title, "appkit");
}
