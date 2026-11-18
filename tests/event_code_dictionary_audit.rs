//! Cross-module event-code dictionary audit.
//!
//! Flutter baseline: 3.50.0 (`flutter-golden` HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! The bridge vocabulary is spread over five modules
//! (`platform_channel`, `system_events`, `framework_channels`,
//! `media_capabilities`, `native_capabilities`). This audit compares every
//! code those modules declare against the pinned Flutter 3.50 official
//! constants and namespaces, and reports four classes of divergence:
//!
//! * `duplicate` — one code used by two namespaces, or twice in one.
//! * `gap` — an official constant inside the audited scope that Opurom does
//!   not cover.
//! * `illegal` — a value that violates the namespace or shape rule of its
//!   dictionary.
//! * `unused` — a declared code that the implementation never references.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/message_codecs.dart:310-324` — the
//!   closed standard-codec type-tag table (`null`=0 .. `float32List`=14).
//! - `packages/flutter/lib/src/services/message_codecs.dart:379-385` — tag 5
//!   ("larger integers") is declared but never written by `writeValue`.
//! - `packages/flutter/lib/src/services/message_codecs.dart:386-466` — the
//!   written tag set of `StandardMessageCodec.writeValue`.
//! - `packages/flutter/lib/src/services/message_codecs.dart:490-537` — the
//!   readable tag set of `StandardMessageCodec.readValue`.
//! - `packages/flutter/lib/src/services/message_codecs.dart:467` — an
//!   unsupported value is an `ArgumentError`.
//! - `packages/flutter/lib/src/services/message_codecs.dart:535-536` — an
//!   unknown tag is a `FormatException`.
//! - `packages/flutter/lib/src/services/message_codecs.dart:628-663` — the
//!   standard error envelope is `[1, code, message, details]` plus an
//!   optional stacktrace entry.
//! - `packages/flutter/lib/src/services/message_codecs.dart:160-199` — the
//!   JSON envelopes `[result]`, `[code, message, details]` and
//!   `[code, message, details, stacktrace]`.
//! - `packages/flutter/lib/src/services/message_codec.dart:108-122` —
//!   `PlatformException` carries a free-form `code` string.
//! - `packages/flutter/lib/src/services/message_codec.dart:205-215` —
//!   `MissingPluginException` is an exception kind, not an error code.
//! - `packages/flutter/lib/src/services/platform_channel.dart:359-364` — a
//!   null reply on a strict channel raises `MissingPluginException`.
//! - `packages/flutter/lib/src/services/system_channels.dart:385-388` — the
//!   `flutter/lifecycle` string channel.
//! - `packages/flutter/lib/src/services/system_channels.dart:390-404` — the
//!   `flutter/system` JSON channel and its only documented event type.
//! - `packages/flutter/lib/src/services/binding.dart:174-182` — the framework
//!   switch on the `type` string of a `flutter/system` message.
//! - `packages/flutter/lib/src/services/binding.dart:448-457` — the five
//!   `AppLifecycleState.*` lifecycle message strings.
//! - `packages/flutter/lib/src/services/system_channels.dart:77,98,182,202,
//!   210,294,322,342,346,371,386,402,414,423,430,439,450,478,501,515,566,577,
//!   599,616` — the official framework channel namespace.
//! - `engine/src/flutter/shell/platform/android/io/flutter/embedding/engine/
//!   systemchannels/PlatformChannel.java:196` — `notImplemented()` is how a
//!   host reports an unimplemented method.
//! - `engine/src/flutter/shell/platform/android/io/flutter/embedding/engine/
//!   systemchannels/PlatformChannel.java:56,200` — the only literal Flutter
//!   error-code string, `"error"`.
//!
//! The audit is read-only: it inspects the compiled constants and the crate
//! source text. No ABI/IR change, no new runtime registry.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use opurom_platform::channel_contract;
use opurom_platform::framework_channels::lifecycle_message;
use opurom_platform::media_capabilities::camera_methods;
use opurom_platform::platform_channel::{
    decode_message, encode_message, error_codes, system_channels, CodecError, Value,
};
use opurom_platform::system_events::SystemEvent;
use opurom_platform::Lifecycle;

// ---------------------------------------------------------------------------
// Pinned Flutter 3.50 official tables
// ---------------------------------------------------------------------------

/// `flutter/system` event types. Flutter 3.50 defines exactly one.
const OFFICIAL_SYSTEM_EVENTS: &[(&str, &str)] = &[(
    "memoryPressure",
    "packages/flutter/lib/src/services/binding.dart:174-182",
)];

