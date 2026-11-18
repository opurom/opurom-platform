//! Flutter-compatible platform channel value codec and channel contract.
//!
//! Flutter baseline: 3.50-candidate, engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee` (pinned locally in
//! `flutter-golden`). All file:line references in this module refer to that
//! checkout, paths relative to `flutter-golden/`.
//!
//! Architecture note: Opurom compiles IR ahead of time and has no runtime
//! interpreter or mutable widget tree. This module therefore implements only
//! the *data contract* of Flutter's channels — named channels, the frozen
//! `StandardMessageCodec` wire format, method-call/reply envelopes and the
//! missing/plugin error distinction. Handler registration is a compile-time
//! table (`ChannelTable`), not a runtime dynamic registry; the in-process
//! `PlatformAdapter` remains the transport. This is the ADAPT decision in
//! `opurom-spec/PLATFORM_CHANNEL_PARITY.md:§3 G1/G2/G3/G4`.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/message_codecs.dart:263-562`
//!   (`StandardMessageCodec` tags, writeValue/readValue, expanding size).
//! - `packages/flutter/lib/src/services/message_codecs.dart:580-657`
//!   (`StandardMethodCodec` call and reply envelopes).
//! - `packages/flutter/lib/src/services/platform_channel.dart:350-366`
//!   (null reply ⇒ `MissingPluginException`),
//!   `:586-614` (handler error mapping), `:692-737` (EventChannel listen
//!   0→1 / cancel 1→0), `:622-633` (`OptionalMethodChannel`).
//! - `engine/src/flutter/lib/ui/channel_buffers.dart:75-81,257-260,321,343-354`
//!   (per-channel FIFO, default capacity 1, drop-oldest overflow).
//! - `engine/src/flutter/lib/ui/channel_buffers.dart:330,345,382-385` —
//!   channels are held in `Map<String, _Channel>` with `putIfAbsent` and
//!   name-keyed lookup. Opurom projects this to a `BTreeMap<&str, …>` index so
//!   dispatch is O(log n) instead of a Vec scan while keeping a deterministic
//!   iteration order (no hash-map iteration in compiled output).
//! - `packages/flutter/lib/src/services/binding.dart:654-675` —
//!   `setMessageHandler` replaces the handler for a name; the table's `with`
//!   keeps that last-handler-wins rule at build time.

use std::collections::BTreeMap;
use std::fmt;

/// A value in the Flutter standard codec's closed type universe.
///
/// Typed lists are separate variants because Flutter tags them separately
/// (`message_codecs.dart:310-324`): a `Uint8List` is not a `List<int>` on the
/// wire. Maps use `BTreeMap` so the Rust value has a canonical iteration order;
/// the wire format itself carries Flutter's insertion order.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// Dart `int` in `[-2^31, 2^31-1]` ⇒ tag 3 (`message_codecs.dart:391-398`).
    I32(i32),
    /// Dart `int` outside that range, up to i64 ⇒ tag 4.
    I64(i64),
    /// Dart `double` ⇒ tag 6 (checked before int in Dart, `:388-397`).
    F64(f64),
    String(String),
    Uint8List(Vec<u8>),
    Int32List(Vec<i32>),
    Int64List(Vec<i64>),
    Float32List(Vec<f32>),
    Float64List(Vec<f64>),
    List(Vec<Value>),
    Map(BTreeMap<Value, Value>),
}

impl Value {
    /// Flutter's tag byte for this value (`message_codecs.dart:310-324`).
    pub fn tag(&self) -> u8 {
        match self {
            Value::Null => 0,
            Value::Bool(true) => 1,
            Value::Bool(false) => 2,
            Value::I32(_) => 3,
            Value::I64(_) => 4,
            // Tag 5 (large integer / ASCII hex) is read but never emitted by
            // StandardMessageCodec.writeValue (`:379-385`); kept for decode.
            Value::F64(_) => 6,
            Value::String(_) => 7,
            Value::Uint8List(_) => 8,
            Value::Int32List(_) => 9,
            Value::Int64List(_) => 10,
            Value::Float64List(_) => 11,
            Value::List(_) => 12,
            Value::Map(_) => 13,
            Value::Float32List(_) => 14,
        }
    }
}

