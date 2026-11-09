// SPDX-License-Identifier: GPL-3.0-only
//! Null-sink tests for the playback pipeline.
//!
//! Every test drives [`Playback::render`] — the same function the cpal callback
//! calls — against a [`Playback::detached`] pipeline, so the whole path runs
//! with no audio device present and behaves identically in CI and on a desktop.

// cpal 0.18, which text to speech plays through; speech to text still
// captures with the workspace's 0.17 under the plain name.
use super::*;
use cpal_playback as cpal;
use mixer::SeamFade;
use super_voice_shared::tts::audio::frames::SampleFormat;

const DEVICE: DeviceFormat = DeviceFormat {
    sample_rate: 48000,
    channels: 1,
};

/// The stop flag of an utterance nobody stops, for tests about everything else.
static LIVE: AtomicBool = AtomicBool::new(false);

fn params(sample_rate: u32, channels: u16) -> AudioParams {
    AudioParams {
        sample_rate,
        channels,
        format: SampleFormat::F32Le,
    }
}

/// Pull `n` samples in `block`-sized calls, exactly as a device would.
fn drain(p: &Playback, n: usize, block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n);
    let mut buf = vec![0.0; block];
    while out.len() < n {
        p.render(&mut buf);
        out.extend_from_slice(&buf);
    }
    out.truncate(n);
    out
}

/// Largest sample-to-sample jump — the measure of a seam being audible.
fn max_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0_f32, f32::max)
}

#[tokio::test]
async fn silence_is_rendered_before_anything_is_queued() {
    let p = Playback::detached(DEVICE);
    assert_eq!(p.state(), State::Idle);
    let out = drain(&p, 512, 128);
    assert!(out.iter().all(|s| *s == 0.0), "idle must be silent");
    assert_eq!(p.stats().underrun_samples, 0, "idle is not an underrun");
}

#[tokio::test]
async fn nothing_is_heard_until_the_prebuffer_fills() {
    let p = Playback::detached(DEVICE);
    // 10 ms at 48 kHz — well under the 120 ms prebuffer.
    p.push(&vec![1.0_f32; 480], params(48000, 1), &LIVE)
        .await
        .unwrap();
    assert_eq!(p.state(), State::Prebuffering);
    let out = drain(&p, 480, 240);
    assert!(
        out.iter().all(|s| *s == 0.0),
        "prebuffering must not emit a partial start"
    );
    // One fade's worth is held back for the closing fade-out, so the ring holds
    // the rest.
    let fade = SeamFade::new(DEFAULT_FADE_MS, DEVICE).samples();
    assert_eq!(
        p.buffered(),
        480 - fade,
        "and must not consume what it is holding"
    );
}

#[tokio::test]
async fn playback_starts_once_the_prebuffer_threshold_is_met() {
    let p = Playback::detached(DEVICE);
    // 200 ms > the 120 ms threshold.
    p.push(&vec![0.5_f32; 9600], params(48000, 1), &LIVE)
        .await
        .unwrap();
    assert_eq!(p.state(), State::Playing);
    let out = drain(&p, 4800, 480);
    // Past the head fade the level is untouched; the fade itself is checked in
    // the mixer's own tests.
    let fade = SeamFade::new(DEFAULT_FADE_MS, DEVICE).samples();
    assert!(
        out[fade..].iter().all(|s| (*s - 0.5).abs() < 1e-6),
        "the body of the chunk plays at its original level"
    );
    assert!(
        out[0] < 0.5,
        "and the head is faded in rather than starting on a step"
    );
}