/// `flutter/lifecycle` message strings.
const OFFICIAL_LIFECYCLE: &[(&str, &str)] = &[
    (
        "AppLifecycleState.resumed",
        "packages/flutter/lib/src/services/binding.dart:450",
    ),
    (
        "AppLifecycleState.inactive",
        "packages/flutter/lib/src/services/binding.dart:451",
    ),
    (
        "AppLifecycleState.hidden",
        "packages/flutter/lib/src/services/binding.dart:452",
    ),
    (
        "AppLifecycleState.paused",
        "packages/flutter/lib/src/services/binding.dart:453",
    ),
    (
        "AppLifecycleState.detached",
        "packages/flutter/lib/src/services/binding.dart:454",
    ),
];

/// The only literal error-code string Flutter's embedder sends
/// (`result.error("error", …)`).
const OFFICIAL_ERROR_CODES: &[(&str, &str)] = &[(
    "error",
    "engine/src/flutter/shell/platform/android/io/flutter/embedding/engine/systemchannels/PlatformChannel.java:56",
)];

/// The Flutter failure kind each Opurom error code stands for. Flutter
/// models a missing handler, an unimplemented method and an invalid argument
/// as exception kinds, not as error-envelope code strings.
const FLUTTER_ERROR_KINDS: &[(&str, &str, &str)] = &[
    (
        "missing_plugin",
        "MissingPluginException",
        "packages/flutter/lib/src/services/message_codec.dart:205-215",
    ),
    (
        "unimplemented",
        "MethodChannel.Result.notImplemented",
        "engine/src/flutter/shell/platform/android/io/flutter/embedding/engine/systemchannels/PlatformChannel.java:196",
    ),
    (
        "invalid_argument",
        "ArgumentError",
        "packages/flutter/lib/src/services/message_codecs.dart:467",
    ),
    (
        "error",
        "PlatformException",
        "packages/flutter/lib/src/services/message_codecs.dart:656",
    ),
];

/// The official framework channel namespace
/// (`packages/flutter/lib/src/services/system_channels.dart`).
const OFFICIAL_FRAMEWORK_CHANNELS: &[(&str, &str)] = &[
    ("flutter/navigation", "system_channels.dart:77"),
    ("flutter/backgesture", "system_channels.dart:98"),
    ("flutter/platform", "system_channels.dart:182"),
    ("flutter/status_bar", "system_channels.dart:202"),
    ("flutter/processtext", "system_channels.dart:210"),
    ("flutter/textinput", "system_channels.dart:294"),
    ("flutter/scribe", "system_channels.dart:322"),
    ("flutter/spellcheck", "system_channels.dart:342"),
    ("flutter/undomanager", "system_channels.dart:346"),
    ("flutter/keyevent", "system_channels.dart:371"),
    ("flutter/lifecycle", "system_channels.dart:386"),
    ("flutter/system", "system_channels.dart:402"),
    ("flutter/accessibility", "system_channels.dart:414"),
    ("flutter/platform_views", "system_channels.dart:423"),
    ("flutter/platform_views_2", "system_channels.dart:430"),
    ("flutter/skia", "system_channels.dart:439"),
    ("flutter/mousecursor", "system_channels.dart:450"),
    ("flutter/restoration", "system_channels.dart:478"),
    ("flutter/deferredcomponent", "system_channels.dart:501"),
    ("flutter/localization", "system_channels.dart:515"),
    ("flutter/menu", "system_channels.dart:566"),
    ("flutter/contextmenu", "system_channels.dart:577"),
    ("flutter/keyboard", "system_channels.dart:599"),
    ("flutter/sensitivecontent", "system_channels.dart:616"),
];

/// The framework channels the bridge vocabulary mirrors.
const BRIDGED_FRAMEWORK_CHANNELS: &[&str] = &[
    "flutter/platform",
    "flutter/lifecycle",
    "flutter/system",
    "flutter/platform_views",
    "flutter/platform_views_2",
];

/// The plugin channel namespaces Opurom mirrors, from the pinned plugin
/// snapshot (`flutter-plugins` at
/// `557d3284ac9dda32a1106bb75be8a23bdccd2f96`).
const OFFICIAL_PLUGIN_CHANNELS: &[(&str, &str)] = &[
    (
        "plugins.flutter.io/camera",
        "flutter-plugins/packages/camera/camera_platform_interface/lib/src/method_channel/method_channel_camera.dart:17",
    ),
    (
        "plugins.flutter.io/path_provider",
        "flutter-plugins/packages/path_provider/path_provider_platform_interface/lib/src/method_channel_path_provider.dart:16",
    ),
    (
        "plugins.flutter.io/url_launcher",
        "flutter-plugins/packages/url_launcher/url_launcher_platform_interface/lib/method_channel_url_launcher.dart:12",
    ),
    (
        "plugins.flutter.io/image_picker",
        "flutter-plugins/packages/image_picker/image_picker_platform_interface/lib/src/method_channel/method_channel_image_picker.dart:12",
    ),
    (
        "plugins.flutter.io/shared_preferences",
        "flutter-plugins/packages/shared_preferences/shared_preferences_platform_interface/lib/method_channel_shared_preferences.dart:12",
    ),
];

