// SPDX-License-Identifier: GPL-3.0-only
//! `/transcribe` — streaming speech-to-text via SSE.

use crate::daemon::client::internal::response::require_unit;
use crate::daemon::client::internal::session::{
    APP_ID_NAME, APP_NAME, SETTINGS_SCOPES, with_settings_token,
};
use super_stt_shared::daemon::http_client;
use super_stt_shared::daemon::session;
use super_stt_shared::validation::get_http_socket_path;

/// Result type for streaming record responses.
pub enum RecordEvent {
    /// Intermediate preview text during recording. Snapshot semantics:
    /// the text replaces (not appends to) any previously-displayed
    /// preview.
    Preview(String),
    /// Final transcription result.
    Final(Result<String, String>),
}

/// Start a recording and stream `RecordEvent`s as the daemon emits SSE
/// events on `POST /transcribe`. Closing the returned stream early
/// signals the daemon to stop the recording.
///
/// On `invalid_session` from the initial connect (the cached settings
/// token expired or was revoked), we forget the cache, mint a fresh
/// token, and retry once — mirroring the behavior of
/// [`session::with_token`] for non-streaming calls. Without this,
/// once the cached token went stale the settings UI's transcription
/// test panel would surface `"Error: invalid_session (expired)"`
/// forever, since the cache is never invalidated.
pub fn record_command_stream() -> impl futures_util::Stream<Item = RecordEvent> + Send + 'static {
    cosmic::iced::stream::channel(
        32,
        move |mut channel: cosmic::iced::futures::channel::mpsc::Sender<RecordEvent>| async move {
            use futures_util::{SinkExt, StreamExt};
            use super_stt_shared::daemon::http_client::{
                HttpError, TranscribeEvent, TranscribeOptions,
            };

            let result: Result<(), String> = async {
                let socket = get_http_socket_path();
                let opts = TranscribeOptions {
                    wait: true,
                    write_mode: false,
                    stop_mode: Some("manual-only".to_string()),
                };

                // Try to open the stream with the cached token; on
                // InvalidSession, drop the cache and re-auth once.
                let mut stream = {
                    let token =
                        session::obtain(socket.clone(), APP_ID_NAME, APP_NAME, SETTINGS_SCOPES)
                            .await
                            .map_err(|e| e.to_string())?;
                    match http_client::transcribe_stream(socket.clone(), &token, opts.clone()).await
                    {
                        Ok(s) => Box::pin(s),
                        Err(HttpError::InvalidSession { .. }) => {
                            let _ = session::forget(APP_ID_NAME);
                            let token = session::obtain(
                                socket.clone(),
                                APP_ID_NAME,
                                APP_NAME,
                                SETTINGS_SCOPES,
                            )
                            .await
                            .map_err(|e| e.to_string())?;
                            Box::pin(
                                http_client::transcribe_stream(socket, &token, opts)
                                    .await
                                    .map_err(|e| e.to_string())?,
                            )
                        }
                        Err(e) => return Err(e.to_string()),
                    }
                };

                while let Some(event) = stream.next().await {
                    match event {
                        TranscribeEvent::Preview(text) => {
                            let _ = channel.send(RecordEvent::Preview(text)).await;
                        }
                        TranscribeEvent::Done(text) => {
                            let result = if text.trim().is_empty() {
                                "No speech detected".to_string()
                            } else {
                                text
                            };
                            let _ = channel.send(RecordEvent::Final(Ok(result))).await;
                            return Ok(());
                        }
                        TranscribeEvent::Error(msg) => {
                            let _ = channel.send(RecordEvent::Final(Err(msg))).await;
                            return Ok(());
                        }
                    }
                }
                let _ = channel
                    .send(RecordEvent::Final(Err(
                        "transcribe stream ended unexpectedly".to_string(),
                    )))
                    .await;
                Ok(())
            }
            .await;

            if let Err(e) = result {
                let _ = channel.send(RecordEvent::Final(Err(e))).await;
            }
        },
    )
}

/// Send a stop signal to a running recording (HTTP `POST /transcribe/stop`).
pub async fn stop_record_command() -> Result<(), String> {
    with_settings_token(|socket, token| async move {
        let resp = http_client::transcribe_stop(socket, &token).await?;
        require_unit(resp, "stop_record_command")
    })
    .await
}
