// SPDX-License-Identifier: GPL-3.0-only
//! The speak path end to end: text in, samples in the playback ring.
//!
//! Drives the real [`SpeechEngine`] against the real `WasmBackend` host and the
//! mock component fixture, with the playback pipeline detached from cpal. So
//! everything between `speak()` and the audio callback is production code —
//! request build, `POST /v1/synthesize`, frame decode, sample decode, the ring
//! — and no audio device is needed, which is what lets this run in CI.
//!
//! Requires the fixture: `just build-mock-wasm-backend`.
#![cfg(feature = "wasm-backends")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super_tts_daemon::audio::playback::DeviceFormat;
use super_tts_daemon::daemon::speech::{SpeakError, SpeechEngine};
use super_tts_daemon::daemon::types::{LoadedModel, SharedLoadedModel};
use super_tts_daemon::tts_models::ModelDefinition;
use super_tts_daemon::tts_models::wasm::WasmBackend;
use super_tts_registry_types::manifest::{Device, TtsModel};

/// The mock synthesizes 960 samples of s16le at 24 kHz.
const MOCK_SAMPLES: usize = 960;
const MOCK_RATE: u32 = 24000;

/// Device format for the null sink. 48 kHz mono means the mock's 24 kHz output
/// is resampled on the way in — the ordinary case, so the test covers it.
const DEVICE: DeviceFormat = DeviceFormat {
    sample_rate: 48000,
    channels: 1,
};

fn mock_component() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "tests/fixtures/mock-wasm-backend/target/wasm32-wasip2/release/mock_wasm_backend.wasm",
    );
    p.exists().then_some(p)
}

fn definition() -> ModelDefinition {
    ModelDefinition {
        name: "mock".into(),
        source: "github.com/super-tts/mock".into(),
        is_multilingual: false,
        primary_language: "en".into(),
        supported_languages: vec!["en".into()],
        estimated_vram_bytes: 0,
        processing_interval: Duration::from_millis(0),
        supported_devices: vec![Device::None],
        realtime: false,
        provider: None,
        product: TtsModel::default(),
    }
}

/// A shared slot holding the mock backend, or `None` when the fixture is not
/// built (the caller then skips).
fn loaded_model() -> Option<SharedLoadedModel> {
    let path = mock_component()?;
    let backend = WasmBackend::new(&path, Vec::new(), "mock".to_string(), Vec::new())
        .expect("load mock backend");
    Some(Arc::new(tokio::sync::RwLock::new(Some(LoadedModel::new(
        definition(),
        Box::new(backend),
    )))))
}

/// An empty slot, for the not-loaded path.
fn empty_model() -> SharedLoadedModel {
    Arc::new(tokio::sync::RwLock::new(None))
}

/// Pull `n` samples out of the engine's playback ring the way a device would.
async fn drain(engine: &SpeechEngine, n: usize) -> Vec<f32> {
    let playback = engine
        .playback_handle()
        .await
        .expect("the engine opened a pipeline");
    let mut out = Vec::with_capacity(n);
    let mut buf = vec![0.0_f32; 256];
    while out.len() < n {
        playback.render(&mut buf);
        out.extend_from_slice(&buf);
    }
    out.truncate(n);
    out
}

/// Play the engine's ring out the way a device would, then give the daemon's
/// playout watcher a moment to notice.
///
/// The null sink only moves when a test renders it, so any assertion about what
/// happens *after* the audio is heard has to render it first. That the ring must
/// be emptied for the utterance to close is the contract under test, not an
/// artifact of the fake device: a real one is doing exactly this, in realtime,
/// for as long as the audio lasts.
async fn play_out(engine: &SpeechEngine) {
    let playback = engine
        .playback_handle()
        .await
        .expect("the engine opened a pipeline");
    let mut buf = vec![0.0_f32; 256];
    for _ in 0..2000 {
        playback.render(&mut buf);
        if playback.buffered() == 0 {
            break;
        }
    }
    assert_eq!(playback.buffered(), 0, "the ring should have played out");
    // One more block so the watcher sees `Idle`, then let it run: it polls, so
    // the close trails the last sample by a poll interval and a residual block.
    playback.render(&mut buf);
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        if engine.current().is_none() {
            return;
        }
    }
}

