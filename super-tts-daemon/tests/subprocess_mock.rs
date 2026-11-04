// SPDX-License-Identifier: GPL-3.0-only
//! Drives the daemon's real `SubprocessBackend` orchestration (manifest parse,
//! socket, systemd-run spawn, ping/load/status/synthesize, teardown) against the
//! `mock_backend` fixture — no GPU, model, or network. Needs a systemd `--user`
//! session, so it is gated behind `SUPER_TTS_TEST_SUBPROCESS=1` and skipped on
//! hosted CI runners.
//!
//! Run: `SUPER_TTS_TEST_SUBPROCESS=1` cargo test -p super-tts-daemon \
//!        --features test-fixtures --test `subprocess_mock` -- --nocapture
#![cfg(all(feature = "subprocess-backends", feature = "test-fixtures"))]

use super_tts_daemon::registry::host_detect;
use super_tts_daemon::tts_models::subprocess::SubprocessBackend;
use super_tts_daemon::tts_models::synthesize::Synthesize;
use super_tts_daemon::tts_models::v1::{CollectingSink, SynthesizeRequest};
use super_tts_shared::audio::frames::{FrameKind, SampleFormat};

/// Removes the per-test backend dir on scope exit — including panic unwinds, so a
/// failed assertion doesn't leak `~/.cache/super-tts-mock-test-<pid>`.
///
/// Also the cache dir the daemon grants that backend, which is named after the
/// backend dir and lives under the real `~/.cache/super-tts/backends`, so each
/// run would otherwise leave one behind per test.
struct CleanupDir(std::path::PathBuf);
impl Drop for CleanupDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
        if let Some(name) = self.0.file_name() {
            let cache = super_tts_shared::paths::cache_dir()
                .join("backends")
                .join(name);
            let _ = std::fs::remove_dir_all(cache);
        }
    }
}

const MOCK_TOML: &str = r#"
[backend]
source = "github.com/jorge-menjivar/super-tts-piper"
name = "Mock"
version = "0.0.0"
kind = "subprocess"
entrypoint = "mock-backend"
product = "super-tts"
contract = "v1"
description = "Test backend."

[[models]]
name = "mock"
multilingual = false
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]

# A second model so two tests in one process do not collide: the transient unit
# is named for the model and the pid, and the daemon itself never runs two
# subprocess backends at once, so the name is only ambiguous under a test.
[[models]]
name = "mock-options"
multilingual = false
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]

# A third, for the same reason.
[[models]]
name = "mock-reconfigure"
multilingual = false
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]
"#;

/// A backend directory holding the manifest and the mock binary, plus the guard
/// that removes it.
///
/// Built outside `/tmp`: `PrivateTmp=yes` in the systemd sandbox makes `/tmp`
/// private, so a `ReadOnlyPaths=/tmp/...` bind mount fails at namespace setup.
/// `label` keeps two tests in one process from sharing a directory, and with it
/// a unit name.
fn seed_backend_dir(label: &str) -> (std::path::PathBuf, CleanupDir) {
    seed_backend_dir_with(label, MOCK_TOML)
}

/// [`seed_backend_dir`] with `manifest` as the `backend.toml`.
fn seed_backend_dir_with(label: &str, manifest: &str) -> (std::path::PathBuf, CleanupDir) {
    let dir = dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(format!(
            "super-tts-mock-test-{label}-{}",
            std::process::id()
        ));
    std::fs::create_dir_all(&dir).unwrap();
    let cleanup = CleanupDir(dir.clone());
    std::fs::write(dir.join("backend.toml"), manifest).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_mock_backend"), dir.join("mock-backend")).unwrap();
    (dir, cleanup)
}

