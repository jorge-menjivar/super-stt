// SPDX-License-Identifier: GPL-3.0-only
//! `POST /tts/speak/read`: speak the text the read method names, the
//! highlighted text or the clipboard.
//!
//! A shortcut is what asks for this, so nobody is looking at the response.
//! Every refusal therefore also gets a desktop notice, the way
//! [`handle_speak`](SuperTTSDaemon::handle_speak) gives one to the failures
//! its caller did not cause. Here the caller caused none of them: the text
//! comes from the screen, not from the request.

use crate::tts::daemon::speech::MAX_TEXT_CHARS;
use crate::tts::daemon::types::SuperTTSDaemon;
use crate::tts::input::screen_text;
use crate::tts::output::notice::Failure;
use log::{debug, warn};
use std::time::Duration;
use super_voice_shared::models::read_method::ReadMethod;
use super_voice_shared::tts::models::protocol::{DaemonResponse, ErrorCode};

/// The most text read from the screen: [`MAX_TEXT_CHARS`] of the widest UTF-8
/// characters, and one byte more, so more than an utterance may be is always
/// seen as too long rather than cut off.
const MAX_READ_BYTES: u64 = MAX_TEXT_CHARS as u64 * 4 + 1;

/// How long the app that owns the text has to hand it over. An app that never
/// does would otherwise hold the request open for good.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

impl SuperTTSDaemon {
    /// Speak the text the read method names. See the [module docs](self).
    pub async fn handle_speak_read(&self) -> DaemonResponse {
        let method = self.config.read().await.synthesis.read_method;
        let read = tokio::time::timeout(
            READ_TIMEOUT,
            tokio::task::spawn_blocking(move || screen_text::read(method, MAX_READ_BYTES)),
        )
        .await;
        let read = match read {
            Ok(Ok(read)) => read,
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("the app holding the text did not hand it over".to_string()),
        };
        self.speak_what_was_read(method, read).await
    }

    /// The half of [`handle_speak_read`](Self::handle_speak_read) after the
    /// read, apart so a test can hand it what a read returned.
    pub(crate) async fn speak_what_was_read(
        &self,
        method: ReadMethod,
        read: Result<Option<String>, String>,
    ) -> DaemonResponse {
        let text = match read {
            Ok(Some(text)) => text,
            Ok(None) => {
                return self
                    .refuse_read(
                        Failure::nothing_to_read(method),
                        ErrorCode::NothingToRead,
                        "nothing_to_read",
                    )
                    .await;
            }
            Err(detail) => {
                warn!("Could not read the {method} to speak it: {detail}");
                return self
                    .refuse_read(
                        Failure::could_not_read(method, &detail),
                        ErrorCode::Internal,
                        &format!("could_not_read: {detail}"),
                    )
                    .await;
            }
        };

        debug!("Read {} characters from the {method}", text.chars().count());

        // `handle_speak` refuses these two without a notice, because a caller
        // that sent bad text is looking at the response. Nobody sent this text,
        // so they are checked here, where the user can be told.
        let trimmed = text.trim();
        if trimmed.chars().count() > MAX_TEXT_CHARS {
            return self
                .refuse_read(
                    Failure::too_much_to_read(method),
                    ErrorCode::InvalidValue,
                    "text_too_long",
                )
                .await;
        }
        if crate::tts::text::normalize::normalize(trimmed).is_empty() {
            return self
                .refuse_read(
                    Failure::nothing_to_read(method),
                    ErrorCode::NothingToRead,
                    "nothing_to_read",
                )
                .await;
        }

        self.handle_speak(text).await
    }