#[tokio::test]
async fn speak_synthesizes_and_queues_audio_for_playback() {
    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);

    let utterance = engine
        .speak(&model, "hello there", None, None)
        .await
        .expect("speak should succeed");

    assert!(
        utterance.id.starts_with("utt_"),
        "an utterance id is returned so a client can cancel it, got {}",
        utterance.id
    );
    assert_eq!(utterance.marks, 1, "the mock emits one alignment mark");
    assert_eq!(
        engine.current().as_deref(),
        Some(utterance.id.as_str()),
        "queued is not spoken: the slot stays claimed until the ring plays out"
    );

    // 960 samples at 24 kHz resampled to 48 kHz is ~1920, and the ring should
    // hold about that. Exactness is the resampler's business, not this test's.
    let playback = engine.playback_handle().await.expect("pipeline opened");
    let buffered = playback.buffered();
    assert!(
        (1500..=2400).contains(&buffered),
        "expected ~1920 resampled samples queued, got {buffered}"
    );

    let out = drain(&engine, buffered).await;
    assert!(
        out.iter().any(|s| s.abs() > 0.001),
        "the queued audio must be the mock's ramp, not silence"
    );
    assert!(
        out.iter().all(|s| s.is_finite() && s.abs() <= 1.0),
        "every sample stays finite and in range"
    );

    // And once the device has actually played it, the utterance closes.
    play_out(&engine).await;
    assert_eq!(
        engine.current(),
        None,
        "the slot is released once the audio has been heard"
    );
}

/// The mock's ramp is monotonically rising, so the queued audio must rise too.
/// A decoder that dropped a frame, misordered chunks, or lost the bytes
/// straddling a frame boundary would break monotonicity even though the audio
/// would still "sound like something".
#[tokio::test]
async fn the_queued_audio_preserves_the_backends_ramp() {
    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DeviceFormat {
        // Match the mock's rate so no resampling smooths over an ordering bug.
        sample_rate: MOCK_RATE,
        channels: 1,
    });

    engine
        .speak(&model, "ramp", None, None)
        .await
        .expect("speak");

    let playback = engine.playback_handle().await.expect("pipeline opened");
    assert_eq!(
        playback.buffered(),
        MOCK_SAMPLES,
        "at a matching rate every sample arrives exactly once"
    );
    let out = drain(&engine, MOCK_SAMPLES).await;

    // The seam fade attenuates both ends, so check the middle, which is
    // untouched and must be strictly rising.
    let mid = &out[200..MOCK_SAMPLES - 200];
    for w in mid.windows(2) {
        assert!(
            w[1] > w[0],
            "the ramp must stay monotonic through the pipeline: {} then {}",
            w[0],
            w[1]
        );
    }
}

#[tokio::test]
async fn speaking_without_a_model_is_a_coded_refusal() {
    let engine = SpeechEngine::detached(DEVICE);
    let err = engine
        .speak(&empty_model(), "hello", None, None)
        .await
        .expect_err("no model is loaded");
    assert!(matches!(err, SpeakError::NotLoaded));
    assert_eq!(
        engine.current(),
        None,
        "a refused request claims no utterance slot"
    );
}

#[tokio::test]
async fn empty_and_oversized_text_are_refused_before_the_backend_is_touched() {
    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);

    for blank in ["", "   ", "\n\t "] {
        let err = engine
            .speak(&model, blank, None, None)
            .await
            .expect_err("blank text is refused");
        assert!(matches!(err, SpeakError::EmptyText), "for {blank:?}");
    }

    let huge = "a".repeat(super_tts_daemon::daemon::speech::MAX_TEXT_CHARS + 1);
    let err = engine
        .speak(&model, &huge, None, None)
        .await
        .expect_err("oversized text is refused");
    assert!(matches!(err, SpeakError::TextTooLong));

    assert!(
        engine.playback_handle().await.is_none(),
        "a refused request must not even open the audio device"
    );
}