#[tokio::test]
async fn subprocess_orchestration_against_mock() {
    if std::env::var("SUPER_TTS_TEST_SUBPROCESS").is_err() {
        return; // needs a systemd --user session
    }
    // `main` installs the provider for the real daemon; an integration test is
    // its own process and has to do the same, or the first reqwest client built
    // during spawn panics with "No provider set". Without this the test fails
    // before it reaches any assertion — which is why the gap survived: it only
    // runs with SUPER_TTS_TEST_SUBPROCESS=1, never on CI.
    super_tts_daemon::install_crypto_provider();

    let (dir, _cleanup) = seed_backend_dir("orchestration");

    // Spawn + load via the real daemon orchestration.
    // The real probe: the mock declares no `[[models.files]]` at all, so
    // selection has nothing to resolve, but `spawn` takes the host the daemon
    // hands it and the test should exercise the same call.
    let host = host_detect::detect();
    let mut backend = SubprocessBackend::spawn(&dir, "mock", "cpu", &host, None, Vec::new())
        .await
        .expect("spawn + load mock backend");

    // Synthesize drives /v1/synthesize → a framed s16le ramp read incrementally
    // off the socket. The ramp is the point: silence would survive a decoder
    // that dropped a frame or lost the bytes straddling a frame boundary.
    let mut sink = CollectingSink::default();
    backend
        .synthesize(
            &SynthesizeRequest {
                text: "hello",
                ..Default::default()
            },
            &mut sink,
        )
        .await
        .expect("synthesize");

    let params = sink.params.expect("params arrive before any frame");
    assert_eq!(params.sample_rate, 24000);
    assert_eq!(params.channels, 1);
    assert_eq!(params.format, SampleFormat::S16Le);
    assert_eq!(
        sink.frames
            .iter()
            .filter(|f| f.kind == FrameKind::Audio)
            .count(),
        4,
        "the mock splits the ramp across 4 frames"
    );

    let samples = sink.samples();
    assert_eq!(samples.len(), 960);
    for (i, got) in samples.iter().enumerate() {
        let want = f32::from(i16::try_from(i).unwrap()) / 32768.0;
        assert!(
            (got - want).abs() < 1e-6,
            "sample {i}: got {got}, want {want}"
        );
    }
    assert_eq!(
        sink.frames.last().expect("frames were received").kind,
        FrameKind::Done,
        "the stream must end with a terminal frame"
    );

    backend.shutdown().await.expect("clean shutdown");
}

/// Changing an option reaches a *running* backend, without reloading it.
///
/// The end-to-end version of the claim the daemon's option write rests on: the
/// headers are read per request, so swapping them is enough. Same spawned unit
/// throughout — nothing is torn down, no model is provisioned again — and the
/// second synthesis carries the new value where the first carried the old one.
#[tokio::test]
async fn a_changed_option_reaches_a_running_subprocess() {
    /// The `options` string the mock echoes back in its mark frame.
    async fn echoed_options(backend: &SubprocessBackend) -> String {
        let mut sink = CollectingSink::default();
        backend
            .synthesize(
                &SynthesizeRequest {
                    text: "hello",
                    ..Default::default()
                },
                &mut sink,
            )
            .await
            .expect("synthesize");
        let mark = sink
            .frames
            .iter()
            .find(|f| f.kind == FrameKind::Mark)
            .expect("the mock emits one mark frame");
        String::from_utf8_lossy(&mark.payload).into_owned()
    }

    if std::env::var("SUPER_TTS_TEST_SUBPROCESS").is_err() {
        return; // needs a systemd --user session
    }
    super_tts_daemon::install_crypto_provider();

    let (dir, _cleanup) = seed_backend_dir("reconfigure");

    let option = "x-tts-option-voice_design_preset".to_string();
    let mut backend = SubprocessBackend::spawn(
        &dir,
        "mock-reconfigure",
        "cpu",
        &host_detect::detect(),
        None,
        vec![(option.clone(), "Deep narrator (male)".to_string())],
    )
    .await
    .expect("spawn + load mock backend");

    let before = echoed_options(&backend).await;
    assert!(
        before.contains("voice_design_preset=Deep narrator (male)"),
        "the spawn value must arrive first: {before}"
    );

    backend.reconfigure(super_tts_daemon::tts_models::synthesize::BackendContext {
        headers: vec![(option, "Bright presenter (female)".to_string())],
        user_allowed_hosts: Vec::new(),
    });

    let after = echoed_options(&backend).await;
    assert!(
        after.contains("voice_design_preset=Bright presenter (female)"),
        "the new value must arrive on the next request, with no reload in between: {after}"
    );
    assert!(
        !after.contains("Deep narrator"),
        "the old value must be gone, not appended: {after}"
    );

    backend.shutdown().await.expect("clean shutdown");
}