/// The point of the whole module: two independently synthesized chunks must
/// come out as one continuous waveform. The boundary is explicit — only the
/// caller knows where one synthesis response ended and the next began.
#[tokio::test]
async fn separately_synthesized_chunks_join_without_a_step() {
    let p = Playback::detached(DEVICE);
    // Two DC blocks at opposite levels: butted together they step by 2.0.
    p.push(&vec![1.0_f32; 24000], params(48000, 1), &LIVE)
        .await
        .unwrap();
    p.end_chunk(&LIVE).await;
    p.push(&vec![-1.0_f32; 24000], params(48000, 1), &LIVE)
        .await
        .unwrap();
    p.finish(&LIVE).await;

    let out = drain(&p, 48000, 512);
    let step = max_step(&out);
    assert!(
        step < 0.2,
        "the seam between chunks must be faded; largest step was {step}"
    );
}

/// Continuity has to survive resampling too — a 24 kHz backend into a 48 kHz
/// device is the ordinary case, not an edge one.
#[tokio::test]
async fn continuity_survives_resampling_from_the_backend_rate() {
    let p = Playback::detached(DEVICE);
    let sine = |n: usize, phase: f32| -> Vec<f32> {
        (0..n)
            .map(|i| (i as f32 * 0.02 + phase).sin() * 0.5)
            .collect()
    };
    p.push(&sine(12000, 0.0), params(24000, 1), &LIVE)
        .await
        .unwrap();
    p.end_chunk(&LIVE).await;
    p.push(&sine(12000, 1.0), params(24000, 1), &LIVE)
        .await
        .unwrap();
    p.finish(&LIVE).await;

    let out = drain(&p, 40000, 512);
    let step = max_step(&out);
    assert!(
        step < 0.2,
        "resampled chunks must still join smoothly; largest step was {step}"
    );
    assert!(
        out.iter().any(|s| s.abs() > 0.1),
        "the signal must actually be present, not silence"
    );
}

#[tokio::test]
async fn a_mono_backend_is_spread_across_a_stereo_device() {
    let stereo = DeviceFormat {
        sample_rate: 48000,
        channels: 2,
    };
    let p = Playback::detached(stereo);
    // 200 ms of mono, which must become 200 ms of stereo frames.
    p.push(&vec![0.25_f32; 9600], params(48000, 1), &LIVE)
        .await
        .unwrap();
    p.finish(&LIVE).await;

    let out = drain(&p, 19200, 512);
    assert_eq!(out.len(), 19200, "200 ms of mono becomes 200 ms of stereo");
    // Both channels must match everywhere, fade included — the fade is a gain,
    // and applying it unevenly would pull the image off centre.
    for frame in out.chunks(2) {
        assert!(
            (frame[1] - frame[0]).abs() < 1e-6,
            "both channels carry the same mono sample"
        );
    }
    let fade = SeamFade::new(DEFAULT_FADE_MS, stereo).samples();
    for frame in out[fade..out.len() - fade].chunks(2) {
        assert!(
            (frame[0] - 0.25).abs() < 1e-6,
            "the body keeps the source level, got {}",
            frame[0]
        );
    }
}

#[tokio::test]
async fn an_underrun_renders_silence_and_is_counted() {
    let p = Playback::detached(DEVICE);
    p.push(&vec![1.0_f32; 9600], params(48000, 1), &LIVE)
        .await
        .unwrap();
    assert_eq!(p.state(), State::Playing);

    // Ask for far more than was queued while still Playing — the producer has
    // not said it is finished, so the shortfall is a genuine underrun.
    let out = drain(&p, 19200, 4800);
    assert!(
        out.iter().all(|s| s.is_finite()),
        "an underrun must not emit garbage"
    );
    assert!(
        out[15000..].iter().all(|s| *s == 0.0),
        "past the queued audio the output is silence"
    );
    assert!(
        p.stats().underrun_samples > 0,
        "the shortfall must be counted"
    );
}

/// Draining short is the utterance ending, not a failure to keep up — counting
/// it would make every normal utterance look like a glitch.
#[tokio::test]
async fn draining_past_the_end_is_not_counted_as_an_underrun() {
    let p = Playback::detached(DEVICE);
    p.push(&vec![1.0_f32; 9600], params(48000, 1), &LIVE)
        .await
        .unwrap();
    p.finish(&LIVE).await;
    let _ = drain(&p, 19200, 4800);
    assert_eq!(p.stats().underrun_samples, 0);
    assert_eq!(p.state(), State::Idle, "the stream idles once drained");
}