// `BTreeMap` requires Ord; Flutter maps only require identity keys, but the
// canonical Rust representation needs a total order. NaN is never a valid map
// key in either Dart or Rust; compare its bits for determinism.
impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Value {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let rank = |v: &Value| -> u8 {
            match v {
                Value::Null => 0,
                Value::Bool(_) => 1,
                Value::I32(_) => 2,
                Value::I64(_) => 3,
                Value::F64(_) => 4,
                Value::String(_) => 5,
                Value::Uint8List(_) => 6,
                Value::Int32List(_) => 7,
                Value::Int64List(_) => 8,
                Value::Float32List(_) => 9,
                Value::Float64List(_) => 10,
                Value::List(_) => 11,
                Value::Map(_) => 12,
            }
        };
        match rank(self).cmp(&rank(other)) {
            Ordering::Equal => match (self, other) {
                (Value::Null, Value::Null) => Ordering::Equal,
                (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
                (Value::I32(a), Value::I32(b)) => a.cmp(b),
                (Value::I64(a), Value::I64(b)) => a.cmp(b),
                (Value::F64(a), Value::F64(b)) => a.to_bits().cmp(&b.to_bits()),
                (Value::String(a), Value::String(b)) => a.cmp(b),
                (Value::Uint8List(a), Value::Uint8List(b)) => a.cmp(b),
                (Value::Int32List(a), Value::Int32List(b)) => a.cmp(b),
                (Value::Int64List(a), Value::Int64List(b)) => a.cmp(b),
                (Value::Float32List(a), Value::Float32List(b)) => a
                    .iter()
                    .map(|x| x.to_bits())
                    .cmp(b.iter().map(|x| x.to_bits())),
                (Value::Float64List(a), Value::Float64List(b)) => a
                    .iter()
                    .map(|x| x.to_bits())
                    .cmp(b.iter().map(|x| x.to_bits())),
                (Value::List(a), Value::List(b)) => a.cmp(b),
                (Value::Map(a), Value::Map(b)) => a.cmp(b),
                _ => Ordering::Equal,
            },
            other => other,
        }
    }
}

impl Eq for Value {}

/// Codec/input error. Flutter throws `FormatException('Message corrupted')`
/// for any malformed value and `ArgumentError` for an unsupported write
/// (`message_codecs.dart:337-345,464-469,515-536`); both map here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecError {
    Corrupt(&'static str),
    Unsupported(&'static str),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodecError::Corrupt(why) => write!(f, "FormatException: Message corrupted ({why})"),
            CodecError::Unsupported(why) => write!(f, "ArgumentError: {why}"),
        }
    }
}

impl std::error::Error for CodecError {}

// ------------------
// StandardMessageCodec — wire format
// ------------------

const TAG_NULL: u8 = 0;
const TAG_TRUE: u8 = 1;
const TAG_FALSE: u8 = 2;
const TAG_I32: u8 = 3;
const TAG_I64: u8 = 4;
const TAG_LARGE_INT: u8 = 5;
const TAG_F64: u8 = 6;
const TAG_STRING: u8 = 7;
const TAG_U8_LIST: u8 = 8;
const TAG_I32_LIST: u8 = 9;
const TAG_I64_LIST: u8 = 10;
const TAG_F64_LIST: u8 = 11;
const TAG_LIST: u8 = 12;
const TAG_MAP: u8 = 13;
const TAG_F32_LIST: u8 = 14;