#[tokio::test]
async fn cancel_clears_the_queued_audio() {
    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);
    engine
        .speak(&model, "hello", None, None)
        .await
        .expect("speak");
    let playback = engine.playback_handle().await.expect("pipeline opened");
    assert!(playback.buffered() > 0);

    engine.cancel().await;
    assert_eq!(
        playback.buffered(),
        0,
        "cancel drops queued audio rather than playing it out"
    );

    let mut buf = vec![0.0_f32; 512];
    playback.render(&mut buf);
    assert!(
        buf.iter().all(|s| *s == 0.0),
        "nothing queued before the cancel is still heard"
    );
}

/// Cancelling when nothing is speaking is a no-op, not an error: a client
/// bound to a keypress should not have to know whether it won the race.
#[tokio::test]
async fn cancelling_nothing_is_harmless() {
    let engine = SpeechEngine::detached(DEVICE);
    assert_eq!(engine.cancel().await, None);
}

/// The v1 policy is newest-wins. The second utterance must not be mixed into
/// whatever the first left in the ring.
#[tokio::test]
async fn a_second_utterance_supersedes_the_first() {
    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DeviceFormat {
        sample_rate: MOCK_RATE,
        channels: 1,
    });

    let first = engine
        .speak(&model, "one", None, None)
        .await
        .expect("first speak");
    let second = engine
        .speak(&model, "two", None, None)
        .await
        .expect("second speak");

    assert_ne!(first.id, second.id, "each utterance gets its own id");
    let playback = engine.playback_handle().await.expect("pipeline opened");
    assert_eq!(
        playback.buffered(),
        MOCK_SAMPLES,
        "the ring holds exactly the second utterance, not both concatenated"
    );
}

/// A model with a small `max_input_chars`, so the chunker has to split.
fn loaded_model_with_limit(limit: u32) -> Option<SharedLoadedModel> {
    let path = mock_component()?;
    let backend = WasmBackend::new(&path, Vec::new(), "mock".to_string(), Vec::new())
        .expect("load mock backend");
    let mut def = definition();
    def.product.max_input_chars = Some(limit);
    Some(Arc::new(tokio::sync::RwLock::new(Some(LoadedModel::new(
        def,
        Box::new(backend),
    )))))
}

/// Text past the model's limit becomes several synthesis requests, and the
/// audio from all of them reaches the ring.
#[tokio::test]
async fn long_text_is_split_into_several_synthesis_chunks() {
    let Some(model) = loaded_model_with_limit(40) else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DeviceFormat {
        sample_rate: MOCK_RATE,
        channels: 1,
    });

    let text = "First sentence here. Second sentence follows. Third one arrives now. \
                And a fourth to finish.";
    let utterance = engine.speak(&model, text, None, None).await.expect("speak");

    assert!(
        utterance.chunks > 1,
        "text over the model's limit must be split, got {} chunk(s)",
        utterance.chunks
    );
    let playback = engine.playback_handle().await.expect("pipeline opened");
    assert_eq!(
        playback.buffered(),
        MOCK_SAMPLES * utterance.chunks,
        "every chunk's audio reaches the ring"
    );
    assert_eq!(
        utterance.marks, utterance.chunks,
        "one mark per synthesis response"
    );
}

/// Text within the limit stays one request: each extra split costs a round trip
/// and resets the model's prosodic context.
#[tokio::test]
async fn short_text_stays_a_single_synthesis_chunk() {
    let Some(model) = loaded_model_with_limit(4000) else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);
    let utterance = engine
        .speak(&model, "Hello there. How are you?", None, None)
        .await
        .expect("speak");
    assert_eq!(utterance.chunks, 1);
}

/// Markup is stripped before the backend sees it — otherwise the model reads
/// the asterisks out loud.
#[tokio::test]
async fn markup_is_normalized_away_before_synthesis() {
    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);
    let utterance = engine
        .speak(
            &model,
            "# Title\nThis is **bold** with `code` and a [link](https://x.com/y).",
            None,
            None,
        )
        .await
        .expect("speak");
    // The heading ends with its line, so it is an utterance of its own.
    assert_eq!(utterance.chunks, 2);
    assert!(engine.playback_handle().await.is_some());
}