/// The closed standard-codec type-tag table
/// (`packages/flutter/lib/src/services/message_codecs.dart:310-324`).
const OFFICIAL_CODEC_TAGS: &[(u8, &str, &str)] = &[
    (0, "null", "message_codecs.dart:310"),
    (1, "true", "message_codecs.dart:311"),
    (2, "false", "message_codecs.dart:312"),
    (3, "int32", "message_codecs.dart:313"),
    (4, "int64", "message_codecs.dart:314"),
    (5, "largeInt", "message_codecs.dart:315"),
    (6, "float64", "message_codecs.dart:316"),
    (7, "string", "message_codecs.dart:317"),
    (8, "uint8List", "message_codecs.dart:318"),
    (9, "int32List", "message_codecs.dart:319"),
    (10, "int64List", "message_codecs.dart:320"),
    (11, "float64List", "message_codecs.dart:321"),
    (12, "list", "message_codecs.dart:322"),
    (13, "map", "message_codecs.dart:323"),
    (14, "float32List", "message_codecs.dart:324"),
];

/// The tag Flutter never writes (`message_codecs.dart:379-385`).
const UNWRITTEN_CODEC_TAG: u8 = 5;

// ---------------------------------------------------------------------------
// Audit model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum FindingKind {
    Duplicate,
    Gap,
    Illegal,
    Unused,
}

impl FindingKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Duplicate => "duplicate",
            Self::Gap => "gap",
            Self::Illegal => "illegal",
            Self::Unused => "unused",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Finding {
    namespace: String,
    kind: FindingKind,
    detail: String,
}

impl Finding {
    fn new(namespace: &str, kind: FindingKind, detail: impl Into<String>) -> Self {
        Self {
            namespace: namespace.to_owned(),
            kind,
            detail: detail.into(),
        }
    }

    /// Machine-readable report line: `namespace|kind|detail`.
    fn as_line(&self) -> String {
        format!("{}|{}|{}", self.namespace, self.kind.as_str(), self.detail)
    }
}

/// The shape rule a dictionary value must satisfy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    /// The value must be one of the official values.
    Official,
    /// The value must be official, or carry the host-adapter prefix.
    OfficialOrPrefixed(&'static str),
    /// The value must be a lower snake_case identifier.
    SnakeCase,
}

/// How a declared value is referenced by the implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Usage {
    /// The literal must appear in production source.
    Literal(&'static str),
    /// The identifier token must appear in production source.
    Identifier(&'static str),
}

/// One audited vocabulary: the Flutter-pinned official table, the Opurom
/// counterpart, and the rule that decides which values are legal.
struct Vocabulary {
    namespace: &'static str,
    official: Vec<(&'static str, &'static str)>,
    opurom: Vec<(&'static str, &'static str, Usage)>,
    rule: Rule,
    /// Whether every official value must have an Opurom counterpart.
    complete: bool,
}

fn is_legal(value: &str, rule: Rule, official: &BTreeSet<&str>) -> bool {
    match rule {
        Rule::Official => official.contains(value),
        Rule::OfficialOrPrefixed(prefix) => official.contains(value) || value.starts_with(prefix),
        Rule::SnakeCase => {
            !value.is_empty()
                && value
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                && !value.starts_with('_')
                && !value.ends_with('_')
        }
    }
}

fn duplicated(values: &[&str]) -> Vec<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut repeated: BTreeSet<&str> = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            repeated.insert(value);
        }
    }
    repeated.into_iter().map(str::to_owned).collect()
}