    /// Tell the user why nothing was read, and answer the caller the same.
    async fn refuse_read(
        &self,
        failure: Failure,
        code: ErrorCode,
        message: &str,
    ) -> DaemonResponse {
        let method = crate::tts::output::notification::method_for_speech(
            self.config.read().await.notification_method,
        );
        let mut notifier = self.notifier.lock().await;
        crate::tts::output::notification::deliver(method, &mut notifier, &failure).await;
        DaemonResponse::error_with_code(code, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::daemon::types::test_daemon;
    use crate::tts::output::notification::Notifier;
    use std::sync::Arc;

    /// A test daemon whose notices are recorded, with the read method set.
    async fn daemon_reading(
        method: ReadMethod,
    ) -> (SuperTTSDaemon, Arc<std::sync::Mutex<Vec<(String, String)>>>) {
        let daemon = test_daemon().await;
        daemon.config.write().await.synthesis.read_method = method;
        let (notifier, sent) = Notifier::fake(false);
        *daemon.notifier.lock().await = notifier;
        (daemon, sent)
    }

    /// Nothing selected or copied is `nothing_to_read`, and the notice says
    /// what to do for the method in use.
    #[tokio::test]
    async fn nothing_to_read_is_refused_and_noticed() {
        for (method, hint) in [
            (ReadMethod::Selection, "Select some text"),
            (ReadMethod::Clipboard, "Copy some text"),
        ] {
            for read in [Ok(None), Ok(Some("**  **".to_string()))] {
                let (daemon, sent) = daemon_reading(method).await;
                let resp = daemon.speak_what_was_read(method, read.clone()).await;
                assert_eq!(resp.error_code, Some(ErrorCode::NothingToRead), "{read:?}");
                let sent = sent.lock().unwrap();
                assert_eq!(sent.len(), 1, "{read:?}");
                assert_eq!(sent[0].0, "Nothing to read");
                assert!(sent[0].1.starts_with(hint), "{:?}", sent[0]);
            }
        }
    }

    /// A read that fails says which source it was and why.
    #[tokio::test]
    async fn a_failed_read_is_refused_and_noticed() {
        let (daemon, sent) = daemon_reading(ReadMethod::Clipboard).await;
        let resp = daemon
            .speak_what_was_read(ReadMethod::Clipboard, Err("no compositor".to_string()))
            .await;
        assert_eq!(resp.error_code, Some(ErrorCode::Internal));
        assert!(
            resp.message
                .as_deref()
                .is_some_and(|m| m.contains("no compositor")),
            "{:?}",
            resp.message
        );
        let sent = sent.lock().unwrap();
        assert_eq!(
            sent.as_slice(),
            [(
                "Could not read the clipboard".to_string(),
                "no compositor".to_string()
            )]
        );
    }

    /// More than an utterance may be is refused with a notice, rather than
    /// silently, the way `POST /tts/speak` refuses it.
    #[tokio::test]
    async fn too_much_text_is_refused_and_noticed() {
        let (daemon, sent) = daemon_reading(ReadMethod::Selection).await;
        let text = "a".repeat(MAX_TEXT_CHARS + 1);
        let resp = daemon
            .speak_what_was_read(ReadMethod::Selection, Ok(Some(text)))
            .await;
        assert_eq!(resp.error_code, Some(ErrorCode::InvalidValue));
        assert_eq!(sent.lock().unwrap()[0].0, "Too much text to read");
    }

    /// Text that was read goes on to be spoken. With no model loaded that is
    /// `handle_speak`'s refusal and its notice, not one of this module's.
    #[tokio::test]
    async fn text_that_was_read_is_spoken() {
        let (daemon, sent) = daemon_reading(ReadMethod::Selection).await;
        let resp = daemon
            .speak_what_was_read(ReadMethod::Selection, Ok(Some("Hello there.".to_string())))
            .await;
        assert_eq!(resp.error_code, Some(ErrorCode::ModelNotLoaded));
        assert_eq!(sent.lock().unwrap()[0].0, "No model loaded");
    }

    /// The notification setting's `off` silences these notices too.
    #[tokio::test]
    async fn notices_follow_the_notification_setting() {
        use super_voice_shared::models::notification_method::NotificationMethod;
        let (daemon, sent) = daemon_reading(ReadMethod::Selection).await;
        daemon.config.write().await.notification_method = NotificationMethod::Off;
        let resp = daemon
            .speak_what_was_read(ReadMethod::Selection, Ok(None))
            .await;
        assert_eq!(resp.error_code, Some(ErrorCode::NothingToRead));
        assert!(sent.lock().unwrap().is_empty());
    }
}