/// Input that is entirely markup normalizes to nothing. Refusing is right —
/// there is no speech to produce — and it must not reach the backend or open
/// the audio device.
#[tokio::test]
async fn text_that_is_entirely_markup_is_refused_as_empty() {
    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);
    let err = engine
        .speak(&model, "```\nfn main() {}\n```", None, None)
        .await;
    // A code fence normalizes to the spoken notice, so this one *does* speak.
    assert!(err.is_ok(), "a code block is announced, not dropped");

    let engine2 = SpeechEngine::detached(DEVICE);
    let err = engine2
        .speak(&model, "**** ___ ~~~", None, None)
        .await
        .expect_err("pure punctuation has nothing to say");
    assert!(matches!(err, SpeakError::EmptyText));
    assert!(
        engine2.playback_handle().await.is_none(),
        "nothing to say must not open the audio device"
    );
}

// -------------------------------------------------------------- streaming

use super_tts_daemon::daemon::speech::SpeakOptions;

/// Yield until `cond` holds, or give up.
///
/// Queuing is deliberately not awaited by `push`: the pump task pushes to the
/// ring concurrently with synthesis, which is what lets audio start before the
/// text is finished. So mid-stream assertions are about what happens *soon*,
/// not what has already happened by the time `push` returns.
async fn eventually(mut cond: impl FnMut() -> bool) -> bool {
    for _ in 0..1000 {
        if cond() {
            return true;
        }
        tokio::task::yield_now().await;
    }
    cond()
}

/// The streaming contract that matters: feeding text as deltas must produce
/// the same audio as handing it over whole. If these diverged, an LLM-driven
/// utterance would sound different from the same text pasted in.
#[tokio::test]
async fn streaming_deltas_produce_the_same_audio_as_one_shot() {
    let Some(model) = loaded_model_with_limit(60) else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let text = "First sentence here. Second sentence follows along. \
                Third one arrives now. And a fourth to finish it.";

    let one_shot = SpeechEngine::detached(DEVICE);
    let whole = one_shot
        .speak(&model, text, None, None)
        .await
        .expect("one-shot speak");
    let whole_buffered = one_shot
        .playback_handle()
        .await
        .expect("pipeline")
        .buffered();

    let streamed = SpeechEngine::detached(DEVICE);
    let mut session = streamed
        .begin(&model, SpeakOptions::default())
        .await
        .expect("begin");
    // Deltas that deliberately cut across sentence boundaries.
    for delta in text.as_bytes().chunks(7) {
        session
            .push(std::str::from_utf8(delta).expect("ascii"))
            .await
            .expect("push");
    }
    let streamed_utterance = session.end().await.expect("end");
    let streamed_buffered = streamed
        .playback_handle()
        .await
        .expect("pipeline")
        .buffered();

    assert_eq!(
        streamed_utterance.chunks, whole.chunks,
        "the same text must split the same way however it arrives"
    );
    assert_eq!(
        streamed_buffered, whole_buffered,
        "and produce the same amount of audio"
    );
}

/// The point of streaming: audio is produced while text is still arriving,
/// not after. If this failed, the WebSocket would be a slower POST.
#[tokio::test]
async fn sentences_are_synthesized_before_the_stream_ends() {
    let Some(model) = loaded_model_with_limit(60) else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);
    let mut session = engine
        .begin(&model, SpeakOptions::default())
        .await
        .expect("begin");

    assert_eq!(
        session.chunks(),
        0,
        "nothing is synthesized before any text"
    );
    session
        .push("First sentence here. Second sentence follows along. And more text after.")
        .await
        .expect("push");
    assert!(
        session.chunks() > 0,
        "a completed sentence must be synthesized before `end` is called"
    );
    let playback = engine.playback_handle().await.expect("pipeline");
    assert!(
        eventually(|| playback.buffered() > 0).await,
        "and its audio must reach the ring without waiting for the stream to end"
    );
    session.end().await.expect("end");
}

/// A partial sentence is held rather than spoken: cutting mid-clause sounds
/// wrong, and the rest is usually one token away.
#[tokio::test]
async fn an_incomplete_sentence_is_not_synthesized_early() {
    let Some(model) = loaded_model_with_limit(4000) else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);
    let mut session = engine
        .begin(&model, SpeakOptions::default())
        .await
        .expect("begin");
    session.push("This sentence has not").await.expect("push");
    assert_eq!(session.chunks(), 0, "a fragment waits for its terminator");
    let utterance = session.end().await.expect("end");
    assert_eq!(utterance.chunks, 1, "and is flushed when the stream ends");
}

