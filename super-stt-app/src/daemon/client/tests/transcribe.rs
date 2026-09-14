// SPDX-License-Identifier: GPL-3.0-only
//! `v1/transcribe` — the settings page's test-recording panel.

use futures_util::StreamExt;
use serde_json::json;
use super_stt_shared::models::protocol::PreviewSource;

use super::fake_daemon::{FakeDaemon, Reply};
use crate::daemon::client::v1::transcribe::{self, RecordEvent};

/// An SSE body, as the daemon writes it: blocks of `event:`/`data:` lines
/// separated by a blank line.
fn sse(blocks: &[(&str, serde_json::Value)]) -> String {
    blocks
        .iter()
        .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n"))
        .collect()
}

async fn drain() -> Vec<RecordEvent> {
    Box::pin(transcribe::record_command_stream())
        .collect::<Vec<_>>()
        .await
}

fn preview_of(event: &RecordEvent) -> (&str, Option<PreviewSource>) {
    match event {
        RecordEvent::Preview { text, source } => (text.as_str(), *source),
        RecordEvent::Final(_) => panic!("expected a preview"),
    }
}

fn final_of(event: &RecordEvent) -> Result<&str, &str> {
    match event {
        RecordEvent::Final(result) => result.as_deref().map_err(String::as_str),
        RecordEvent::Preview { .. } => panic!("expected the final event"),
    }
}

/// The panel renders live text, so it asks for incremental frames and holds the
/// connection open for the result. `manual_only` because the panel's own Stop
/// button ends the recording — silence detection cutting it short would make
/// the test say nothing about the microphone.
#[tokio::test]
async fn the_test_panel_asks_for_a_held_open_realtime_stream() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::raw(
        200,
        "text/event-stream",
        sse(&[("done", json!({ "transcription": "hello" }))]),
    ));

    drain().await;

    let request = daemon.request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/transcribe");
    assert_eq!(
        request.json(),
        json!({
            "write_mode": false,
            "wait": true,
            "stream_realtime": true,
            "stop_mode": "manual_only",
        })
    );
}

/// Previews arrive as they are produced and the final transcript closes the
/// stream. `source` is what tells the panel whether a frame extends the last
/// one or replaces it.
#[tokio::test]
async fn previews_stream_before_the_final_transcript() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::raw(
        200,
        "text/event-stream",
        sse(&[
            ("preview", json!({ "text": "hello", "source": "stream" })),
            (
                "preview",
                json!({ "text": "hello wor", "source": "window" }),
            ),
            ("done", json!({ "transcription": "hello world" })),
        ]),
    ));

    let events = drain().await;

    assert_eq!(events.len(), 3);
    assert_eq!(
        preview_of(&events[0]),
        ("hello", Some(PreviewSource::Stream))
    );
    assert_eq!(
        preview_of(&events[1]),
        ("hello wor", Some(PreviewSource::Window))
    );
    assert_eq!(final_of(&events[2]), Ok("hello world"));
}

/// An empty transcript is a successful recording of nothing, not a failure —
/// the panel says so in words rather than leaving the reader with a blank box.
#[tokio::test]
async fn an_empty_transcript_is_reported_as_no_speech() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::raw(
        200,
        "text/event-stream",
        sse(&[("done", json!({ "transcription": "   " }))]),
    ));

    let events = drain().await;

    assert_eq!(final_of(&events[0]), Ok("No speech detected"));
}

#[tokio::test]
async fn a_daemon_side_error_ends_the_stream_as_a_failure() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::raw(
        200,
        "text/event-stream",
        sse(&[("error", json!({ "message": "no model loaded" }))]),
    ));

    let events = drain().await;

    assert_eq!(final_of(&events[0]), Err("no model loaded"));
}

/// A refusal before the stream opens is a JSON envelope, not SSE. Reading it
/// as an event stream would report "ended unexpectedly" and bury the reason the
/// daemon actually gave.
#[tokio::test]
async fn a_refused_recording_reports_the_daemons_reason() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::status(
        409,
        &json!({ "status": "error", "error_code": "recording_in_progress" }),
    ));

    let events = drain().await;

    assert_eq!(
        final_of(&events[0]),
        Err("recording_in_progress (HTTP 409)")
    );
}

/// A stream that ends with neither `done` nor `error` leaves the panel waiting
/// forever unless the absence is itself reported.
#[tokio::test]
async fn a_stream_that_just_stops_is_reported_rather_than_hung_on() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::raw(
        200,
        "text/event-stream",
        sse(&[("preview", json!({ "text": "hello" }))]),
    ));

    let events = drain().await;

    assert_eq!(preview_of(&events[0]), ("hello", None));
    assert_eq!(
        final_of(&events[1]),
        Err("transcribe stream ended unexpectedly")
    );
}

#[tokio::test]
async fn stopping_a_recording_posts_to_the_stop_path() {
    let daemon = FakeDaemon::start().await;

    transcribe::stop_record_command()
        .await
        .expect("recording stopped");

    let request = daemon.request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/transcribe/stop");
    assert_eq!(request.json(), json!({}));
}

#[tokio::test]
async fn a_stop_the_daemon_refuses_surfaces_its_message() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "error",
        "message": "no recording in progress",
    })));

    let error = transcribe::stop_record_command()
        .await
        .expect_err("stop refused");

    assert_eq!(error.to_string(), "no recording in progress");
}