/// The contract says every `/v1` request carries the user's `[[options]]` as
/// `x-tts-option-*` headers, whichever transport. A backend that steers on one
/// — the Qwen backend designs its voice from a pair of them — reads them off
/// the request, so the daemon has to put them there.
///
/// Asserted on `/v1/synthesize` specifically: it is the one route that builds
/// its own request rather than going through `request`, because it streams its
/// response, and it is the route the options are for.
#[tokio::test]
async fn option_headers_reach_the_subprocess() {
    if std::env::var("SUPER_TTS_TEST_SUBPROCESS").is_err() {
        return; // needs a systemd --user session
    }
    super_tts_daemon::install_crypto_provider();

    let (dir, _cleanup) = seed_backend_dir("headers");

    let headers = vec![
        (
            "x-tts-option-voice_design_preset".to_string(),
            "Deep narrator (male)".to_string(),
        ),
        (
            "x-tts-option-voice_design_description".to_string(),
            "A hoarse pirate".to_string(),
        ),
    ];
    let host = host_detect::detect();
    let mut backend = SubprocessBackend::spawn(&dir, "mock-options", "cpu", &host, None, headers)
        .await
        .expect("spawn + load mock backend");

    let mut sink = CollectingSink::default();
    backend
        .synthesize(
            &SynthesizeRequest {
                text: "hello",
                ..Default::default()
            },
            &mut sink,
        )
        .await
        .expect("synthesize");

    let mark = sink
        .frames
        .iter()
        .find(|f| f.kind == FrameKind::Mark)
        .expect("the mock emits one mark frame");
    let echoed = String::from_utf8_lossy(&mark.payload);
    assert!(
        echoed.contains(
            r#""options":"voice_design_description=A hoarse pirate voice_design_preset=Deep narrator (male)""#
        ),
        "the option headers must arrive on the streaming request: {echoed}"
    );

    backend.shutdown().await.expect("clean shutdown");
}

/// A manifest whose one model, `model`, declares a configuration file as its
/// `prepare_files` and has its weights downloaded after it, both served from
/// `base`.
fn preparing_manifest(model: &str, base: &str) -> String {
    format!(
        r#"
[backend]
source = "github.com/jorge-menjivar/super-tts-piper"
name = "Mock"
version = "0.0.0"
kind = "subprocess"
entrypoint = "mock-backend"
product = "super-tts"
contract = "v1"
description = "Test backend."

[[models]]
name = "{model}"
multilingual = false
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]
prepare_files = ["models/mock/config.json"]
files = [
    {{ url = "{base}/weights.bin", destination = "models/mock/weights.bin" }},
    {{ url = "{base}/config.json", destination = "models/mock/config.json" }},
]
"#
    )
}

/// How long the file server holds back the weights. Long enough that a
/// backend told to exit while preparing is gone before the load reaches it.
const WEIGHTS_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

/// Serve every request a few bytes over plain HTTP on a local port, and
/// return the base URL. A request for the weights is answered after
/// [`WEIGHTS_DELAY`], the way a large file arrives after a small one.
async fn serve_model_files() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                if String::from_utf8_lossy(&request).contains("weights.bin") {
                    tokio::time::sleep(WEIGHTS_DELAY).await;
                }
                let body = b"mock model bytes";
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(body).await;
            });
        }
    });
    base
}

/// The files the mock's prepare saw, one per line, or `None` when no prepare
/// finished. See the mock's `PREPARED_MARKER`.
fn prepared_with(dir: &std::path::Path) -> Option<String> {
    let cache = super_tts_shared::paths::cache_dir()
        .join("backends")
        .join(dir.file_name().unwrap());
    std::fs::read_to_string(cache.join("prepared")).ok()
}