/// Audit every vocabulary and return the findings in a stable order.
fn audit(vocabularies: &[Vocabulary], source: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for vocabulary in vocabularies {
        let official: BTreeSet<&str> = vocabulary.official.iter().map(|e| e.0).collect();
        let opurom: BTreeSet<&str> = vocabulary.opurom.iter().map(|e| e.0).collect();

        let official_values: Vec<&str> = vocabulary.official.iter().map(|e| e.0).collect();
        for value in duplicated(&official_values) {
            findings.push(Finding::new(
                vocabulary.namespace,
                FindingKind::Duplicate,
                format!("official:{value}"),
            ));
        }
        let opurom_values: Vec<&str> = vocabulary.opurom.iter().map(|e| e.0).collect();
        for value in duplicated(&opurom_values) {
            findings.push(Finding::new(
                vocabulary.namespace,
                FindingKind::Duplicate,
                format!("opurom:{value}"),
            ));
        }

        for (value, _pin, _usage) in &vocabulary.opurom {
            if !is_legal(value, vocabulary.rule, &official) {
                findings.push(Finding::new(
                    vocabulary.namespace,
                    FindingKind::Illegal,
                    *value,
                ));
            }
        }

        if vocabulary.complete {
            for (value, pin) in &vocabulary.official {
                if !opurom.contains(value) {
                    findings.push(Finding::new(
                        vocabulary.namespace,
                        FindingKind::Gap,
                        format!("{value} ({pin})"),
                    ));
                }
            }
        }

        for (value, _pin, usage) in &vocabulary.opurom {
            let count = match usage {
                Usage::Literal(literal) => literal_count(source, literal),
                Usage::Identifier(identifier) => identifier_count(source, identifier),
            };
            // A declaration alone is one reference; a code that is really used
            // is referenced from at least one more production site.
            if count <= 1 {
                findings.push(Finding::new(
                    vocabulary.namespace,
                    FindingKind::Unused,
                    *value,
                ));
            }
        }
    }

    // A code used by two namespaces is ambiguous on the wire.
    let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for vocabulary in vocabularies {
        for (value, _pin, _usage) in &vocabulary.opurom {
            owners.entry(value).or_default().push(vocabulary.namespace);
        }
    }
    for (value, namespaces) in owners {
        if namespaces.len() > 1 {
            findings.push(Finding::new(
                "cross",
                FindingKind::Duplicate,
                format!("{value} {}", namespaces.join(",")),
            ));
        }
    }

    findings.sort();
    findings
}

// ---------------------------------------------------------------------------
// Source inspection
// ---------------------------------------------------------------------------

fn collect_rust_files(dir: &PathBuf, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).expect("read source directory");
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Production source text of the crate: module test blocks and comment lines
/// are removed so a doc mention never counts as a use.
fn production_source() -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);
    files.sort();
    let mut out = String::new();
    for path in files {
        let raw = fs::read_to_string(&path).expect("read source file");
        let production = raw.split("#[cfg(test)]").next().unwrap_or(&raw);
        for line in production.lines() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn identifier_count(source: &str, identifier: &str) -> usize {
    source
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|token| *token == identifier)
        .count()
}

fn literal_count(source: &str, literal: &str) -> usize {
    source.matches(literal).count()
}

// ---------------------------------------------------------------------------
// Live vocabularies
// ---------------------------------------------------------------------------

fn opurom_system_events() -> Vec<SystemEvent> {
    vec![
        SystemEvent::MemoryPressure,
        SystemEvent::KeyboardVisibility {
            visible: true,
            height: Some(300),
        },
        SystemEvent::Orientation { orientation: 1 },
        SystemEvent::Brightness { dark: true },
        SystemEvent::PowerMode { low_power: true },
        SystemEvent::RefreshRate { hz: 120 },
    ]
}

fn system_event_vocabulary() -> Vocabulary {
    Vocabulary {
        namespace: "system_event",
        official: OFFICIAL_SYSTEM_EVENTS.to_vec(),
        opurom: opurom_system_events()
            .iter()
            .map(|event| {
                (
                    event.code(),
                    "opurom-platform/src/system_events.rs",
                    Usage::Literal(event.code()),
                )
            })
            .collect(),
        rule: Rule::OfficialOrPrefixed("host."),
        complete: true,
    }
}

fn lifecycle_states() -> [Lifecycle; 5] {
    [
        Lifecycle::Resumed,
        Lifecycle::Inactive,
        Lifecycle::Hidden,
        Lifecycle::Paused,
        Lifecycle::Destroyed,
    ]
}

fn lifecycle_vocabulary() -> Vocabulary {
    Vocabulary {
        namespace: "lifecycle",
        official: OFFICIAL_LIFECYCLE.to_vec(),
        opurom: lifecycle_states()
            .iter()
            .map(|state| {
                let message = lifecycle_message(*state);
                (
                    message,
                    "opurom-platform/src/framework_channels.rs",
                    Usage::Literal(message),
                )
            })
            .collect(),
        rule: Rule::Official,
        complete: true,
    }
}

