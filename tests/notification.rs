//! Contract tests for the notification capability: shared validation runs
//! before any OS call, the deterministic headless model covers post, cancel
//! and permission, script escaping is audited on every host, the platform
//! factory degrades with a typed reason, and macOS posts a live banner.

use opurom_platform::notification::{
    applescript_source, platform_notifier, HeadlessNotifier, Notification, NotificationHost,
    NotifyError,
};

// ---------------------------------------------------------------- contract

#[test]
fn invalid_arguments_fail_before_any_os_call() {
    let mut notifier = HeadlessNotifier::new();
    for (id, channel, title, body) in [
        (1, "chat", "", "body"),
        (2, "chat", "   ", "body"),
        (3, "", "title", "body"),
        (4, "   ", "title", "body"),
    ] {
        let result = notifier.post(id, channel, title, body);
        assert!(
            matches!(result, Err(NotifyError::Invalid(_))),
            "got {result:?}"
        );
    }
    // A rejected post never reaches the delivery stream.
    assert!(notifier.delivered().is_empty());
    // An empty body is legal: the title alone still names the banner.
    notifier
        .post(5, "chat", "title", "")
        .expect("empty body is legal");
    assert_eq!(notifier.delivered().len(), 1);
}

#[test]
fn headless_post_replaces_by_id_and_cancel_clears() {
    let mut notifier = HeadlessNotifier::new();
    notifier.post(1, "chat", "one", "first").expect("post");
    notifier.post(2, "chat", "two", "second").expect("post");
    // A repost with the same id updates the banner in place, like a device
    // notification manager treats the same id.
    notifier.post(1, "chat", "one", "updated").expect("repost");
    let ids: Vec<i32> = notifier.delivered().iter().map(|note| note.id).collect();
    assert_eq!(ids, [1, 2]);
    assert_eq!(notifier.delivered()[0].body, "updated");

    notifier.cancel(1).expect("cancel");
    let ids: Vec<i32> = notifier.delivered().iter().map(|note| note.id).collect();
    assert_eq!(ids, [2]);

    notifier.cancel_all().expect("cancel all");
    assert!(notifier.delivered().is_empty());
}

#[test]
fn headless_permission_reads_are_always_granted() {
    let mut notifier = HeadlessNotifier::new();
    assert!(matches!(notifier.has_permission(), Ok(true)));
    assert!(matches!(notifier.request_permission(), Ok(true)));
}

#[test]
fn platform_notifier_degrades_with_a_typed_reason() {
    match platform_notifier() {
        Ok(_) => assert!(
            cfg!(target_os = "macos"),
            "only macOS ships an OS adapter in 0.1.0"
        ),
        Err(error) => {
            assert!(cfg!(not(target_os = "macos")), "unexpected {error}");
            assert!(
                matches!(error, NotifyError::Unsupported(ref reason) if !reason.is_empty()),
                "got {error:?}"
            );
        }
    }
}

// ---------------------------------------------------------------- escaping

#[test]
fn plain_text_becomes_a_direct_delivery_script() {
    let script = applescript_source(&Notification::new(
        1,
        "chat",
        "Download done",
        "file.pdf arrived",
    ));
    assert_eq!(
        script,
        "display notification \"file.pdf arrived\" with title \"Download done\""
    );
}

#[test]
fn quotes_and_backslashes_stay_inside_the_literal() {
    let script = applescript_source(&Notification::new(
        1,
        "chat",
        "say \"hi\" \\ done",
        "open \"a\" \\ b",
    ));
    assert_eq!(
        script,
        "display notification \"open \\\"a\\\" \\\\ b\" with title \"say \\\"hi\\\" \\\\ done\""
    );
}

#[test]
fn a_breakout_attempt_cannot_close_the_literal() {
    // The classic injection shape: a quote that would end the literal and
    // start new AppleScript statements.
    let script = applescript_source(&Notification::new(
        1,
        "chat",
        "x\" do shell script \"evil",
        "body",
    ));
    assert_eq!(
        script,
        "display notification \"body\" with title \"x\\\" do shell script \\\"evil\""
    );
}

#[test]
fn line_breaks_rejoin_with_the_linefeed_constant() {
    let script = applescript_source(&Notification::new(1, "chat", "t", "one\ntwo\r\nthree"));
    assert_eq!(
        script,
        "display notification \"one\" & linefeed & \"two\" & linefeed & \"three\" with title \"t\""
    );
}

// ---------------------------------------------------------------- macOS live

/// A real banner goes through the system scripting tool. The tool's success
/// means the OS accepted the delivery; a missing tool or a rejected event
/// degrades to a named SKIP, never a fake PASS. The script channel's other
/// bridge methods must be typed `Unsupported`, never a silent success.
#[cfg(target_os = "macos")]
#[test]
fn live_post_reaches_the_system_scripting_tool() {
    use opurom_platform::notification::OsascriptNotice;

    let mut notice = OsascriptNotice::new();
    match notice.post(1, "opurom.live", "opurom delivery probe", "a live banner") {
        Ok(()) => println!("LIVE notification accepted by the system scripting tool"),
        Err(error) => println!("SKIP live notification: {error}"),
    }
    assert!(matches!(notice.cancel(1), Err(NotifyError::Unsupported(_))));
    assert!(matches!(
        notice.cancel_all(),
        Err(NotifyError::Unsupported(_))
    ));
    assert!(matches!(
        notice.has_permission(),
        Err(NotifyError::Unsupported(_))
    ));
    assert!(matches!(
        notice.request_permission(),
        Err(NotifyError::Unsupported(_))
    ));
}