/// Writer mirroring `WriteBuffer` alignment: `putFloat64` inserts zero bytes
/// until the absolute position is 8-byte aligned (`message_codecs.dart:289-290,
/// :396`, typed lists at `:440-452`).
struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn align(&mut self, alignment: usize) {
        while self.bytes.len() % alignment != 0 {
            self.bytes.push(0);
        }
    }

    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn i32(&mut self, value: i32) {
        self.bytes.extend_from_slice(&value.to_ne_bytes());
    }

    fn i64(&mut self, value: i64) {
        self.bytes.extend_from_slice(&value.to_ne_bytes());
    }

    fn f64(&mut self, value: f64) {
        self.align(8);
        self.bytes.extend_from_slice(&value.to_ne_bytes());
    }

    fn f32(&mut self, value: f32) {
        self.align(4);
        self.bytes.extend_from_slice(&value.to_ne_bytes());
    }

    /// Expanding size: 0..253 one byte, 254..65535 tag 254 + u16, else tag 255
    /// + u32 (`message_codecs.dart:545-562`).
    fn size(&mut self, value: usize) -> Result<(), CodecError> {
        let value = u32::try_from(value).map_err(|_| CodecError::Unsupported("size > u32"))?;
        if value < 254 {
            self.u8(value as u8);
        } else if value <= 0xffff {
            self.u8(254);
            self.bytes.extend_from_slice(&(value as u16).to_ne_bytes());
        } else {
            self.u8(255);
            self.bytes.extend_from_slice(&value.to_ne_bytes());
        }
        Ok(())
    }

    fn value(&mut self, value: &Value) -> Result<(), CodecError> {
        self.u8(value.tag());
        match value {
            Value::Null | Value::Bool(_) => {}
            Value::I32(n) => self.i32(*n),
            Value::I64(n) => self.i64(*n),
            Value::F64(n) => self.f64(*n),
            Value::String(s) => {
                let bytes = s.as_bytes();
                self.size(bytes.len())?;
                self.bytes.extend_from_slice(bytes);
            }
            Value::Uint8List(items) => {
                self.size(items.len())?;
                self.align(1);
                self.bytes.extend_from_slice(items);
            }
            Value::Int32List(items) => {
                self.size(items.len())?;
                self.align(4);
                for item in items {
                    self.i32(*item);
                }
            }
            Value::Int64List(items) => {
                self.size(items.len())?;
                self.align(8);
                for item in items {
                    self.i64(*item);
                }
            }
            Value::Float32List(items) => {
                self.size(items.len())?;
                self.align(4);
                for item in items {
                    self.f32(*item);
                }
            }
            Value::Float64List(items) => {
                self.size(items.len())?;
                self.align(8);
                for item in items {
                    self.f64(*item);
                }
            }
            Value::List(items) => {
                self.size(items.len())?;
                for item in items {
                    self.value(item)?;
                }
            }
            Value::Map(entries) => {
                self.size(entries.len())?;
                for (key, val) in entries {
                    self.value(key)?;
                    self.value(val)?;
                }
            }
        }
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        let end = self
            .at
            .checked_add(n)
            .ok_or(CodecError::Corrupt("overflow"))?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or(CodecError::Corrupt("truncated"))?;
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> Result<i32, CodecError> {
        let raw: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(i32::from_ne_bytes(raw))
    }

    fn i64(&mut self) -> Result<i64, CodecError> {
        let raw: [u8; 8] = self.take(8)?.try_into().unwrap();
        Ok(i64::from_ne_bytes(raw))
    }

    fn align(&mut self, alignment: usize) -> Result<(), CodecError> {
        let padding = (alignment - (self.at % alignment)) % alignment;
        if self.take(padding).is_err() {
            return Err(CodecError::Corrupt("alignment padding"));
        }
        Ok(())
    }

    fn f64(&mut self) -> Result<f64, CodecError> {
        self.align(8)?;
        let raw: [u8; 8] = self.take(8)?.try_into().unwrap();
        Ok(f64::from_ne_bytes(raw))
    }

    fn f32(&mut self) -> Result<f32, CodecError> {
        self.align(4)?;
        let raw: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(f32::from_ne_bytes(raw))
    }

    fn size(&mut self) -> Result<usize, CodecError> {
        match self.u8()? {
            254 => {
                let raw: [u8; 2] = self.take(2)?.try_into().unwrap();
                Ok(u16::from_ne_bytes(raw) as usize)
            }
            255 => {
                let raw: [u8; 4] = self.take(4)?.try_into().unwrap();
                Ok(u32::from_ne_bytes(raw) as usize)
            }
            byte => Ok(byte as usize),
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, CodecError> {
        if depth > 128 {
            return Err(CodecError::Corrupt("recursion limit"));
        }
        let tag = self.u8()?;
        match tag {
            TAG_NULL => Ok(Value::Null),
            TAG_TRUE => Ok(Value::Bool(true)),
            TAG_FALSE => Ok(Value::Bool(false)),
            TAG_I32 => Ok(Value::I32(self.i32()?)),
            TAG_I64 => Ok(Value::I64(self.i64()?)),
            // Tag 5 decodes the ASCII hex form of a large integer
            // (`message_codecs.dart:379-385,503-505`). Opurom's closed value
            // universe has no bignum; reject it rather than invent a type.
            TAG_LARGE_INT => Err(CodecError::Unsupported("large integer")),
            TAG_F64 => Ok(Value::F64(self.f64()?)),
            TAG_STRING => {
                let len = self.size()?;
                let bytes = self.take(len)?;
                String::from_utf8(bytes.to_vec())
                    .map(Value::String)
                    .map_err(|_| CodecError::Corrupt("invalid UTF-8"))
            }
            TAG_U8_LIST => {
                let len = self.size()?;
                Ok(Value::Uint8List(self.take(len)?.to_vec()))
            }
            TAG_I32_LIST => {
                let len = self.size()?;
                self.align(4)?;
                let mut out = Vec::with_capacity(len.min(1 << 20));
                for _ in 0..len {
                    out.push(self.i32()?);
                }
                Ok(Value::Int32List(out))
            }
            TAG_I64_LIST => {
                let len = self.size()?;
                self.align(8)?;
                let mut out = Vec::with_capacity(len.min(1 << 20));
                for _ in 0..len {
                    out.push(self.i64()?);
                }
                Ok(Value::Int64List(out))
            }
            TAG_F32_LIST => {
                let len = self.size()?;
                self.align(4)?;
                let mut out = Vec::with_capacity(len.min(1 << 20));
                for _ in 0..len {
                    out.push(self.f32()?);
                }
                Ok(Value::Float32List(out))
            }
            TAG_F64_LIST => {
                let len = self.size()?;
                self.align(8)?;
                let mut out = Vec::with_capacity(len.min(1 << 20));
                for _ in 0..len {
                    out.push(self.f64()?);
                }
                Ok(Value::Float64List(out))
            }
            TAG_LIST => {
                let len = self.size()?;
                let mut out = Vec::with_capacity(len.min(1 << 20));
                for _ in 0..len {
                    out.push(self.value(depth + 1)?);
                }
                Ok(Value::List(out))
            }
            TAG_MAP => {
                let len = self.size()?;
                let mut out = BTreeMap::new();
                for _ in 0..len {
                    let key = self.value(depth + 1)?;
                    let val = self.value(depth + 1)?;
                    out.insert(key, val);
                }
                Ok(Value::Map(out))
            }
            _ => Err(CodecError::Corrupt("unknown tag")),
        }
    }
}

/// `StandardMessageCodec.encodeMessage(null)` returns `null`, not zero bytes
/// (`message_codecs.dart:327-332`). `None` means "no message" throughout this
/// module, matching the Dart `ByteData?` type.
pub fn encode_message(value: &Value) -> Result<Option<Vec<u8>>, CodecError> {
    if matches!(value, Value::Null) {
        return Ok(None);
    }
    let mut writer = Writer::new();
    writer.value(value)?;
    Ok(Some(writer.bytes))
}

/// Decodes a value and rejects trailing bytes, matching
/// `message_codecs.dart:337-345`.
pub fn decode_message(bytes: Option<&[u8]>) -> Result<Value, CodecError> {
    let Some(bytes) = bytes else {
        return Ok(Value::Null);
    };
    let mut reader = Reader::new(bytes);
    let value = reader.value(0)?;
    if reader.at != bytes.len() {
        return Err(CodecError::Corrupt("trailing bytes"));
    }
    Ok(value)
}

// ------------------
// StandardMethodCodec — method call and reply envelopes
// ------------------

/// A named method call, wire form = `writeValue(method)` + `writeValue(args)`
/// with no trailing bytes (`message_codecs.dart:600-617`).
#[derive(Clone, Debug, PartialEq)]
pub struct MethodCall {
    pub method: String,
    pub arguments: Value,
}

/// Typed reply, mirroring Flutter's success / `PlatformException` / null-reply
/// distinction (`platform_channel.dart:350-366`, `message_codecs.dart:620-657`).
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// Success envelope (leading byte 0).
    Success(Value),
    /// Error envelope (leading byte 1): code, message, details.
    ///
    /// Flutter sends an optional fourth stacktrace string on the wire; base
    /// `encodeErrorEnvelope` never writes one. We accept and decode it in
    /// [`decode_envelope`] but do not synthesize one.
    PlatformError {
        code: String,
        message: Option<String>,
        details: Value,
    },
    /// No reply bytes. `MethodChannel` turns this into `MissingPluginException`;
    /// `OptionalMethodChannel` turns it into `null` (`:622-633`). The channel
    /// type decides which; the codec cannot.
    MissingPlugin,
}

pub fn encode_method_call(call: &MethodCall) -> Result<Vec<u8>, CodecError> {
    let mut writer = Writer::new();
    writer.value(&Value::String(call.method.clone()))?;
    writer.value(&call.arguments)?;
    Ok(writer.bytes)
}

pub fn decode_method_call(bytes: &[u8]) -> Result<MethodCall, CodecError> {
    let mut reader = Reader::new(bytes);
    let method = match reader.value(0)? {
        Value::String(s) => s,
        _ => return Err(CodecError::Corrupt("method name is not a string")),
    };
    let arguments = reader.value(0)?;
    if reader.at != bytes.len() {
        return Err(CodecError::Corrupt("trailing bytes in method call"));
    }
    Ok(MethodCall { method, arguments })
}

pub fn encode_envelope(reply: &Reply) -> Result<Option<Vec<u8>>, CodecError> {
    match reply {
        Reply::MissingPlugin => Ok(None),
        Reply::Success(value) => {
            let mut writer = Writer::new();
            writer.u8(0);
            writer.value(value)?;
            Ok(Some(writer.bytes))
        }
        Reply::PlatformError {
            code,
            message,
            details,
        } => {
            let mut writer = Writer::new();
            writer.u8(1);
            writer.value(&Value::String(code.clone()))?;
            writer.value(&Value::String(message.clone().unwrap_or_default()))?;
            writer.value(details)?;
            Ok(Some(writer.bytes))
        }
    }
}

pub fn decode_envelope(bytes: Option<&[u8]>) -> Result<Reply, CodecError> {
    let Some(bytes) = bytes else {
        return Ok(Reply::MissingPlugin);
    };
    let mut reader = Reader::new(bytes);
    match reader.u8()? {
        0 => {
            let value = reader.value(0)?;
            if reader.at != bytes.len() {
                return Err(CodecError::Corrupt("trailing bytes in success envelope"));
            }
            Ok(Reply::Success(value))
        }
        1 => {
            let code = match reader.value(0)? {
                Value::String(s) => s,
                _ => return Err(CodecError::Corrupt("error code is not a string")),
            };
            let message = match reader.value(0)? {
                Value::Null => None,
                Value::String(s) => Some(s),
                _ => return Err(CodecError::Corrupt("error message is not a string")),
            };
            let details = reader.value(0)?;
            if reader.at < bytes.len() {
                // Flutter reads an optional stacktrace String here
                // (`message_codecs.dart:653-655`). Consume it but do not expose
                // it: Opurom has no Dart stack to report.
                match reader.value(0)? {
                    Value::Null | Value::String(_) => {}
                    _ => return Err(CodecError::Corrupt("invalid stacktrace")),
                }
            }
            if reader.at != bytes.len() {
                return Err(CodecError::Corrupt("trailing bytes in error envelope"));
            }
            Ok(Reply::PlatformError {
                code,
                message,
                details,
            })
        }
        _ => Err(CodecError::Corrupt("invalid envelope tag")),
    }
}

// ------------------
// Named channels, handler table and bounded FIFO
// ------------------

/// Stable error codes for the channel boundary. Flutter uses the string code
/// in its error envelope; these constants avoid ad-hoc strings in Opurom.
/// `MISSING_PLUGIN` is only ever a null reply in Flutter
/// (`platform_channel.dart:363`), never an error envelope.
pub mod error_codes {
    pub const MISSING_PLUGIN: &str = "missing_plugin";
    pub const UNIMPLEMENTED: &str = "unimplemented";
    pub const INVALID_ARGUMENT: &str = "invalid_argument";
    pub const ERROR: &str = "error";
}

/// Compile-time registered handler for one named channel.
///
/// Flutter handlers are mutable closures installed through
/// `BinaryMessenger.setMessageHandler` (`binary_messenger.dart:66-80`). Opurom
/// builds one `ChannelTable` per adapter at startup; there is no runtime
/// registration API, honoring the no-interpreter red line. Replacing a
/// handler is an explicit table replacement, not a mutation of a global map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    /// `BasicMessageChannel<T>` (`platform_channel.dart:202-256`).
    BasicMessage,
    /// `MethodChannel` / `OptionalMethodChannel` (`:291-633`). The
    /// `optional` flag selects null-reply-as-`null` instead of throwing.
    Method { optional: bool },
    /// `EventChannel` (`:650-738`). Its listen/cancel methods are ordinary
    /// method calls on the same name; events use success/error envelopes.
    Event,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelSpec {
    pub name: &'static str,
    pub kind: ChannelKind,
}

/// A handler is a named function pointer plus an opaque pointer to static
/// state. `unsafe` is confined to the table: the caller must provide a state
/// pointer that outlives the table (typically `&'static`), keeping the ABI
/// deterministic and free of runtime allocation for handler lookup.
pub type HandlerFn = unsafe fn(*mut u8, &[u8]) -> Result<Option<Vec<u8>>, CodecError>;

#[derive(Clone, Copy)]
pub struct Handler {
    pub function: HandlerFn,
    pub state: *mut u8,
}

impl fmt::Debug for Handler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handler")
            .field("state", &self.state)
            .finish()
    }
}