#[tokio::test]
async fn cancelling_a_stream_drops_its_queued_audio() {
    let Some(model) = loaded_model_with_limit(60) else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);
    let mut session = engine
        .begin(&model, SpeakOptions::default())
        .await
        .expect("begin");
    // Long enough to complete a chunk: below the model's limit nothing is
    // synthesized until the stream ends, which is the chunker doing its job.
    session
        .push("First sentence here. Second sentence follows along. Third one arrives. More after that.")
        .await
        .expect("push");
    let playback = engine.playback_handle().await.expect("pipeline");
    assert!(eventually(|| playback.buffered() > 0).await);

    session.cancel().await;
    assert_eq!(playback.buffered(), 0, "cancel flushes the ring");
    assert!(session.is_cancelled());
    assert_eq!(engine.current(), None, "and releases the utterance slot");
}

/// An abandoned connection must not leave audio playing, so dropping a session
/// has to cancel it — the WebSocket handler relies on this for the case where
/// the task dies without running its cleanup.
#[tokio::test]
async fn dropping_a_session_cancels_it() {
    let Some(model) = loaded_model_with_limit(60) else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);
    let playback = {
        let mut session = engine
            .begin(&model, SpeakOptions::default())
            .await
            .expect("begin");
        session
            .push("First sentence here. Second sentence follows along. Third one arrives. More after that.")
            .await
            .expect("push");
        let playback = engine.playback_handle().await.expect("pipeline");
        assert!(eventually(|| playback.buffered() > 0).await);
        playback
    };
    assert_eq!(playback.buffered(), 0, "the drop flushed the ring");
    assert_eq!(engine.current(), None, "and released the slot");
}

/// A later utterance supersedes a live stream, and the stream must be able to
/// tell — otherwise a client keeps feeding a session whose audio is discarded.
#[tokio::test]
async fn a_new_utterance_supersedes_a_live_stream() {
    let Some(model) = loaded_model_with_limit(60) else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);
    let mut session = engine
        .begin(&model, SpeakOptions::default())
        .await
        .expect("begin");
    session.push("First sentence here.").await.expect("push");
    assert!(!session.is_cancelled());

    engine
        .speak(&model, "Interrupting.", None, None)
        .await
        .expect("the interrupting utterance");
    assert!(
        session.is_cancelled(),
        "the stream must observe that it was superseded"
    );
}

/// Seconds of speech from the mock: one synthesis chunk per sentence under a
/// 60-character limit, 40 ms each, so about three times what the two-second
/// ring holds.
///
/// A backlog behind the ring is the ordinary case for a reply of any length on
/// a backend faster than realtime, and it is what every stop has to get past:
/// the mock's single chunk fits in the ring whole, so a test built on one
/// cannot see what happens to audio still waiting its turn.
fn backlog() -> String {
    "Here is one more sentence to speak aloud. ".repeat(150)
}

/// `POST /speak/stop` mid-reply. The stop flushed the ring, but the pump still
/// held seconds of synthesized audio and refilled it — so the voice cut out,
/// came back a ring's worth further on, and carried on through the backlog.
#[tokio::test]
async fn a_stop_is_not_undone_by_the_audio_queued_behind_the_ring() {
    let Some(model) = loaded_model_with_limit(60) else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);
    let mut session = engine
        .begin(&model, SpeakOptions::default())
        .await
        .expect("begin");
    session.push(&backlog()).await.expect("push");
    let playback = engine.playback_handle().await.expect("pipeline");
    assert!(eventually(|| playback.buffered() > 0).await);

    engine.cancel().await;
    // Long enough for a pump that missed the stop to refill the ring.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        playback.buffered(),
        0,
        "nothing queued behind the ring is played after a stop"
    );
    let mut buf = vec![0.0_f32; 512];
    playback.render(&mut buf);
    assert!(buf.iter().all(|s| *s == 0.0), "the device stays silent");

    // `POST /speak` is still inside `end` when a user stops it mid-reply, and
    // it must answer once stopped rather than once the backlog is played.
    tokio::time::timeout(Duration::from_secs(1), session.end())
        .await
        .expect("end returns once stopped")
        .expect("end");
}