fn error_code_vocabulary() -> Vocabulary {
    Vocabulary {
        namespace: "error_code",
        official: OFFICIAL_ERROR_CODES.to_vec(),
        opurom: vec![
            (
                error_codes::MISSING_PLUGIN,
                "opurom-platform/src/platform_channel.rs",
                Usage::Identifier("MISSING_PLUGIN"),
            ),
            (
                error_codes::UNIMPLEMENTED,
                "opurom-platform/src/platform_channel.rs",
                Usage::Identifier("UNIMPLEMENTED"),
            ),
            (
                error_codes::INVALID_ARGUMENT,
                "opurom-platform/src/platform_channel.rs",
                Usage::Identifier("INVALID_ARGUMENT"),
            ),
            (
                error_codes::ERROR,
                "opurom-platform/src/platform_channel.rs",
                Usage::Identifier("ERROR"),
            ),
        ],
        rule: Rule::SnakeCase,
        // Flutter defines no closed code-string vocabulary, so the official
        // side cannot be "covered" exhaustively.
        complete: false,
    }
}

fn channel_name_vocabulary() -> Vocabulary {
    let mut official: Vec<(&'static str, &'static str)> = OFFICIAL_FRAMEWORK_CHANNELS
        .iter()
        .copied()
        .filter(|(name, _)| BRIDGED_FRAMEWORK_CHANNELS.contains(name))
        .collect();
    official.push(OFFICIAL_PLUGIN_CHANNELS[0]);
    Vocabulary {
        namespace: "channel_name",
        official,
        opurom: vec![
            (
                system_channels::SYSTEM,
                "opurom-platform/src/platform_channel.rs",
                Usage::Identifier("SYSTEM"),
            ),
            (
                system_channels::LIFECYCLE,
                "opurom-platform/src/platform_channel.rs",
                Usage::Identifier("LIFECYCLE"),
            ),
            (
                system_channels::PLATFORM,
                "opurom-platform/src/platform_channel.rs",
                Usage::Identifier("PLATFORM"),
            ),
            (
                system_channels::PLATFORM_VIEWS,
                "opurom-platform/src/platform_channel.rs",
                Usage::Identifier("PLATFORM_VIEWS"),
            ),
            (
                system_channels::PLATFORM_VIEWS_2,
                "opurom-platform/src/platform_channel.rs",
                Usage::Identifier("PLATFORM_VIEWS_2"),
            ),
            (
                camera_methods::CHANNEL,
                "opurom-platform/src/media_capabilities.rs",
                Usage::Identifier("CHANNEL"),
            ),
        ],
        rule: Rule::Official,
        complete: true,
    }
}

fn vocabularies() -> Vec<Vocabulary> {
    vec![
        system_event_vocabulary(),
        lifecycle_vocabulary(),
        error_code_vocabulary(),
        channel_name_vocabulary(),
    ]
}

// ---------------------------------------------------------------------------
// Codec tag dictionary
// ---------------------------------------------------------------------------

struct TagSample {
    variant: &'static str,
    value: Value,
    tag: u8,
}

fn tag_samples() -> Vec<TagSample> {
    vec![
        TagSample {
            variant: "Null",
            value: Value::Null,
            tag: 0,
        },
        TagSample {
            variant: "Bool(true)",
            value: Value::Bool(true),
            tag: 1,
        },
        TagSample {
            variant: "Bool(false)",
            value: Value::Bool(false),
            tag: 2,
        },
        TagSample {
            variant: "I32",
            value: Value::I32(7),
            tag: 3,
        },
        TagSample {
            variant: "I64",
            value: Value::I64(7),
            tag: 4,
        },
        TagSample {
            variant: "F64",
            value: Value::F64(1.5),
            tag: 6,
        },
        TagSample {
            variant: "String",
            value: Value::String("opurom".to_owned()),
            tag: 7,
        },
        TagSample {
            variant: "Uint8List",
            value: Value::Uint8List(vec![1, 2, 3]),
            tag: 8,
        },
        TagSample {
            variant: "Int32List",
            value: Value::Int32List(vec![1, 2, 3]),
            tag: 9,
        },
        TagSample {
            variant: "Int64List",
            value: Value::Int64List(vec![1, 2, 3]),
            tag: 10,
        },
        TagSample {
            variant: "Float64List",
            value: Value::Float64List(vec![1.0]),
            tag: 11,
        },
        TagSample {
            variant: "List",
            value: Value::List(vec![Value::I32(1)]),
            tag: 12,
        },
        TagSample {
            variant: "Map",
            value: Value::Map(BTreeMap::from([(
                Value::String("k".to_owned()),
                Value::I32(1),
            )])),
            tag: 13,
        },
        TagSample {
            variant: "Float32List",
            value: Value::Float32List(vec![1.0]),
            tag: 14,
        },
    ]
}