/// A reply shorter than the prebuffer threshold must still be heard.
#[tokio::test]
async fn an_utterance_shorter_than_the_prebuffer_still_plays() {
    let p = Playback::detached(DEVICE);
    p.push(&vec![0.75_f32; 960], params(48000, 1), &LIVE)
        .await
        .unwrap(); // 20 ms
    assert_eq!(p.state(), State::Prebuffering);
    p.finish(&LIVE).await;
    assert_eq!(p.state(), State::Draining);

    let out = drain(&p, 960, 240);
    assert!(
        out.iter().any(|s| (*s - 0.75).abs() < 1e-6),
        "a short utterance must not be stuck in the prebuffer"
    );
}

#[tokio::test]
async fn cancel_goes_silent_within_one_buffer() {
    let p = Playback::detached(DEVICE);
    p.push(&vec![1.0_f32; 48000], params(48000, 1), &LIVE)
        .await
        .unwrap();
    assert_eq!(p.state(), State::Playing);

    p.cancel();
    assert_eq!(p.state(), State::Idle);
    assert_eq!(p.buffered(), 0, "queued audio is discarded, not played out");

    let out = drain(&p, 4800, 480);
    assert!(
        out.iter().all(|s| *s == 0.0),
        "nothing queued before the cancel may still be heard"
    );
}

#[tokio::test]
async fn a_cancelled_stream_accepts_a_new_utterance() {
    let p = Playback::detached(DEVICE);
    p.push(&vec![1.0_f32; 48000], params(48000, 1), &LIVE)
        .await
        .unwrap();
    p.cancel();

    p.push(&vec![-0.5_f32; 9600], params(48000, 1), &LIVE)
        .await
        .unwrap();
    p.finish(&LIVE).await;
    let out = drain(&p, 9600, 480);
    assert!(
        out.iter().any(|s| (*s + 0.5).abs() < 1e-6),
        "the pipeline must be reusable after a cancel"
    );
    assert!(
        out.iter().all(|s| *s <= 0.0 + 1e-6),
        "no sample from the cancelled utterance survives"
    );
}