/// The stream's own `cancel` frame waited for the pump to push its whole
/// backlog through a ring that only empties as fast as the speakers play — a
/// cancel that took as long as the speech it was meant to cut.
#[tokio::test]
async fn cancelling_a_stream_does_not_wait_for_its_backlog() {
    let Some(model) = loaded_model_with_limit(60) else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);
    let mut session = engine
        .begin(&model, SpeakOptions::default())
        .await
        .expect("begin");
    session.push(&backlog()).await.expect("push");
    let playback = engine.playback_handle().await.expect("pipeline");
    assert!(eventually(|| playback.buffered() > 0).await);

    tokio::time::timeout(Duration::from_secs(1), session.cancel())
        .await
        .expect("cancel returns without playing the backlog out");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(playback.buffered(), 0, "and nothing is refilled after it");
    assert_eq!(engine.current(), None, "and it releases the utterance slot");
}

/// A new utterance while the old one's backlog is still queued. The old pump
/// kept writing into the ring the new utterance was writing to, so the two
/// played interleaved.
#[tokio::test]
async fn a_new_utterance_is_not_interleaved_with_the_backlog_it_replaced() {
    let Some(model) = loaded_model_with_limit(60) else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DeviceFormat {
        sample_rate: MOCK_RATE,
        channels: 1,
    });
    let mut session = engine
        .begin(&model, SpeakOptions::default())
        .await
        .expect("begin");
    session.push(&backlog()).await.expect("push");
    let playback = engine.playback_handle().await.expect("pipeline");
    assert!(eventually(|| playback.buffered() > 0).await);

    tokio::time::timeout(
        Duration::from_secs(1),
        engine.speak(&model, "Interrupting.", None, None),
    )
    .await
    .expect("the new utterance is not stuck behind the old one's backlog")
    .expect("speak");
    // Long enough for the superseded pump to write, if it still could.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        playback.buffered(),
        MOCK_SAMPLES,
        "the ring holds exactly the new utterance"
    );
    assert!(session.is_cancelled());
}

/// Marks reach the streaming client, which is what powers highlight-as-it-
/// speaks. The one-shot path counts them; only this path forwards them.
#[tokio::test]
async fn marks_are_forwarded_to_the_stream() {
    use super_tts_daemon::daemon::speech::SpeechEvent;

    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);
    let mut session = engine
        .begin(&model, SpeakOptions::default())
        .await
        .expect("begin");
    session.push("Hello there.").await.expect("push");
    session.end().await.expect("end");

    let mut marks = Vec::new();
    while let Some(SpeechEvent::Mark(m)) = session.try_next_event() {
        marks.push(m);
    }
    assert_eq!(marks.len(), 1, "the mock emits one mark per response");
    assert_eq!(marks[0].start_char, Some(0));
    assert_eq!(marks[0].end_char, Some(5));
}

#[tokio::test]
async fn progress_reports_a_position_within_the_utterance() {
    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let engine = SpeechEngine::detached(DEVICE);
    let mut session = engine
        .begin(&model, SpeakOptions::default())
        .await
        .expect("begin");
    session.push("Hello there.").await.expect("push");
    session.end().await.expect("end");

    let (spoken, queued) = session.progress();
    assert_eq!(spoken, 0, "nothing has been rendered to a device yet");
    assert!(queued > 0, "the utterance is queued, got {queued}ms");

    // Render some, and the spoken position must advance while queued falls.
    let playback = engine.playback_handle().await.expect("pipeline");
    let mut buf = vec![0.0_f32; 4800]; // 100 ms at 48 kHz
    playback.render(&mut buf);
    let (spoken_after, queued_after) = session.progress();
    assert!(
        spoken_after > 0,
        "spoken position must advance as the device consumes audio"
    );
    assert!(
        queued_after < queued,
        "and what remains queued must fall: {queued} then {queued_after}"
    );
}

// ----------------------------------------------------------------- events

use super_tts_daemon::daemon::events::{AnyReceiver, EventBus, Topic};

