//! macOS adapter: post one banner through the system scripting tool, run as a
//! child of the calling process, so the banner is attributed to the host app
//! and that app's notification permission governs it. Works from a plain CLI,
//! a test binary or a bundled app alike — no bundle identifier required.
//!
//! shortcut: success reports the Apple Event was accepted, not that the banner
//! was seen (this path has no read-back API), and the script channel offers no
//! dismissal or permission calls; switch to UserNotifications.framework when
//! the host runs as an app bundle — a bare executable makes that framework
//! abort with `bundleProxyForCurrentProcess is nil`.
use super::{applescript_source, Notification, NotificationHost, NotifyError};

/// The macOS notification adapter. Stateless: all state lives in the OS.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsascriptNotice;

impl OsascriptNotice {
    pub fn new() -> OsascriptNotice {
        OsascriptNotice
    }
}

impl NotificationHost for OsascriptNotice {
    fn post(&mut self, id: i32, channel: &str, title: &str, body: &str) -> Result<(), NotifyError> {
        let notification = Notification::new(id, channel, title, body);
        notification.validate()?;
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(applescript_source(&notification))
            .output()
            .map_err(|error| NotifyError::Internal(format!("osascript did not run: {error}")))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            return Err(NotifyError::Internal(format!(
                "osascript exited with {}",
                output.status
            )));
        }
        Err(NotifyError::Internal(stderr))
    }

    fn cancel(&mut self, _id: i32) -> Result<(), NotifyError> {
        Err(NotifyError::Unsupported(
            "the script channel cannot dismiss a posted banner in 0.1.0".to_string(),
        ))
    }

    fn cancel_all(&mut self) -> Result<(), NotifyError> {
        Err(NotifyError::Unsupported(
            "the script channel cannot dismiss a posted banner in 0.1.0".to_string(),
        ))
    }

    fn has_permission(&self) -> Result<bool, NotifyError> {
        Err(NotifyError::Unsupported(
            "the script channel cannot query notification permission in 0.1.0".to_string(),
        ))
    }

    fn request_permission(&mut self) -> Result<bool, NotifyError> {
        Err(NotifyError::Unsupported(
            "the script channel has no permission prompt in 0.1.0".to_string(),
        ))
    }
}
