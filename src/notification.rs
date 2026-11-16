//! The notification capability: one flat contract — [`NotificationHost::post`]
//! an identified banner, `cancel` it, and ask about permission — mirrored by
//! the Android notification bridge, so one .opu program drives the same four
//! primitives on desktop and on a device. The portable half is `std` only; the
//! macOS adapter posts through the system scripting tool.
//!
//! Determinism: arguments are validated before any OS call, so an invalid
//! `post` fails identically on every target, and [`HeadlessNotifier`] records
//! the stream FIFO with no wall clock as the reference model. On the script
//! channel, `cancel` and the permission calls are typed
//! [`NotifyError::Unsupported`] reasons instead of silent successes.

/// Typed notification failures; a rejected or unsupported call never reports
/// a silent success. The Android bridge maps its platform errors onto these
/// same variants so callers classify failures identically on both sides.
#[derive(Debug)]
pub enum NotifyError {
    /// The OS refused permission (a denied request, or a post without it).
    PermissionDenied,
    /// This target has no path for the operation in 0.1.0; carries the reason.
    Unsupported(String),
    /// The arguments violate the contract; carries which rule failed.
    Invalid(String),
    /// The OS reported a failure carrying its message.
    Internal(String),
}

impl std::fmt::Display for NotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotifyError::PermissionDenied => write!(f, "notification permission denied"),
            NotifyError::Unsupported(reason) => write!(f, "notification unsupported: {reason}"),
            NotifyError::Invalid(reason) => write!(f, "invalid notification: {reason}"),
            NotifyError::Internal(message) => {
                write!(f, "notification backend error: {message}")
            }
        }
    }
}

impl std::error::Error for NotifyError {}

/// One identified banner: the bridge's `post(id, channel, title, body)`. The
/// id names the banner for `cancel`; the channel groups it on the device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub id: i32,
    pub channel: String,
    pub title: String,
    pub body: String,
}

impl Notification {
    pub fn new(
        id: i32,
        channel: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Notification {
        Notification {
            id,
            channel: channel.into(),
            title: title.into(),
            body: body.into(),
        }
    }

    /// Structural validation shared by every adapter: the title and channel
    /// carry text, so a blank banner is a typed error, never a blank OS call.
    /// The body may be empty — the title alone still names the banner.
    pub fn validate(&self) -> Result<(), NotifyError> {
        if self.title.trim().is_empty() {
            return Err(NotifyError::Invalid("title is empty".to_string()));
        }
        if self.channel.trim().is_empty() {
            return Err(NotifyError::Invalid("channel is empty".to_string()));
        }
        Ok(())
    }
}

/// The one notification ABI the bridge mirrors, method for method
/// (`cancel_all` travels the wire as `cancelAll`). Clicks and actions are out
/// of scope for 0.1.0.
pub trait NotificationHost {
    fn post(&mut self, id: i32, channel: &str, title: &str, body: &str) -> Result<(), NotifyError>;
    fn cancel(&mut self, id: i32) -> Result<(), NotifyError>;
    fn cancel_all(&mut self) -> Result<(), NotifyError>;
    fn has_permission(&self) -> Result<bool, NotifyError>;
    fn request_permission(&mut self) -> Result<bool, NotifyError>;
}

/// The deterministic reference notifier: banners are recorded in delivery
/// order with no OS surface, so tests drive the exact stream the real
/// adapters promise. One banner lives per id — a repost updates it in place,
/// matching how a device notification manager treats the same id.
#[derive(Default)]
pub struct HeadlessNotifier {
    delivered: Vec<Notification>,
}

impl HeadlessNotifier {
    pub fn new() -> HeadlessNotifier {
        HeadlessNotifier::default()
    }

    /// Every live banner, FIFO by first post.
    pub fn delivered(&self) -> &[Notification] {
        &self.delivered
    }
}

impl NotificationHost for HeadlessNotifier {
    fn post(&mut self, id: i32, channel: &str, title: &str, body: &str) -> Result<(), NotifyError> {
        let notification = Notification::new(id, channel, title, body);
        notification.validate()?;
        match self.delivered.iter_mut().find(|note| note.id == id) {
            Some(slot) => *slot = notification,
            None => self.delivered.push(notification),
        }
        Ok(())
    }

    fn cancel(&mut self, id: i32) -> Result<(), NotifyError> {
        self.delivered.retain(|note| note.id != id);
        Ok(())
    }

    fn cancel_all(&mut self) -> Result<(), NotifyError> {
        self.delivered.clear();
        Ok(())
    }

    // The model has no OS that could deny it: always granted.
    fn has_permission(&self) -> Result<bool, NotifyError> {
        Ok(true)
    }

    fn request_permission(&mut self) -> Result<bool, NotifyError> {
        Ok(true)
    }
}

#[cfg(target_os = "macos")]
mod osascript_notice;
#[cfg(target_os = "macos")]
pub use osascript_notice::OsascriptNotice;

/// The AppleScript source that posts one banner. Public so a caller can audit
/// exactly what text leaves the process, and so the escaping runs under test
/// on every host. The scripting language has no `\\n` escape, so line breaks
/// rejoin with its `linefeed` constant; quotes and backslashes are escaped so
/// no title or body can break out of its literal.
pub fn applescript_source(notification: &Notification) -> String {
    fn literal(value: &str) -> String {
        let mut segments: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut chars = value.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\r' => {
                    // Treat CRLF as one break, not two.
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    segments.push(std::mem::take(&mut current));
                }
                '\n' => segments.push(std::mem::take(&mut current)),
                '\\' | '"' => {
                    current.push('\\');
                    current.push(ch);
                }
                _ => current.push(ch),
            }
        }
        segments.push(current);
        if segments.len() == 1 {
            return format!("\"{}\"", segments[0]);
        }
        segments
            .iter()
            .map(|segment| format!("\"{segment}\""))
            .collect::<Vec<_>>()
            .join(" & linefeed & ")
    }

    format!(
        "display notification {} with title {}",
        literal(&notification.body),
        literal(&notification.title)
    )
}

/// The running platform's real notifier, or the typed reason it has none.
/// Linux and Windows have no OS adapter in 0.1.0, so they degrade with an
/// explicit reason instead of a silent no-op.
pub fn platform_notifier() -> Result<Box<dyn NotificationHost>, NotifyError> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(OsascriptNotice::new()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(NotifyError::Unsupported(
            "no notification adapter implemented on this platform in 0.1.0".to_string(),
        ))
    }
}