/// The stop that did not stop. A producer running ahead of the speakers has a
/// write waiting on a full ring, and emptying the ring used to hand that write
/// the room to refill it: the listener heard the voice cut, then come back a
/// ring's worth further on and carry on through the backlog.
#[tokio::test]
async fn a_cancel_is_not_undone_by_a_write_waiting_for_space() {
    let p = Arc::new(Playback::detached(DEVICE));
    let stop = Arc::new(AtomicBool::new(false));
    let capacity = (DEVICE.sample_rate * RING_SECONDS) as usize;

    // Five seconds into a two-second ring: the write fills it and waits.
    let writer = tokio::spawn({
        let p = Arc::clone(&p);
        let stop = Arc::clone(&stop);
        async move {
            p.push(&vec![1.0_f32; 5 * 48000], params(48000, 1), &stop)
                .await
        }
    });
    for _ in 0..1000 {
        if p.buffered() == capacity {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(
        p.buffered(),
        capacity,
        "the write filled the ring and waits"
    );
    let _ = drain(&p, 24000, 480);

    stop.store(true, Ordering::Relaxed);
    p.cancel();
    // Long enough for the waiting write to retry several times.
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(p.buffered(), 0, "nothing was written after the cancel");
    assert_eq!(p.state(), State::Idle);
    let out = drain(&p, 4800, 480);
    assert!(out.iter().all(|s| *s == 0.0), "and nothing more is heard");
    let gave_up = tokio::time::timeout(Duration::from_millis(100), writer).await;
    assert!(
        matches!(gave_up, Ok(Ok(Ok(())))),
        "the write dropped its remainder rather than waiting for room: {gave_up:?}"
    );
}

/// A producer that learns of its stop late must not reach into the utterance
/// that replaced it: a tail held back after the cancel would open the new one
/// with a fragment of the old, and without the new one's fade-in.
#[tokio::test]
async fn a_stopped_utterance_leaves_nothing_for_the_next() {
    let p = Playback::detached(DEVICE);
    let old = AtomicBool::new(false);
    p.push(&vec![1.0_f32; 9600], params(48000, 1), &old)
        .await
        .unwrap();
    old.store(true, Ordering::Relaxed);
    p.cancel();

    // The stopped utterance's next frame and its chunk boundary, arriving late.
    p.push(&vec![1.0_f32; 9600], params(48000, 1), &old)
        .await
        .unwrap();
    p.end_chunk(&old).await;
    assert_eq!(p.buffered(), 0, "a stopped utterance writes nothing");
    assert_eq!(p.state(), State::Idle);

    let new = AtomicBool::new(false);
    p.push(&vec![-0.5_f32; 9600], params(48000, 1), &new)
        .await
        .unwrap();
    p.finish(&new).await;
    let out = drain(&p, 9600, 480);
    assert!(
        out.iter().all(|s| *s <= 0.0),
        "no sample of the stopped utterance reaches the next"
    );
    assert!(
        out[0].abs() < 0.01,
        "the next utterance opens on its own fade-in, got {}",
        out[0]
    );
}

#[tokio::test]
async fn an_empty_chunk_is_a_no_op() {
    let p = Playback::detached(DEVICE);
    p.push(&[], params(48000, 1), &LIVE).await.unwrap();
    assert_eq!(p.state(), State::Idle);
    assert_eq!(p.buffered(), 0);
}

/// The visualizer stamps each window with where its audio will be heard, and
/// the fade tail `push` holds back sits ahead of the next chunk even though it
/// is not in the ring yet.
#[tokio::test]
async fn the_queue_position_counts_the_held_tail() {
    let p = Playback::detached(DEVICE);
    let fade = SeamFade::new(DEFAULT_FADE_MS, DEVICE).samples() as u64;

    p.push(&vec![0.1_f32; 4800], params(48000, 1), &LIVE)
        .await
        .unwrap();
    assert_eq!(p.stats().written_samples, 4800 - fade);
    assert_eq!(
        p.queue_position(),
        4800,
        "the next chunk lands after the tail"
    );

    p.end_chunk(&LIVE).await;
    assert_eq!(
        p.queue_position(),
        4800,
        "closing the chunk writes the tail out; it does not move the position"
    );
    p.push(&vec![0.1_f32; 4800], params(48000, 1), &LIVE)
        .await
        .unwrap();
    assert_eq!(p.queue_position(), 9600);
}

/// A backend faster than realtime must not be able to buffer without bound —
/// `push` blocks on a full ring instead of growing it.
#[tokio::test]
async fn a_full_ring_applies_backpressure_to_the_producer() {
    let p = Playback::detached(DEVICE);
    let capacity = (DEVICE.sample_rate * RING_SECONDS) as usize;

    // Queue the ring full, then confirm a further push cannot complete while
    // nothing is draining it.
    p.push(&vec![0.1_f32; capacity], params(48000, 1), &LIVE)
        .await
        .unwrap();
    let fade = SeamFade::new(DEFAULT_FADE_MS, DEVICE).samples();
    assert_eq!(p.buffered(), capacity - fade, "less the held tail");

    let pending = tokio::time::timeout(
        Duration::from_millis(50),
        p.push(&vec![0.2_f32; 4800], params(48000, 1), &LIVE),
    )
    .await;
    assert!(
        pending.is_err(),
        "push must wait for space rather than growing the buffer"
    );
    assert!(
        p.buffered() <= capacity,
        "the ring never exceeds its allocated capacity"
    );
}

#[tokio::test]
async fn rendering_a_zero_length_buffer_is_harmless() {
    let p = Playback::detached(DEVICE);
    p.push(&vec![1.0_f32; 9600], params(48000, 1), &LIVE)
        .await
        .unwrap();
    let mut empty: [f32; 0] = [];
    p.render(&mut empty);
    let fade = SeamFade::new(DEFAULT_FADE_MS, DEVICE).samples();
    assert_eq!(p.buffered(), 9600 - fade, "nothing was consumed");
}

/// The point of [`Playback::wait_for_playout`]: `finish` means queued, and the
/// listener has heard nothing yet. Waiting must not return until a device has
/// actually consumed the ring.
#[tokio::test]
async fn waiting_for_playout_outlasts_the_queued_audio() {
    let p = Arc::new(Playback::detached(DEVICE));
    // A second of audio — well under the ring, so `finish` returns with all of
    // it still unheard, exactly as a faster-than-realtime backend leaves it.
    p.push(&vec![0.5_f32; 48000], params(48000, 1), &LIVE)
        .await
        .unwrap();
    p.finish(&LIVE).await;
    assert_eq!(p.state(), State::Draining);

    let waiting = tokio::spawn({
        let p = Arc::clone(&p);
        async move { p.wait_for_playout(Duration::from_secs(30)).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !waiting.is_finished(),
        "a full ring is not a finished utterance"
    );

    // Now be the device.
    let mut buf = vec![0.0_f32; 480];
    while p.buffered() > 0 {
        p.render(&mut buf);
    }
    p.render(&mut buf);
    assert_eq!(p.state(), State::Idle);

    let drained = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("the wait must end once the audio has been played")
        .expect("the waiter did not panic");
    assert!(
        drained,
        "a ring that played out is a clean finish, not a stall"
    );
}

/// A sink that stops consuming must not hold the utterance open for good: the
/// slot it gates is what lets the daemon load a different model.
#[tokio::test]
async fn a_stalled_device_gives_up_rather_than_waiting_forever() {
    let p = Playback::detached(DEVICE);
    p.push(&vec![0.5_f32; 48000], params(48000, 1), &LIVE)
        .await
        .unwrap();
    p.finish(&LIVE).await;

    // Nothing renders, so nothing is ever consumed.
    let drained = tokio::time::timeout(
        Duration::from_secs(5),
        p.wait_for_playout(Duration::from_millis(150)),
    )
    .await
    .expect("the wait must time out rather than hang");
    assert!(
        !drained,
        "a device that never consumed must be reported as stalled"
    );
    assert!(p.buffered() > 0, "and the audio is still sitting there");
}

/// Progress on a slow sink is not a stall. The timeout must measure *silence
/// from the device*, not total time, or a long utterance on a slow sink would
/// be cut short — which is the failure this whole path is fixing.
#[tokio::test]
async fn a_slow_but_moving_device_is_not_treated_as_stalled() {
    let p = Arc::new(Playback::detached(DEVICE));
    p.push(&vec![0.5_f32; 48000], params(48000, 1), &LIVE)
        .await
        .unwrap();
    p.finish(&LIVE).await;

    let feeding = tokio::spawn({
        let p = Arc::clone(&p);
        async move {
            let mut buf = vec![0.0_f32; 480];
            // 10 ms of audio every 20 ms: half of realtime, and far longer
            // overall than the stall timeout below.
            while p.buffered() > 0 {
                p.render(&mut buf);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            p.render(&mut buf);
        }
    });

    let drained = tokio::time::timeout(
        Duration::from_secs(20),
        p.wait_for_playout(Duration::from_millis(300)),
    )
    .await
    .expect("the wait must end");
    feeding.await.expect("the feeder did not panic");
    assert!(
        drained,
        "a device making progress is not stalled, however slowly"
    );
}

/// An xrun with nothing playing is not a dropout: the daemon holds the device
/// open between utterances and feeds it silence, so the device going hungry
/// there costs the listener nothing. Reported as such, and counted — bursts of
/// these used to read as a speech fault minutes after anyone last spoke.
#[test]
fn a_stream_error_is_told_apart_by_what_was_playing() {
    let p = Playback::detached(DEVICE);
    assert_eq!(p.state(), State::Idle);
    // Both branches must run without panicking or blocking the stream thread;
    // the counter is what proves each one was taken.
    stream_error(&p.shared, &p.stream_errors, &cpal::ErrorKind::Xrun.into());
    assert_eq!(
        p.stream_errors.load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    p.shared.lock().state = State::Playing;
    stream_error(&p.shared, &p.stream_errors, &cpal::ErrorKind::Xrun.into());
    assert_eq!(
        p.stream_errors.load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}

/// The classifier must never wait on the producer's lock: it runs on the same
/// thread cpal uses to meet the device deadline.
#[test]
fn a_stream_error_reported_while_the_lock_is_held_does_not_block() {
    let p = Playback::detached(DEVICE);
    let held = p.shared.lock();
    stream_error(&p.shared, &p.stream_errors, &cpal::ErrorKind::Xrun.into());
    drop(held);
    assert_eq!(
        p.stream_errors.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "the report still lands even when the state cannot be read"
    );
}

/// A device that went away ends cpal's ALSA stream, so it is reported loudly
/// even with nothing playing: every utterance after it is silent.
#[test]
fn a_device_gone_is_reported_whatever_was_playing() {
    let p = Playback::detached(DEVICE);
    assert_eq!(p.state(), State::Idle);
    stream_error(
        &p.shared,
        &p.stream_errors,
        &cpal::ErrorKind::DeviceNotAvailable.into(),
    );
    assert_eq!(
        p.stream_errors.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

fn range(
    format: cpal::SampleFormat,
    channels: u16,
    min: u32,
    max: u32,
) -> cpal::SupportedStreamConfigRange {
    cpal::SupportedStreamConfigRange::new(
        channels,
        min,
        max,
        cpal::SupportedBufferSize::Unknown,
        format,
    )
}

/// cpal 0.18 defaults a device without `F32` to `I32` where 0.17 chose `I16`.
/// The pipeline asks for the `I16` that device still offers, at the rate and
/// channel count the default named, rather than refusing the device.
#[test]
fn an_integer_default_falls_back_to_i16_at_the_same_rate_and_channels() {
    let default = cpal::SupportedStreamConfig::new(
        2,
        48000,
        cpal::SupportedBufferSize::Unknown,
        cpal::SampleFormat::I32,
    );
    let offered = [
        range(cpal::SampleFormat::I32, 2, 8000, 192_000),
        range(cpal::SampleFormat::I16, 1, 8000, 192_000),
        range(cpal::SampleFormat::I16, 2, 8000, 44100),
        range(cpal::SampleFormat::I16, 2, 8000, 192_000),
    ];
    let chosen = i16_alternative(&default, offered).expect("an I16 config is offered");
    assert_eq!(chosen.sample_format(), cpal::SampleFormat::I16);
    assert_eq!(chosen.channels(), 2);
    assert_eq!(chosen.sample_rate(), 48000);
}

/// With no `I16` at the default's shape there is nothing to steer to; the
/// default stands and `open` names the format it cannot write.
#[test]
fn no_matching_i16_leaves_the_default_alone() {
    let default = cpal::SupportedStreamConfig::new(
        2,
        48000,
        cpal::SupportedBufferSize::Unknown,
        cpal::SampleFormat::I24,
    );
    let offered = [
        range(cpal::SampleFormat::I24, 2, 48000, 48000),
        range(cpal::SampleFormat::I16, 6, 48000, 48000),
        range(cpal::SampleFormat::I16, 2, 44100, 44100),
    ];
    assert!(i16_alternative(&default, offered).is_none());
    assert!(!renders(cpal::SampleFormat::I24));
    assert!(renders(cpal::SampleFormat::F32) && renders(cpal::SampleFormat::I16));
}
