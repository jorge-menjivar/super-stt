// SPDX-License-Identifier: GPL-3.0-only
//! `voices`-scope HTTP smoke test: the cloned-voice library end to end.
//!
//! Covers the round trip a client actually performs — upload a recording, see
//! it listed, play it back, rename it, delete it — plus the two refusals that
//! decide whether the endpoint is safe to expose: input that is not audio, and
//! a token that was granted a different scope.
//!
//! Hermetic like the other smoke tests: `SUPER_TTS_AUTO_APPROVE=1` for consent
//! and an isolated `XDG_DATA_HOME`, so the library under test is a temporary
//! directory and never the developer's own voices.

mod common;

use common::{Method, StatusCode, TestDaemon};
use hyper::Request;

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1::handshake;
use std::path::PathBuf;
use tokio::net::UnixStream;

async fn start_daemon(scopes: &[&str]) -> (TestDaemon, PathBuf, String) {
    let daemon = common::daemon("voices");
    let daemon = daemon.start().await;
    let token = daemon.token("voices-smoke", scopes).await;
    let socket = daemon.socket().to_path_buf();
    (daemon, socket, token)
}

/// One request with an arbitrary body, returning `(status, content-type, bytes)`.
async fn raw(
    socket_path: &PathBuf,
    method: Method,
    path: &str,
    token: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, String, Vec<u8>) {
    let stream = UnixStream::connect(socket_path).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Full<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let mut builder = Request::builder()
        .method(method)
        .uri(format!("http://tts.local/v1{path}"))
        .header("host", "tts.local")
        .header("authorization", format!("Bearer {token}"));
    if let Some(ct) = content_type {
        builder = builder
            .header("content-type", ct)
            .header("content-length", body.len().to_string());
    }
    let req = builder
        .body(Full::new(Bytes::from(body)))
        .expect("build req");

    let resp = sender.send_request(req).await.expect("send req");
    let status = resp.status();
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    (status, ct, bytes.to_vec())
}

/// A JSON request, returning `(status, parsed body)`.
async fn json(
    p: &PathBuf,
    method: Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let (ct, bytes) = match body {
        Some(v) => (
            Some("application/json"),
            serde_json::to_vec(&v).expect("encode"),
        ),
        None => (None, Vec::new()),
    };
    let (status, _, out) = raw(p, method, path, token, ct, bytes).await;
    (
        status,
        serde_json::from_slice(&out).unwrap_or(serde_json::Value::Null),
    )
}

/// Upload a WAV as the raw body, the way a client adds a recording.
async fn upload(
    p: &PathBuf,
    path: &str,
    token: &str,
    wav: Vec<u8>,
) -> (StatusCode, serde_json::Value) {
    let (status, _, out) = raw(p, Method::POST, path, token, Some("audio/wav"), wav).await;
    (
        status,
        serde_json::from_slice(&out).unwrap_or(serde_json::Value::Null),
    )
}

/// A mono 16-bit WAV of `seconds` at 44.1 kHz — deliberately not the rate the
/// library stores, so the round trip exercises the resample.
fn wav(seconds: f32) -> Vec<u8> {
    let rate = 44_100_u32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frames = (seconds * rate as f32) as usize;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cursor, spec).expect("writer");
        for i in 0..frames {
            #[allow(clippy::cast_possible_truncation)]
            let sample = ((i as f32 / 40.0).sin() * 6000.0) as i16;
            w.write_sample(sample).expect("write");
        }
        w.finalize().expect("finalize");
    }
    cursor.into_inner()
}