impl PartialEq for Handler {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state && self.function as usize == other.function as usize
    }
}
impl Eq for Handler {}

/// Immutable-by-convention channel table. `&self` lookups cannot mutate
/// registration; rebuilding the table is the explicit replacement mechanism.
#[derive(Default)]
pub struct ChannelTable {
    entries: BTreeMap<&'static str, (ChannelSpec, Handler)>,
}

impl ChannelTable {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Register a channel. Duplicate names mirror Flutter's "last handler
    /// wins" `setMessageHandler` semantics (`binding.dart:654-675`),
    /// but the whole table is built before the event loop starts. The
    /// name-keyed index mirrors `ChannelBuffers._channels` (`channel_buffers.dart:330,382-385`).
    pub fn with(mut self, spec: ChannelSpec, handler: Handler) -> Self {
        self.entries.insert(spec.name, (spec, handler));
        self
    }

    pub fn spec(&self, name: &str) -> Option<&ChannelSpec> {
        self.entries.get(name).map(|(spec, _)| spec)
    }

    /// Deterministic name order, matching the sorted `BTreeMap` keys.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.entries.keys().copied()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Dispatches encoded bytes by channel name, returning encoded reply
    /// bytes. A missing handler returns `None`, exactly Flutter's null reply
    /// which becomes `MissingPluginException` on a strict `MethodChannel`.
    ///
    /// # Safety
    /// The caller must ensure each handler's `state` pointer is valid for the
    /// lifetime of this call. Build tables from `&'static` state or guarantee
    /// the owner outlives every dispatch.
    pub unsafe fn dispatch(
        &self,
        name: &str,
        message: &[u8],
    ) -> Result<Option<Vec<u8>>, CodecError> {
        let Some((_, handler)) = self.entries.get(name) else {
            return Ok(None);
        };
        unsafe { (handler.function)(handler.state, message) }
    }
}