/// The tag byte a value produces on the wire. A one-element list carries the
/// sample tag at index two: list tag, size, sample tag.
fn emitted_tag(value: &Value) -> u8 {
    let encoded = encode_message(&Value::List(vec![value.clone()]))
        .expect("list encodes")
        .expect("a list is not a null message");
    encoded[2]
}

fn audit_codec_tags(samples: &[TagSample]) -> Vec<Finding> {
    let mut findings = Vec::new();

    let official_tags: Vec<String> = OFFICIAL_CODEC_TAGS
        .iter()
        .map(|entry| entry.0.to_string())
        .collect();
    let official_refs: Vec<&str> = official_tags.iter().map(String::as_str).collect();
    for value in duplicated(&official_refs) {
        findings.push(Finding::new(
            "codec_tag",
            FindingKind::Duplicate,
            format!("official:{value}"),
        ));
    }

    let mut covered: BTreeSet<u8> = BTreeSet::new();
    for sample in samples {
        let official = OFFICIAL_CODEC_TAGS
            .iter()
            .find(|entry| entry.0 == sample.tag)
            .map(|entry| entry.1)
            .unwrap_or("");
        if official.is_empty() {
            findings.push(Finding::new(
                "codec_tag",
                FindingKind::Illegal,
                format!("{}:{}", sample.variant, sample.tag),
            ));
        }
        if sample.value.tag() != sample.tag {
            findings.push(Finding::new(
                "codec_tag",
                FindingKind::Illegal,
                format!("{}:tag {}", sample.variant, sample.value.tag()),
            ));
        }
        let emitted = emitted_tag(&sample.value);
        if emitted != sample.tag {
            findings.push(Finding::new(
                "codec_tag",
                FindingKind::Illegal,
                format!("{}:emits {}", sample.variant, emitted),
            ));
        }
        covered.insert(sample.tag);
    }

    for (tag, name, pin) in OFFICIAL_CODEC_TAGS {
        if *tag == UNWRITTEN_CODEC_TAG {
            continue;
        }
        if !covered.contains(tag) {
            findings.push(Finding::new(
                "codec_tag",
                FindingKind::Gap,
                format!("{tag} {name} ({pin})"),
            ));
        }
    }

    // Tag 5 is declared by Flutter but never written by either side.
    findings.push(Finding::new(
        "codec_tag",
        FindingKind::Unused,
        format!("{UNWRITTEN_CODEC_TAG} largeInt"),
    ));

    findings.sort();
    findings
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn pinned_official_tables_are_duplicate_free_and_legal() {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (value, pin) in OFFICIAL_SYSTEM_EVENTS {
        assert!(seen.insert(value), "duplicate official value {value}");
        assert!(!pin.is_empty());
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (value, pin) in OFFICIAL_LIFECYCLE {
        assert!(seen.insert(value), "duplicate official value {value}");
        assert!(!pin.is_empty());
        assert!(value.starts_with("AppLifecycleState."));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (value, pin) in OFFICIAL_ERROR_CODES {
        assert!(seen.insert(value), "duplicate official value {value}");
        assert!(!pin.is_empty());
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (value, pin) in OFFICIAL_FRAMEWORK_CHANNELS {
        assert!(seen.insert(value), "duplicate official value {value}");
        assert!(!pin.is_empty());
        assert!(
            value.starts_with("flutter/"),
            "{value} is outside the framework namespace"
        );
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (value, pin) in OFFICIAL_PLUGIN_CHANNELS {
        assert!(seen.insert(value), "duplicate official value {value}");
        assert!(!pin.is_empty());
        assert!(
            value.starts_with("plugins.flutter.io/"),
            "{value} is outside the plugin namespace"
        );
    }
    let mut seen: BTreeSet<u8> = BTreeSet::new();
    for (tag, name, pin) in OFFICIAL_CODEC_TAGS {
        assert!(seen.insert(*tag), "duplicate official tag {tag}");
        assert!(!name.is_empty());
        assert!(!pin.is_empty());
    }
    assert_eq!(seen.len(), 15);
}

#[test]
fn live_vocabulary_audit_reports_no_duplicates_illegal_values_or_gaps() {
    let source = production_source();
    let findings = audit(&vocabularies(), &source);
    let blocking: Vec<&Finding> = findings
        .iter()
        .filter(|finding| finding.kind != FindingKind::Unused)
        .collect();
    assert!(
        blocking.is_empty(),
        "unexpected audit findings: {:?}",
        blocking
            .iter()
            .map(|finding| finding.as_line())
            .collect::<Vec<String>>()
    );
}

#[test]
fn unused_codes_match_the_documented_baseline() {
    let source = production_source();
    let mut findings = audit(&vocabularies(), &source);
    findings.extend(audit_codec_tags(&tag_samples()));
    let mut unused: Vec<String> = findings
        .iter()
        .filter(|finding| finding.kind == FindingKind::Unused)
        .map(|finding| finding.as_line())
        .collect();
    unused.sort();

    let expected = vec![
        // The plugin channel name is declared but no production channel table
        // references it yet.
        "channel_name|unused|plugins.flutter.io/camera".to_owned(),
        // Flutter declares tag 5 but never writes it.
        "codec_tag|unused|5 largeInt".to_owned(),
        // Flutter models a missing handler as an exception kind, never as an
        // error-envelope code, so this spelling is a kind-level label only.
        "error_code|unused|missing_plugin".to_owned(),
    ];
    assert_eq!(unused, expected);

    for finding in &findings {
        assert_eq!(finding.as_line().split('|').count(), 3);
    }
}

#[test]
fn auditor_detects_injected_duplicates_gaps_illegal_values_and_unused_codes() {
    let source = production_source();

    let broken = Vocabulary {
        namespace: "probe",
        official: vec![("alpha", "pin"), ("beta", "pin")],
        opurom: vec![
            ("alpha", "pin", Usage::Literal("opurom_probe_alpha")),
            ("alpha", "pin", Usage::Literal("opurom_probe_alpha")),
            ("Gamma!", "pin", Usage::Literal("opurom_probe_gamma")),
        ],
        rule: Rule::Official,
        complete: true,
    };
    let findings = audit(&[broken], &source);
    let kinds: BTreeSet<(FindingKind, String)> = findings
        .iter()
        .map(|finding| (finding.kind, finding.detail.clone()))
        .collect();
    assert!(kinds.contains(&(FindingKind::Duplicate, "opurom:alpha".to_owned())));
    assert!(kinds.contains(&(FindingKind::Illegal, "Gamma!".to_owned())));
    assert!(kinds.contains(&(FindingKind::Gap, "beta (pin)".to_owned())));
    // "alpha" is not present in the crate source at all.
    assert!(kinds.contains(&(FindingKind::Unused, "alpha".to_owned())));

    // A cross-namespace duplicate is reported against the shared namespace.
    let first = Vocabulary {
        namespace: "one",
        official: vec![("shared", "pin")],
        opurom: vec![("shared", "pin", Usage::Identifier("SHARED_TOKEN_UNIQUE"))],
        rule: Rule::Official,
        complete: true,
    };
    let second = Vocabulary {
        namespace: "two",
        official: vec![("shared", "pin")],
        opurom: vec![("shared", "pin", Usage::Identifier("SHARED_TOKEN_UNIQUE"))],
        rule: Rule::Official,
        complete: true,
    };
    let findings = audit(&[first, second], &source);
    assert!(findings.iter().any(|finding| {
        finding.namespace == "cross"
            && finding.kind == FindingKind::Duplicate
            && finding.detail.starts_with("shared ")
    }));
}

#[test]
fn codec_tag_dictionary_matches_the_flutter_tag_table() {
    let samples = tag_samples();
    // One sample per `Value` variant; the closed value universe has 14
    // variants because Flutter's tag 5 has no Rust counterpart.
    assert_eq!(samples.len(), 14);

    let findings = audit_codec_tags(&samples);
    let blocking: Vec<&Finding> = findings
        .iter()
        .filter(|finding| finding.kind != FindingKind::Unused)
        .collect();
    assert!(
        blocking.is_empty(),
        "unexpected codec tag findings: {:?}",
        blocking
            .iter()
            .map(|finding| finding.as_line())
            .collect::<Vec<String>>()
    );

    // Every writable official tag is produced by Opurom.
    let emitted: BTreeSet<u8> = samples
        .iter()
        .map(|sample| emitted_tag(&sample.value))
        .collect();
    let expected: BTreeSet<u8> = OFFICIAL_CODEC_TAGS
        .iter()
        .map(|entry| entry.0)
        .filter(|tag| *tag != UNWRITTEN_CODEC_TAG)
        .collect();
    assert_eq!(emitted, expected);

    // Tag 5 is recognized but rejected: the closed value universe has no
    // bignum type, and Flutter never writes the tag either.
    assert_eq!(
        decode_message(Some(&[5, 1, b'1'])),
        Err(CodecError::Unsupported("large integer"))
    );
    // An unknown tag is a corrupt message, matching Flutter's
    // `FormatException` (`message_codecs.dart:535-536`).
    assert_eq!(
        decode_message(Some(&[15])),
        Err(CodecError::Corrupt("unknown tag"))
    );
}

#[test]
fn system_event_codes_are_produced_and_parsed() {
    for event in opurom_system_events() {
        let value = event.to_value();
        let code = event.code();
        let Value::Map(fields) = &value else {
            panic!("{code} must be encoded as a map");
        };
        assert_eq!(
            fields.get(&Value::String("type".to_owned())),
            Some(&Value::String(code.to_owned())),
            "{code} must be carried by the `type` key"
        );
        assert_eq!(
            SystemEvent::from_value(&value),
            Some(event),
            "{code} must decode back to its event"
        );
    }
    // Only the framework event is named by Flutter; the rest are explicitly
    // host-adapter codes and must never claim framework ownership.
    let framework: Vec<&str> = opurom_system_events()
        .iter()
        .filter(|event| event.is_flutter_framework_event())
        .map(|event| event.code())
        .collect();
    assert_eq!(framework, vec!["memoryPressure"]);
}

#[test]
fn error_codes_map_onto_flutter_failure_kinds() {
    let opurom_codes = [
        error_codes::MISSING_PLUGIN,
        error_codes::UNIMPLEMENTED,
        error_codes::INVALID_ARGUMENT,
        error_codes::ERROR,
    ];
    let mut kinds: Vec<&str> = FLUTTER_ERROR_KINDS.iter().map(|entry| entry.0).collect();
    kinds.sort();
    let mut sorted_codes = opurom_codes.to_vec();
    sorted_codes.sort();
    assert_eq!(kinds, sorted_codes, "every error code needs a Flutter kind");

    // Only "error" is a literal Flutter error-code string; the other three
    // name exception kinds that Flutter raises instead of sending.
    let official_codes: Vec<&str> = OFFICIAL_ERROR_CODES.iter().map(|entry| entry.0).collect();
    assert_eq!(official_codes, vec!["error"]);
    for (code, kind, pin) in FLUTTER_ERROR_KINDS {
        assert!(!kind.is_empty());
        assert!(!pin.is_empty());
        if *code != "error" {
            assert!(
                !official_codes.contains(code),
                "{code} must not claim to be a Flutter code string"
            );
        }
    }

    // The kind-level code never appears in an error envelope.
    let source = production_source();
    assert_eq!(
        identifier_count(&source, "MISSING_PLUGIN"),
        1,
        "missing_plugin must stay a kind-level label"
    );
}

#[test]
fn channel_namespace_coverage_is_explicit() {
    let bridged: BTreeSet<&str> = BRIDGED_FRAMEWORK_CHANNELS.iter().copied().collect();
    let official: BTreeSet<&str> = OFFICIAL_FRAMEWORK_CHANNELS
        .iter()
        .map(|entry| entry.0)
        .collect();
    for name in &bridged {
        assert!(official.contains(name), "{name} is not an official channel");
    }

    // The implementation's channel inventory must agree with the pinned
    // table above, name for name.
    let inventory: Vec<&str> = channel_contract::FRAMEWORK_CHANNELS
        .iter()
        .map(|entry| entry.name)
        .collect();
    let mut sorted_inventory = inventory.clone();
    sorted_inventory.sort();
    assert_eq!(
        sorted_inventory,
        official.iter().copied().collect::<Vec<&str>>()
    );

    let uncovered: Vec<&str> = official
        .iter()
        .filter(|name| !bridged.contains(*name))
        .copied()
        .collect();
    assert_eq!(
        uncovered,
        vec![
            "flutter/accessibility",
            "flutter/backgesture",
            "flutter/contextmenu",
            "flutter/deferredcomponent",
            "flutter/keyboard",
            "flutter/keyevent",
            "flutter/localization",
            "flutter/menu",
            "flutter/mousecursor",
            "flutter/navigation",
            "flutter/processtext",
            "flutter/restoration",
            "flutter/scribe",
            "flutter/sensitivecontent",
            "flutter/skia",
            "flutter/spellcheck",
            "flutter/status_bar",
            "flutter/textinput",
            "flutter/undomanager",
        ],
        "the uncovered framework channel set changed"
    );
    assert_eq!(uncovered.len(), 19);

    // Every channel name Opurom declares lives in an official namespace.
    for (name, _pin) in OFFICIAL_PLUGIN_CHANNELS {
        assert!(name.starts_with("plugins.flutter.io/"));
    }
    for name in BRIDGED_FRAMEWORK_CHANNELS {
        assert!(name.starts_with("flutter/"));
    }
}