#[tokio::test]
async fn a_voice_is_uploaded_listed_played_back_renamed_and_deleted() {
    let (_guard, sock, token) = start_daemon(&["voices"]).await;

    // Nothing yet: a daemon that has never cloned a voice lists an empty
    // library rather than failing on a directory that does not exist.
    let (s, body) = json(&sock, Method::GET, "/voice/list", &token, None).await;
    assert_eq!(s, StatusCode::OK, "empty list: {body}");
    assert_eq!(body["voices"].as_array().map(Vec::len), Some(0), "{body}");
    assert!(
        body["model"].is_null(),
        "with nothing loaded there is no cloning capability to report: {body}"
    );

    // Upload.
    let (s, body) = upload(
        &sock,
        "/voice?label=Ada&transcript=the%20quick%20brown%20fox",
        &token,
        wav(1.5),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "upload: {body}");
    let id = body["voice"]["id"].as_str().expect("id").to_string();
    let voice_id = body["voice"]["voice_id"].as_str().expect("voice_id");
    assert_eq!(voice_id, format!("voice:{id}"), "{body}");
    assert_eq!(body["voice"]["label"], "Ada", "{body}");
    assert_eq!(
        body["voice"]["transcript"], "the quick brown fox",
        "the transcript survives the query string: {body}"
    );
    assert_eq!(
        body["voice"]["sample_rate"], 24_000,
        "stored at the canonical rate, not the 44.1 kHz uploaded: {body}"
    );

    // Listed, and readable on its own.
    let (s, body) = json(&sock, Method::GET, "/voice/list", &token, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["voices"].as_array().map(Vec::len), Some(1), "{body}");
    let (s, body) = json(&sock, Method::GET, &format!("/voice/{id}"), &token, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["voice"]["id"], id.as_str(), "{body}");

    // Playable back: the clip comes out as audio, not as JSON about audio.
    let (s, ct, bytes) = raw(
        &sock,
        Method::GET,
        &format!("/voice/{id}/audio"),
        &token,
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(ct, "audio/wav", "the clip is served as audio");
    assert_eq!(&bytes[..4], b"RIFF", "a real WAV comes back");

    // Rename.
    let (s, body) = json(
        &sock,
        Method::PATCH,
        &format!("/voice/{id}"),
        &token,
        Some(serde_json::json!({ "label": "Ada Lovelace" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["voice"]["label"], "Ada Lovelace", "{body}");
    assert_eq!(
        body["voice"]["transcript"], "the quick brown fox",
        "a rename leaves the transcript describing the same clip: {body}"
    );

    // Delete, and stay deleted.
    let (s, body) = json(&sock, Method::DELETE, &format!("/voice/{id}"), &token, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (s, _) = json(&sock, Method::GET, &format!("/voice/{id}"), &token, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "a deleted voice is gone");
    let (s, body) = json(&sock, Method::GET, "/voice/list", &token, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["voices"].as_array().map(Vec::len), Some(0), "{body}");
}

#[tokio::test]
async fn refuses_uploads_it_cannot_use() {
    let (_guard, sock, token) = start_daemon(&["voices"]).await;

    let (s, body) = upload(&sock, "/voice?label=Junk", &token, b"not audio".to_vec()).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error_code"], "unsupported_audio", "{body}");

    let (s, body) = upload(&sock, "/voice", &token, wav(0.5)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "a voice needs a name: {body}");
    assert_eq!(body["error_code"], "invalid_value", "{body}");

    let (s, body) = upload(&sock, "/voice?label=Empty", &token, Vec::new()).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error_code"], "invalid_value", "{body}");

    let (s, _) = json(&sock, Method::GET, "/voice/not-a-uuid", &token, None).await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "a malformed id is a miss, not a path"
    );

    let (s, body) = json(&sock, Method::GET, "/voice/list", &token, None).await;
    assert_eq!(body["voices"].as_array().map(Vec::len), Some(0), "{body}");
    assert_eq!(s, StatusCode::OK);
}

/// Recordings of a person are gated apart from the settings surface: a token
/// that may change every daemon setting still cannot read the library.
#[tokio::test]
async fn a_settings_token_cannot_reach_the_voice_library() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    for (method, path) in [
        (Method::GET, "/voice/list"),
        (Method::GET, "/voice/2f8a2d0e-0000-4000-8000-000000000000"),
        (
            Method::DELETE,
            "/voice/2f8a2d0e-0000-4000-8000-000000000000",
        ),
    ] {
        let (s, body) = json(&sock, method.clone(), path, &token, None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{method} {path}: {body}");
        assert_eq!(body["message"], "scope_denied", "{method} {path}: {body}");
    }

    let (s, body) = upload(&sock, "/voice?label=Sneaky", &token, wav(0.5)).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
}

/// A refused upload must still be *answered*, not reset.
///
/// The scope check refuses on the headers alone, before anything reads the
/// body, so the daemon has to drain what the client is still sending. Without
/// that, hyper closes a connection whose request it never finished reading and
/// the client gets `BrokenPipe` on the write instead of the `403` — which is
/// what a browser reports as a bare "failed to fetch" rather than
/// `scope_denied`.
///
/// A body comfortably past the socket buffer is what makes the race worth
/// testing: the client cannot hand it all to the kernel in one write, so the
/// answer arrives only if the daemon kept reading. Measured against a daemon
/// without the drain, this fails ~48% of the time under load where the 0.5s
/// clip above fails ~12%; on an idle machine neither one fails, so treat this
/// as a canary rather than a proof.
#[tokio::test]
async fn a_refused_upload_is_answered_rather_than_reset() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    // ~1 MiB: well past the ~208 KiB socket buffer, well under the daemon's
    // drain limit.
    let (s, body) = upload(&sock, "/voice?label=Oversized", &token, wav(12.0)).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["message"], "scope_denied", "{body}");
}