/// Synthesize once and return how many samples came back: the proof that the
/// backend loaded and serves.
async fn synthesized_samples(backend: &SubprocessBackend) -> usize {
    let mut sink = CollectingSink::default();
    backend
        .synthesize(
            &SynthesizeRequest {
                text: "hello",
                ..Default::default()
            },
            &mut sink,
        )
        .await
        .expect("synthesize");
    sink.samples().len()
}

/// A model with `prepare_files` is prepared once those files are on disk and
/// before the rest are, then loaded once everything is.
///
/// The manifest lists the weights first, so this also pins that the prepare's
/// files are pulled forward rather than taken in manifest order.
#[tokio::test]
async fn a_prepare_runs_before_the_weights_download() {
    if std::env::var("SUPER_TTS_TEST_SUBPROCESS").is_err() {
        return; // needs a systemd --user session
    }
    super_tts_daemon::install_crypto_provider();

    let base = serve_model_files().await;
    let (dir, _cleanup) =
        seed_backend_dir_with("prepares", &preparing_manifest("mock-prepares", &base));

    let mut backend = SubprocessBackend::spawn(
        &dir,
        "mock-prepares",
        "cpu",
        &host_detect::detect(),
        None,
        Vec::new(),
    )
    .await
    .expect("prepare, download and load");

    let seen = prepared_with(&dir).expect("the backend was sent a prepare");
    assert!(
        seen.lines().any(|f| f == "models/mock/config.json"),
        "the prepare's file was on disk for it: {seen}"
    );
    assert!(
        !seen.lines().any(|f| f.contains("weights")),
        "the weights were not downloaded yet: {seen}"
    );
    assert!(dir.join("models/mock/weights.bin").exists());

    assert_eq!(synthesized_samples(&backend).await, 960);
    backend.shutdown().await.expect("clean shutdown");
}

/// A prepare that fails is the backend's to log, and the load goes ahead as if
/// it had never been sent.
#[tokio::test]
async fn a_failed_prepare_leaves_the_load_unharmed() {
    if std::env::var("SUPER_TTS_TEST_SUBPROCESS").is_err() {
        return; // needs a systemd --user session
    }
    super_tts_daemon::install_crypto_provider();

    let base = serve_model_files().await;
    let (dir, _cleanup) = seed_backend_dir_with(
        "prepare-fails",
        &preparing_manifest("mock-prepare-fails", &base),
    );

    let mut backend = SubprocessBackend::spawn(
        &dir,
        "mock-prepare-fails",
        "cpu",
        &host_detect::detect(),
        None,
        Vec::new(),
    )
    .await
    .expect("a failed prepare must not fail the load");

    assert_eq!(prepared_with(&dir), None, "the prepare did fail");
    assert_eq!(synthesized_samples(&backend).await, 960);
    backend.shutdown().await.expect("clean shutdown");
}

/// A backend that exits while preparing, as a driver crash would end it, is
/// started again for the load. Its unit is left `failed` by the exit, so this
/// is also the test of the restart clearing that.
#[tokio::test]
async fn a_backend_that_dies_preparing_is_started_again() {
    if std::env::var("SUPER_TTS_TEST_SUBPROCESS").is_err() {
        return; // needs a systemd --user session
    }
    super_tts_daemon::install_crypto_provider();

    let base = serve_model_files().await;
    let (dir, _cleanup) = seed_backend_dir_with(
        "dies-preparing",
        &preparing_manifest("mock-dies-on-prepare", &base),
    );

    let mut backend = SubprocessBackend::spawn(
        &dir,
        "mock-dies-on-prepare",
        "cpu",
        &host_detect::detect(),
        None,
        Vec::new(),
    )
    .await
    .expect("the load must start the backend again");

    assert_eq!(synthesized_samples(&backend).await, 960);
    backend.shutdown().await.expect("clean shutdown");
}