/// Receive one event as `(topic, parsed JSON)`, or `None` if none arrives.
///
/// `recv_json` is crate-internal, so integration tests go through the public
/// string form and parse.
async fn next_event(rx: &mut AnyReceiver) -> Option<(&'static str, serde_json::Value)> {
    let (topic, json) = tokio::time::timeout(Duration::from_secs(2), rx.recv_json_str())
        .await
        .ok()?
        .ok()?;
    Some((topic, serde_json::from_str(&json).expect("event is JSON")))
}

/// Drain every event currently pending.
async fn drain_events(rx: &mut AnyReceiver) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    while let Ok(Ok((_, json))) =
        tokio::time::timeout(Duration::from_millis(50), rx.recv_json_str()).await
    {
        out.push(serde_json::from_str(&json).expect("event is JSON"));
    }
    out
}

/// The speak path must announce itself on `/events`, so a panel applet can show
/// speech without polling.
#[tokio::test]
async fn speaking_state_is_published_around_an_utterance() {
    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let bus = Arc::new(EventBus::new());
    let mut rx = bus.subscribe(Topic::SpeakingState);
    let engine = SpeechEngine::detached(DEVICE).with_events(Arc::clone(&bus));

    let utterance = engine
        .speak(&model, "Hello there.", None, None)
        .await
        .expect("speak");

    let (topic, start) = next_event(&mut rx).await.expect("a start event");
    assert_eq!(topic, "speaking_state");
    assert_eq!(start["is_speaking"], true);
    assert_eq!(start["utterance_id"], utterance.id.as_str());

    // Synthesis is over, but nothing has been heard: a backend faster than
    // realtime queues the whole utterance before the first word sounds, and
    // announcing the end here is exactly the bug this guards.
    assert!(
        next_event(&mut rx).await.is_none(),
        "no stop may be published while the audio is still queued"
    );

    play_out(&engine).await;
    let (_, stop) = next_event(&mut rx).await.expect("a stop event");
    assert_eq!(stop["is_speaking"], false);
    assert_eq!(stop["utterance_id"], utterance.id.as_str());
}

/// A superseded utterance must not clear the state belonging to the one that
/// replaced it — otherwise the applet shows "not speaking" while speech plays.
#[tokio::test]
async fn a_superseded_utterance_does_not_clear_the_new_ones_state() {
    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let bus = Arc::new(EventBus::new());
    let mut rx = bus.subscribe(Topic::SpeakingState);
    let engine = SpeechEngine::detached(DEVICE).with_events(Arc::clone(&bus));

    let mut first = engine
        .begin(&model, SpeakOptions::default())
        .await
        .expect("begin");
    let second = engine
        .speak(&model, "Interrupting.", None, None)
        .await
        .expect("second");
    // The first session is now superseded; ending it must not publish a stop.
    first.end().await.expect("end the superseded session");
    // The second is still playing out, so its own stop is still to come.
    play_out(&engine).await;

    let events = drain_events(&mut rx).await;
    let last_stop = events
        .iter()
        .rev()
        .find(|e| e["is_speaking"] == false)
        .expect("a stop was published");
    assert_eq!(
        last_stop["utterance_id"],
        second.id.as_str(),
        "the final stop must belong to the utterance that actually finished"
    );
}

/// Interrupting an utterance that is still playing out must not put a `false`
/// in front of the new one's `true`. The device never went quiet, and a
/// subscriber has no way to read that pair as anything but a gap — the applet
/// drops a live visualizer to idle on it.
#[tokio::test]
async fn superseding_a_playing_utterance_does_not_report_a_stop() {
    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let bus = Arc::new(EventBus::new());
    let mut rx = bus.subscribe(Topic::SpeakingState);
    let engine = SpeechEngine::detached(DEVICE).with_events(Arc::clone(&bus));

    // The first utterance is queued and never rendered, so it is still playing
    // out — the case a user barges in on.
    let first = engine
        .speak(&model, "The first thing.", None, None)
        .await
        .expect("first");
    let second = engine
        .speak(&model, "No, this instead.", None, None)
        .await
        .expect("second");

    let events = drain_events(&mut rx).await;
    let states: Vec<_> = events
        .iter()
        .map(|e| (e["is_speaking"].as_bool(), e["utterance_id"].as_str()))
        .collect();
    assert_eq!(
        states,
        vec![
            (Some(true), Some(first.id.as_str())),
            (Some(true), Some(second.id.as_str())),
        ],
        "one utterance replacing another is a handover, not a stop and a start"
    );
}