/// A bounded per-channel FIFO mirroring `ChannelBuffers._Channel`
/// (`engine/src/flutter/lib/ui/channel_buffers.dart:75-81,131-144,198`).
/// Messages pushed before a listener drains them are queued FIFO; overflow
/// drops the oldest and reports the count.
pub struct ChannelBuffer<T> {
    name: &'static str,
    capacity: usize,
    queue: std::collections::VecDeque<(T, u64)>,
    dropped: u64,
}

impl<T> ChannelBuffer<T> {
    /// Flutter's `ChannelBuffers.kDefaultBufferSize = 1` (`:321`).
    pub const DEFAULT_CAPACITY: usize = 1;

    pub fn new(name: &'static str) -> Self {
        Self::with_capacity(name, Self::DEFAULT_CAPACITY)
    }

    pub fn with_capacity(name: &'static str, capacity: usize) -> Self {
        assert!(capacity > 0, "channel capacity must be positive");
        Self {
            name,
            capacity,
            queue: std::collections::VecDeque::with_capacity(capacity),
            dropped: 0,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Returns `true` when the message was accepted. Flutter's
    /// `_dropOverflowMessages` removes *all* entries beyond `capacity - 1`
    /// (`channel_buffers.dart:144,162-170`); the Opurom counter records how
    /// many older messages were dropped. Capacity is always positive here.
    pub fn push(&mut self, message: T) -> bool {
        while self.queue.len() >= self.capacity {
            self.queue.pop_front();
            self.dropped += 1;
        }
        let sequence = self
            .queue
            .back()
            .map(|(_, s)| s.saturating_add(1))
            .unwrap_or(0);
        self.queue.push_back((message, sequence));
        true
    }

    pub fn pop(&mut self) -> Option<T> {
        self.queue.pop_front().map(|(value, _)| value)
    }

    /// Stamps an incoming message with a monotonic sequence number so
    /// consumers can detect FIFO order without wall-clock time.
    pub fn push_stamped(&mut self, message: T, sequence: u64) -> bool {
        while self.queue.len() >= self.capacity {
            self.queue.pop_front();
            self.dropped += 1;
        }
        self.queue.push_back((message, sequence));
        true
    }

    pub fn pop_stamped(&mut self) -> Option<(T, u64)> {
        self.queue.pop_front()
    }
}

/// Standard channel names from Flutter's `SystemChannels`
/// (`system_channels.dart:181,385,423,430`). Opurom uses the same names so a
/// future wire bridge can interoperate without renaming.
pub mod system_channels {
    /// `services/system_channels.dart:401-404` (JSONMessageCodec).
    pub const SYSTEM: &str = "flutter/system";
    pub const LIFECYCLE: &str = "flutter/lifecycle";
    pub const PLATFORM: &str = "flutter/platform";
    pub const PLATFORM_VIEWS: &str = "flutter/platform_views";
    pub const PLATFORM_VIEWS_2: &str = "flutter/platform_views_2";
}

/// EventChannel listen/cancel method names (`platform_channel.dart:710,725`).
pub const EVENT_LISTEN: &str = "listen";
pub const EVENT_CANCEL: &str = "cancel";
