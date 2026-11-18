//! Background-isolate messaging contract and deterministic task envelope.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Flutter does not define a generic background *task scheduler* in the
//! framework. It defines how a non-root isolate reaches platform channels:
//! `ServicesBinding.rootIsolateToken`
//! (`services/binding.dart:115-122`) is passed to
//! `BackgroundIsolateBinaryMessenger.ensureInitialized`
//! (`services/_background_isolate_binary_messenger_io.dart:15-63`), which
//! assigns a monotonically increasing message identifier (`:76-89`) and
//! routes replies through a receive port. Setting a message handler in a
//! background isolate is explicitly unsupported (`:92-96`).
//!
//! Opurom keeps exactly that mechanism without isolates or runtime handler
//! registries: a background task carries an opaque root-isolate token, a
//! monotonic sequence, a named channel and an already-encoded message. The
//! scheduler is host-injected; this module validates the envelope and keeps
//! replies correlated to the same sequence. No timer, thread or global map
//! lives in the runtime core.

use std::collections::BTreeMap;

use crate::platform_channel::{error_codes, CodecError, Reply};

/// Token that ties a background worker to the root isolate. Flutter's
/// `RootIsolateToken.instance` is opaque; Opurom uses a non-zero u64 so
/// `0` can represent "not initialized".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RootToken(pub u64);

impl RootToken {
    pub fn new(value: u64) -> Result<Self, CodecError> {
        if value == 0 {
            return Err(CodecError::Corrupt("root token must not be zero"));
        }
        Ok(Self(value))
    }
}

/// One outbound background message. The identifier is the Flutter
/// `_messageCount` successor (`_background_isolate_binary_messenger_io.dart:79-83`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundMessage {
    pub identifier: u64,
    pub channel: String,
    pub message: Vec<u8>,
}

/// A background messenger with a deterministic sequence and explicit reply
/// correlation. It does not own a runtime stream: a host drains queued
/// messages and feeds replies back by identifier.
#[derive(Clone, Debug, Default)]
pub struct BackgroundMessenger {
    token: Option<RootToken>,
    next_identifier: u64,
    pending: BTreeMap<u64, BackgroundMessage>,
    replies: BTreeMap<u64, Vec<u8>>,
}

impl BackgroundMessenger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Idempotent, matching Flutter's `ensureInitialized`: a second token
    /// does not reset the sequence or drop pending messages (`:45-63`).
    pub fn ensure_initialized(&mut self, token: RootToken) -> bool {
        if self.token.is_some() {
            return false;
        }
        self.token = Some(token);
        true
    }

    pub fn token(&self) -> Option<RootToken> {
        self.token
    }

    /// Queue one message. Returns the assigned identifier. An uninitialized
    /// messenger is a typed error, just as Flutter's `instance` throws
    /// `StateError` before `ensureInitialized` (`:21-29`).
    pub fn send(
        &mut self,
        channel: impl Into<String>,
        message: Vec<u8>,
    ) -> Result<u64, CodecError> {
        if self.token.is_none() {
            return Err(CodecError::Corrupt(
                "background messenger is not initialized",
            ));
        }
        self.next_identifier = self.next_identifier.saturating_add(1);
        let identifier = self.next_identifier;
        self.pending.insert(
            identifier,
            BackgroundMessage {
                identifier,
                channel: channel.into(),
                message,
            },
        );
        Ok(identifier)
    }

    pub fn pending(&self) -> &BTreeMap<u64, BackgroundMessage> {
        &self.pending
    }

    /// Feed one reply for a previously sent message. Unknown identifiers are
    /// rejected instead of creating a phantom task.
    pub fn reply(&mut self, identifier: u64, bytes: Vec<u8>) -> Result<(), CodecError> {
        if !self.pending.contains_key(&identifier) {
            return Err(CodecError::Corrupt("unknown background message identifier"));
        }
        self.pending.remove(&identifier);
        self.replies.insert(identifier, bytes);
        Ok(())
    }

    /// Drain a reply by identifier. Flutter resolves the matching completer;
    /// Opurom hands the bytes back to the caller at the same correlation point.
    pub fn take_reply(&mut self, identifier: u64) -> Option<Vec<u8>> {
        self.replies.remove(&identifier)
    }

    /// Background isolates cannot install message handlers
    /// (`_background_isolate_binary_messenger_io.dart:92-96`). This typed
    /// error keeps that rule visible instead of silently accepting one.
    pub fn set_message_handler_unsupported() -> Reply {
        Reply::PlatformError {
            code: error_codes::UNIMPLEMENTED.to_owned(),
            message: Some("background isolates do not support setMessageHandler".to_owned()),
            details: crate::platform_channel::Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_rejects_the_uninitialized_value() {
        assert_eq!(
            RootToken::new(0),
            Err(CodecError::Corrupt("root token must not be zero"))
        );
        assert_eq!(RootToken::new(7), Ok(RootToken(7)));
    }

    #[test]
    fn ensure_initialized_is_idempotent_and_sequence_is_monotonic() {
        let mut messenger = BackgroundMessenger::new();
        assert!(messenger.send("a", vec![1]).is_err());
        assert!(messenger.ensure_initialized(RootToken(1)));
        assert!(!messenger.ensure_initialized(RootToken(2)));
        assert_eq!(messenger.token(), Some(RootToken(1)));
        assert_eq!(messenger.send("a", vec![1]), Ok(1));
        assert_eq!(messenger.send("b", vec![2]), Ok(2));
        assert_eq!(messenger.pending()[&1].channel, "a");
        assert_eq!(messenger.pending()[&2].message, vec![2]);
    }

    #[test]
    fn replies_correlate_by_identifier_and_unknown_ids_are_rejected() {
        let mut messenger = BackgroundMessenger::new();
        messenger.ensure_initialized(RootToken(9));
        let id = messenger.send("background.download", vec![7, 8]).unwrap();
        assert_eq!(
            messenger.reply(id + 1, vec![0]),
            Err(CodecError::Corrupt("unknown background message identifier"))
        );
        assert_eq!(messenger.reply(id, vec![42]), Ok(()));
        assert!(messenger.pending().is_empty());
        assert_eq!(messenger.take_reply(id), Some(vec![42]));
        assert_eq!(messenger.take_reply(id), None);
    }

    #[test]
    fn handler_installation_stays_unsupported() {
        assert_eq!(
            BackgroundMessenger::set_message_handler_unsupported(),
            Reply::PlatformError {
                code: error_codes::UNIMPLEMENTED.to_owned(),
                message: Some("background isolates do not support setMessageHandler".to_owned()),
                details: crate::platform_channel::Value::Null,
            }
        );
    }
}