/// The visualizer is fed from playback now, so the applet renders speech the
/// same way it used to render a microphone.
#[tokio::test]
async fn frequency_bands_are_published_from_playback() {
    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let bus = Arc::new(EventBus::new());
    let mut rx = bus.subscribe(Topic::FrequencyBands);
    let engine = SpeechEngine::detached(DEVICE).with_events(Arc::clone(&bus));

    engine
        .speak(&model, "Hello there.", None, None)
        .await
        .expect("speak");

    // Analysis is done and every sample is queued, but nothing has reached a
    // device. Publishing here is what made a visualizer go flat a full ring
    // before the voice stopped — which reads as speech having ended.
    assert!(
        next_event(&mut rx).await.is_none(),
        "no bands may be published for audio nobody has heard yet"
    );

    play_out(&engine).await;
    let (topic, bands) = next_event(&mut rx).await.expect("bands were published");
    assert_eq!(topic, "frequency_bands");
    assert_eq!(
        bands["sample_rate"], 24000.0,
        "the analyzer runs at the backend's rate, before resampling"
    );
    assert!(bands["bands_b64"].is_string());
}

/// The daemon's FFT window, in samples at the backend's rate.
const ANALYSIS_WINDOW: usize = 1024;

/// One frame per analysis window, each released as playback reaches it. A
/// frame per backend frame is what held the applet to 1.5 updates a second
/// under Qwen, whose frames are 0.68 s long. The mock's frames are short
/// instead, so here the count is what tells the two apart; a long frame cut
/// into many windows is covered by the unit tests.
#[tokio::test]
async fn frequency_bands_follow_playback_window_by_window() {
    let Some(model) = loaded_model_with_limit(40) else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let bus = Arc::new(EventBus::new());
    let mut rx = bus.subscribe(Topic::FrequencyBands);
    let engine = SpeechEngine::detached(DEVICE).with_events(Arc::clone(&bus));

    let text = "First sentence here. Second sentence follows. Third one arrives now. \
                And a fourth to finish.";
    let utterance = engine.speak(&model, text, None, None).await.expect("speak");
    assert!(
        utterance.chunks > 1,
        "the text should span several responses"
    );
    // Windows run on across the mock's 960-sample responses rather than
    // restarting in each, and the last, partial one is padded out.
    // A frame per backend frame would be four per response: the mock splits
    // each into four 240-sample frames.
    let windows = (utterance.chunks * MOCK_SAMPLES).div_ceil(ANALYSIS_WINDOW);

    // Half the audio heard is about half the frames, and no more.
    let queued = engine
        .playback_handle()
        .await
        .expect("pipeline opened")
        .buffered();
    drain(&engine, queued / 2).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let heard = drain_events(&mut rx).await.len();
    assert!(
        heard > 0 && heard < windows,
        "{heard} of {windows} frames published with half the audio played"
    );

    play_out(&engine).await;
    let rest = drain_events(&mut rx).await.len();
    assert_eq!(
        heard + rest,
        windows,
        "one frame per window, each published once"
    );
}

#[tokio::test]
async fn speech_progress_is_published_for_the_utterance() {
    let Some(model) = loaded_model() else {
        eprintln!("skipping: mock component not built");
        return;
    };
    let bus = Arc::new(EventBus::new());
    let mut rx = bus.subscribe(Topic::SpeechProgress);
    let engine = SpeechEngine::detached(DEVICE).with_events(Arc::clone(&bus));

    let utterance = engine
        .speak(&model, "Hello there.", None, None)
        .await
        .expect("speak");

    let (topic, progress) = next_event(&mut rx).await.expect("progress was published");
    assert_eq!(topic, "speech_progress");
    assert_eq!(progress["utterance_id"], utterance.id.as_str());
    assert!(progress["queued_ms"].as_u64().is_some());
}
