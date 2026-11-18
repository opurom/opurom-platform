//! Diagnostic and audit-output contract for platform channels.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/message_codec.dart:108-135` —
//!   `PlatformException` has a required `code` and optional `message`,
//!   `details`, `stacktrace`; diagnostic output must preserve that shape.
//! - `packages/flutter/lib/src/services/message_codecs.dart:121-176` —
//!   `JSONMethodCodec` uses `[code, message, details]` error envelopes and
//!   accepts a fourth stacktrace entry.
//! - `packages/flutter/lib/src/services/platform_channel.dart:350-366` —
//!   a null reply is `MissingPluginException` on a strict channel.
//! - `packages/flutter/lib/src/services/message_codecs.dart:337-345,477,536,
//!   615,641,663` — malformed payloads are rejected with stable exceptions.
//!
//! No ABI/IR change: the tests project existing typed replies/rows into a
//! machine-readable diagnostic record and assert deterministic ordering,
//! deduplication and field stability.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use opurom_platform::framework_channels::unimplemented;
use opurom_platform::host_injection_audit::audit_all;
use opurom_platform::native_capabilities::{self, ALL_CAPABILITIES};
use opurom_platform::platform_channel::{decode_envelope, error_codes, CodecError, Reply, Value};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Diagnostic {
    code: String,
    message: String,
    details: String,
}

impl Diagnostic {
    fn from_reply(reply: &Reply) -> Self {
        match reply {
            Reply::PlatformError {
                code,
                message,
                details,
            } => Self {
                code: code.clone(),
                message: message.clone().unwrap_or_default(),
                details: match details {
                    Value::Null => "null".to_owned(),
                    other => format!("{other:?}"),
                },
            },
            other => Self {
                code: "success_or_missing".to_owned(),
                message: format!("{other:?}"),
                details: "null".to_owned(),
            },
        }
    }

    fn as_line(&self) -> String {
        format!("{}|{}|{}", self.code, self.message, self.details)
    }
}

#[test]
fn diagnostic_fields_follow_platform_exception_shape() {
    let err = Reply::PlatformError {
        code: "SAMPLE".to_owned(),
        message: Some("text".to_owned()),
        details: Value::I32(7),
    };
    let d = Diagnostic::from_reply(&err);
    assert_eq!(d.code, "SAMPLE");
    assert_eq!(d.message, "text");
    assert_eq!(d.details, "I32(7)");
    assert_eq!(d.as_line(), "SAMPLE|text|I32(7)");

    let no_message = Reply::PlatformError {
        code: "SAMPLE".to_owned(),
        message: None,
        details: Value::Null,
    };
    assert_eq!(
        Diagnostic::from_reply(&no_message).as_line(),
        "SAMPLE||null"
    );
}

#[test]
fn error_codes_and_messages_are_stable_and_machine_readable() {
    let replies = [
        unimplemented("camera.takePicture"),
        unimplemented("camera.takePicture"),
        unimplemented("video.play"),
    ];
    let rows: Vec<_> = replies.iter().map(Diagnostic::from_reply).collect();
    assert_eq!(rows[0], rows[1]);
    assert_ne!(rows[0], rows[2]);
    for row in &rows {
        assert_eq!(row.code, error_codes::UNIMPLEMENTED);
        assert!(row.message.starts_with("no host handler for "));
        assert_eq!(row.details, "null");
    }
    // Sorting and dedup produce the stable audit order, independent of input
    // order, and every line has exactly three pipe-separated fields.
    let mut shuffled = rows.clone();
    shuffled.reverse();
    shuffled.sort();
    shuffled.dedup();
    assert_eq!(shuffled.len(), 2);
    for row in &shuffled {
        assert_eq!(row.as_line().split('|').count(), 3);
    }
    assert_eq!(
        shuffled[0].code.as_str(),
        error_codes::UNIMPLEMENTED,
        "codes sort before messages"
    );
}

#[test]
fn codec_errors_are_stable_typed_reasons() {
    let cases = [
        (
            CodecError::Corrupt("truncated"),
            "FormatException: Message corrupted (truncated)",
        ),
        (
            CodecError::Corrupt("trailing bytes"),
            "FormatException: Message corrupted (trailing bytes)",
        ),
        (
            CodecError::Unsupported("size > u32"),
            "ArgumentError: size > u32",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
    let missing = decode_envelope(None).unwrap();
    assert_eq!(missing, Reply::MissingPlugin);
    assert_eq!(
        Diagnostic::from_reply(&missing).as_line(),
        "success_or_missing|MissingPlugin|null"
    );
}

#[test]
fn capability_audit_rows_are_sorted_unique_and_stable() {
    let rows = audit_all();
    assert_eq!(rows.len(), ALL_CAPABILITIES.len());
    let mut codes: Vec<_> = rows
        .iter()
        .map(|row| row.capability.as_str().to_owned())
        .collect();
    let original = codes.clone();
    codes.sort();
    codes.dedup();
    assert_eq!(codes.len(), original.len());

    let mut lines: Vec<String> = rows
        .iter()
        .map(|row| {
            let gap = row.gap_reason.unwrap_or("none");
            format!("{}|{}|{}", row.capability.as_str(), row.method_count, gap)
        })
        .collect();
    let unsorted = lines.clone();
    lines.sort();
    lines.dedup();
    assert_eq!(lines.len(), unsorted.len(), "audit rows must be unique");
    let unique: BTreeSet<_> = lines.iter().collect();
    assert_eq!(unique.len(), lines.len());

    // The method table itself remains the source of truth for each row.
    for row in rows {
        assert_eq!(
            row.method_count,
            native_capabilities::methods_for(row.capability).len()
        );
    }

    let mut rendered = String::new();
    for line in &lines {
        writeln!(rendered, "{line}").unwrap();
    }
    assert!(rendered.lines().all(|line| line.split('|').count() == 3));
}
