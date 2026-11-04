// SPDX-License-Identifier: GPL-3.0-only
//! Streaming speech playback: synthesis chunks in, a continuous audio stream
//! out.
//!
//! The daemon's existing output path ([`crate::tts::audio::beeper`]) plays a
//! fully-precomputed, known-length buffer — "these exact 200 ms of sine wave".
//! Speech is the opposite: chunks arrive over time, at a rate that is not the
//! device's, from separate synthesis requests, and must not gap between them or
//! keep sounding once the user interrupts.
//!
//! Three pieces, split by which thread they run on:
//!
//! - [`mixer`] prepares a chunk — resample, channel-map, fade the seam.
//!   Producer side, pure, allocates freely.
//! - [`ring`] is the handoff: a preallocated buffer both sides memcpy through.
//! - [`Playback`] owns the state machine and the cpal stream. The audio
//!   callback only ever locks briefly and memcpys.
//!
//! Everything except the cpal binding is exercised through [`Playback::render`]
//! — the same entry point the callback uses — so the pipeline is testable
//! against a null sink with no audio device present.
//!
//! **Realtime discipline.** The crate builds with `panic = "abort"`, so a panic
//! on the audio callback thread takes the daemon down mid-utterance instead of
//! glitching. The callback path therefore has no indexing that can go out of
//! bounds, no unwrap, no allocation, and never blocks: it takes the state lock
//! with `try_lock` and renders silence if it cannot get it.

pub mod mixer;
pub mod ring;

// cpal 0.18, which text to speech plays through; speech to text still
// captures with the workspace's 0.17 under the plain name.
use cpal_playback as cpal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use log::{debug, warn};
use parking_lot::Mutex;
use super_tts_shared::audio::frames::AudioParams;

pub use mixer::DeviceFormat;
use mixer::SeamFade;
use ring::Ring;

/// How much audio the ring can hold. Two seconds is enough to absorb a slow
/// chunk without the buffer itself becoming a latency floor.
const RING_SECONDS: u32 = 2;

/// How much audio must be buffered before the stream starts.
///
/// Starting on the first sample would underrun immediately on any backend
/// slower than realtime; waiting too long is dead air the user hears as lag.
/// 120 ms is roughly a syllable.
const PREBUFFER_MS: u32 = 120;

/// How often the producer looks in on a draining ring.
///
/// Only a floor: [`Playback::wait_for_playout`] sleeps for however much audio
/// is still buffered, so a two-second tail costs a couple of wakeups rather
/// than two hundred. Short enough that the residual past the last sample is a
/// fraction of a device block, long enough that the audio callback's `try_lock`
/// never meaningfully contends with it.
const DRAIN_POLL_MS: u64 = 10;

/// Default fade applied to each end of every chunk. Applied per end, so a seam
/// dips over roughly twice this. Long enough to hide the discontinuity between
/// two independently synthesized sentences, short enough to be inaudible in
/// speech — and sentence boundaries usually carry natural silence anyway.
const DEFAULT_FADE_MS: u32 = 8;

/// What the stream is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Nothing queued; the callback renders silence.
    Idle,
    /// Filling toward [`PREBUFFER_MS`] before the first sample is heard.
    Prebuffering,
    /// Audio is being consumed.
    Playing,
    /// The producer is done; play out what is buffered, then go idle.
    Draining,
}

/// Counters a settings UI or a test can read back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Interleaved samples the callback had to fill with silence because the
    /// ring was empty while playing.
    pub underrun_samples: u64,
    /// Times the callback could not take the state lock and rendered silence.
    /// Non-zero here is contention, which is a different bug from an underrun.
    pub lock_misses: u64,
    /// Interleaved samples actually handed to the device since the last
    /// [`Playback::cancel`]. Counts only real audio, not the silence rendered
    /// while idle, so it is a position within the utterance rather than
    /// wall-clock time.
    pub rendered_samples: u64,
    /// Interleaved samples written *into* the ring since the last
    /// [`Playback::cancel`]. Always at or ahead of `rendered_samples`, by
    /// however much is buffered.
    ///
    /// The pair is what lets a producer say when a chunk it is queueing now
    /// will be heard: the chunk starts at the current `written_samples`, and it
    /// is sounding once `rendered_samples` passes that. The visualizer is fed
    /// this way — see `daemon::speech` — because analysis has to run on the
    /// producer side while the frames must reach subscribers on the listener's
    /// clock, up to a full ring later.
    pub written_samples: u64,
}

/// State shared between the producer task and the audio callback.
#[derive(Debug)]
struct Shared {
    ring: Ring,
    state: State,
    prebuffer_samples: usize,
    stats: Stats,
    /// Largest block the device has asked for, in interleaved samples.
    ///
    /// Learned from the callback rather than from the stream config, because
    /// `BufferSize::Default` means the host picks and only the callback is told
    /// what it picked. The producer reads it to account for audio that has been
    /// handed over but not yet sounded — see
    /// [`Playback::wait_for_playout`].
    block: usize,
}

impl Shared {
    /// Fill `out` from the ring, silencing whatever the ring could not supply.
    ///
    /// This is the whole consumer side. It runs on the audio callback thread,
    /// so it allocates nothing and cannot panic: `Ring::read` is a bounded
    /// memcpy and the remainder is a slice fill.
    fn render(&mut self, out: &mut [f32]) {
        // A compare and a store on state this thread already holds: the
        // cheapest thing in the callback, and the only way to learn the
        // device's real block size.
        self.block = self.block.max(out.len());
        match self.state {
            State::Idle | State::Prebuffering => {
                out.fill(0.0);
                return;
            }
            State::Playing | State::Draining => {}
        }

        let got = self.ring.read(out);
        self.stats.rendered_samples += got as u64;
        if got < out.len() {
            out[got..].fill(0.0);
            if self.state == State::Playing {
                // Draining is expected to end short — that is the utterance
                // finishing, not a failure to keep up.
                self.stats.underrun_samples += (out.len() - got) as u64;
            }
        }
        if self.state == State::Draining && self.ring.is_empty() {
            self.state = State::Idle;
        }
    }

    /// Promote out of `Prebuffering` once enough is buffered.
    fn maybe_start(&mut self) {
        if self.state == State::Prebuffering && self.ring.available() >= self.prebuffer_samples {
            self.state = State::Playing;
        }
    }
}

/// The playback pipeline's producer half: queue chunks, drain, cancel.
///
/// Deliberately holds no `cpal::Stream`. cpal's stream is `!Send` on several
/// backends because it is bound to the thread that created it, and the daemon
/// needs to reach playback from async tasks on the tokio pool. Rather than
/// asserting `Send` on a handle the platform says is not, the stream is kept
/// where it was created — see [`Stream`] — and only the `Arc<Mutex<Shared>>`
/// crosses threads, which is genuinely shared state.
///
/// It also means the pipeline can be built and driven with no audio device at
/// all: [`Playback::render`] is the same code the callback runs.
/// Every producer method takes `&self`: [`SeamFade`] is stateless and `Copy`,
/// so all mutable state lives behind the `Arc<Mutex<Shared>>` the callback
/// shares. That makes `Playback` `Sync`, so the speak path can hold one
/// `Arc<Playback>` and push, finish, and cancel from different tasks without an
/// outer lock — and, critically, without ever holding a lock across an `await`,
/// which would stall the audio callback's `try_lock`.
pub struct Playback {
    shared: Arc<Mutex<Shared>>,
    /// Stream errors reported since the device was opened.
    ///
    /// Outside the mutex on purpose: the error callback runs on cpal's stream
    /// thread, which is the same thread that must meet the device deadline, so
    /// it cannot afford to wait on the lock the producer holds.
    stream_errors: Arc<std::sync::atomic::AtomicU64>,
    device: DeviceFormat,
    fade: SeamFade,
    /// Producer-side fade bookkeeping. Separate from `Shared` so the audio
    /// callback never contends on it, and small enough that the lock is only
    /// ever held for a splice — never across an `await`.
    producer: Mutex<Producer>,
}

/// Where the producer is within the current utterance.
#[derive(Debug, Default)]
struct Producer {
    /// Whether the utterance's opening fade has been applied.
    started: bool,
    /// The last `fade` frames, held back so [`Playback::finish`] can fade them
    /// out. Costs one fade of latency (8 ms) and is what makes the closing
    /// fade possible at all — the samples must still be in reach when the
    /// utterance turns out to be over.
    tail: Vec<f32>,
}

/// Report a cpal stream error, told apart by whether anything was playing.
///
/// An xrun while the ring is idle means the device went hungry with nothing to
/// feed it — the daemon was writing silence, so no speech was lost and there is
/// nothing for the user to act on. It happens because the stream is held open
/// between utterances (see [`Playback::open`]), and on a loaded machine it
/// happens in bursts, which as a `warn!` each buried the log in what looked
/// like a speech fault minutes after anyone last spoke. An xrun *during*
/// playback is the opposite: the listener heard the gap.
///
/// A device that went away is loud whatever was playing: cpal's ALSA worker
/// exits after reporting it, so this is the last the stream says, and every
/// utterance after it is silent.
///
/// Runs on cpal's stream thread, so it takes the lock with `try_lock` and
/// treats a miss as "cannot tell" — reporting loudly is the safe way to be
/// wrong.
fn stream_error(shared: &Mutex<Shared>, seen: &std::sync::atomic::AtomicU64, error: &cpal::Error) {
    let count = seen.fetch_add(1, Ordering::Relaxed) + 1;
    if error.kind() == cpal::ErrorKind::DeviceNotAvailable {
        warn!(
            "playback stream error #{count}, the output device is gone and playback \
             stays silent until the daemon restarts: {error}"
        );
        return;
    }
    let idle = shared.try_lock().map(|guard| guard.state == State::Idle);
    if idle == Some(true) {
        debug!("playback stream error #{count} with nothing playing: {error}");
    } else {
        warn!("playback stream error #{count} mid-utterance, audio may have dropped: {error}");
    }
}

/// A live output stream, kept alive by being held.
///
/// `!Send` by construction, exactly as cpal intends: whoever calls
/// [`Playback::open`] owns this and must keep it on that thread until playback
/// should stop. Dropping it closes the device.
pub struct Stream(#[allow(dead_code)] cpal::Stream);

/// The output config to hand [`Playback::open`] for `device`.
///
/// The device's default, unless that is a sample format the callback does not
/// write. cpal 0.18 ranks `I32` and `I24` above `I16` for a device with no
/// `F32`, where 0.17 ranked `I16` first. Left alone, a device the daemon used
/// to open in `I16` would now be refused. Such a device still offers `I16`, so
/// this asks for it at the default's rate and channel count.
///
/// # Errors
/// Returns an error if the device reports no default config, or cannot list
/// its configs when the default is unusable.
pub fn output_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let default = device.default_output_config()?;
    if renders(default.sample_format()) {
        return Ok(default);
    }
    let alternative = i16_alternative(&default, device.supported_output_configs()?);
    Ok(alternative.unwrap_or(default))
}

/// Whether [`Playback::open`] writes `format`.
fn renders(format: cpal::SampleFormat) -> bool {
    matches!(format, cpal::SampleFormat::F32 | cpal::SampleFormat::I16)
}

/// `default`, but in `I16`, if one of `ranges` offers that.
fn i16_alternative(
    default: &cpal::SupportedStreamConfig,
    ranges: impl IntoIterator<Item = cpal::SupportedStreamConfigRange>,
) -> Option<cpal::SupportedStreamConfig> {
    ranges
        .into_iter()
        .filter(|r| r.sample_format() == cpal::SampleFormat::I16)
        .filter(|r| r.channels() == default.channels())
        .find_map(|r| r.try_with_sample_rate(default.sample_rate()))
}

impl Playback {
    /// Build a pipeline for a device format without opening a stream.
    ///
    /// This is the null-sink constructor: everything except the cpal binding
    /// behaves exactly as it does in production, and [`Playback::render`] pulls
    /// samples the way the callback would.
    #[must_use]
    pub fn detached(device: DeviceFormat) -> Self {
        let capacity =
            (device.sample_rate * RING_SECONDS) as usize * device.channels.max(1) as usize;
        let prebuffer_samples = (u64::from(device.sample_rate) * u64::from(PREBUFFER_MS) / 1000)
            as usize
            * device.channels.max(1) as usize;
        Self {
            shared: Arc::new(Mutex::new(Shared {
                ring: Ring::new(capacity.max(1)),
                state: State::Idle,
                prebuffer_samples,
                stats: Stats::default(),
                block: 0,
            })),
            stream_errors: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            device,
            fade: SeamFade::new(DEFAULT_FADE_MS, device),
            producer: Mutex::new(Producer::default()),
        }
    }

    /// Open an output stream on `device` and start it.
    ///
    /// The stream runs continuously, rendering silence while idle. Starting it
    /// once and leaving it running is deliberate: the first sound after a
    /// device wakes costs 1–2 s on a Bluetooth sink, and paying that on every
    /// utterance is exactly the lag the prebuffer is trying to avoid.
    ///
    /// The cost of that choice is that the daemon holds the output device for
    /// as long as it runs, so an xrun on a loaded machine is reported even
    /// though nothing was being spoken. cpal recovers from one itself — its
    /// ALSA loop calls `snd_pcm_prepare` and carries on, reporting a second
    /// error only if *that* fails — so the report is a health signal, not a
    /// broken stream. [`stream_error`] classifies it by what was playing at the
    /// time, because an xrun with an idle ring cannot have dropped a word.
    ///
    /// That recovery needs cpal 0.18. The `PipeWire` ALSA plugin signals an xrun
    /// as `POLLERR` and keeps signalling it until `snd_pcm_prepare`. cpal
    /// 0.17 checked `POLLERR` first, reported it, and polled again. An xrun
    /// that landed while the worker sat in `poll()` (it woke late on a busy
    /// machine) therefore spun that thread flat out: hundreds of thousands of
    /// "`alsa::poll()` returned POLLERR" reports a second, no audio, until the
    /// daemon restarted.
    ///
    /// # Errors
    /// Returns an error if the device's format is unsupported or the stream
    /// cannot be built or started.
    pub fn open(
        device: &cpal::Device,
        config: &cpal::SupportedStreamConfig,
    ) -> Result<(Self, Stream)> {
        let format = DeviceFormat {
            sample_rate: config.sample_rate(),
            channels: config.channels(),
        };
        let playback = Self::detached(format);
        let shared = Arc::clone(&playback.shared);

        let err_fn = {
            let shared = Arc::clone(&playback.shared);
            let seen = Arc::clone(&playback.stream_errors);
            move |e| stream_error(&shared, &seen, &e)
        };
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                config.config(),
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    // `try_lock`, never `lock`: blocking here misses the
                    // device deadline, and a rare silent buffer is a far
                    // better failure than a stall.
                    if let Some(mut guard) = shared.try_lock() {
                        guard.render(out);
                    } else {
                        out.fill(0.0);
                    }
                },
                err_fn,
                None,
            )?,
            cpal::SampleFormat::I16 => {
                // Scratch buffer allocated once, outside the callback.
                let mut scratch: Vec<f32> = Vec::new();
                device.build_output_stream(
                    config.config(),
                    move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        if scratch.len() < out.len() {
                            // Grows only until it has seen the device's largest
                            // buffer, which cpal settles on within a few
                            // callbacks; steady state allocates nothing.
                            scratch.resize(out.len(), 0.0);
                        }
                        let block = &mut scratch[..out.len()];
                        if let Some(mut guard) = shared.try_lock() {
                            guard.render(block);
                        } else {
                            block.fill(0.0);
                        }
                        for (dst, src) in out.iter_mut().zip(block.iter()) {
                            *dst = crate::tts::num_cast::f32_to_i16(
                                src.clamp(-1.0, 1.0) * f32::from(i16::MAX),
                            );
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            other => anyhow::bail!("unsupported output sample format: {other}"),
        };
        stream.play().context("starting playback stream")?;
        debug!(
            "playback stream open at {} Hz, {} channels",
            format.sample_rate, format.channels
        );
        Ok((playback, Stream(stream)))
    }

    /// The device format this pipeline renders to.
    #[must_use]
    pub fn format(&self) -> DeviceFormat {
        self.device
    }

    /// Current state.
    #[must_use]
    pub fn state(&self) -> State {
        self.shared.lock().state
    }

    /// Underrun and contention counters.
    #[must_use]
    pub fn stats(&self) -> Stats {
        self.shared.lock().stats
    }

    /// Interleaved samples currently buffered.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.shared.lock().ring.available()
    }

    /// Where the first sample of the next [`push`](Self::push) will land, in
    /// the interleaved device samples [`Stats::written_samples`] counts.
    ///
    /// Ahead of `written_samples` by the tail `push` holds back for the
    /// closing fade: those samples are queued but not yet in the ring, and
    /// the next chunk's audio is written after them.
    #[must_use]
    pub fn queue_position(&self) -> u64 {
        let held = self.producer.lock().tail.len() as u64;
        self.shared.lock().stats.written_samples + held
    }

    /// Queue one synthesis chunk, waiting for ring space as needed.
    ///
    /// `params` describes the chunk as the backend produced it; the pipeline
    /// resamples and channel-maps to the device. Awaiting on a full ring is the
    /// backpressure that keeps a fast backend from buffering an entire article
    /// ahead of playback.
    ///
    /// `cancelled` is the utterance's own stop flag, and once it is set nothing
    /// more is written — including the rest of a write already waiting for
    /// space. See [`cancel`](Self::cancel) for why the pipeline has to know.
    ///
    /// # Errors
    /// Returns an error if resampling fails.
    pub async fn push(
        &self,
        samples: &[f32],
        params: AudioParams,
        cancelled: &AtomicBool,
    ) -> Result<()> {
        let prepared = mixer::prepare(samples, params, self.device)?;
        if prepared.is_empty() {
            return Ok(());
        }
        // Fades belong to the *utterance*, not to whatever slice of it the
        // backend happened to put in one frame. Fading every push would put a
        // dip every few milliseconds of audio, which is audible as tremolo;
        // the utterance's own two edges are the only discontinuities there are.
        let ready = {
            let mut prod = self.producer.lock();
            // Under the lock `cancel` resets the producer under, so a stopped
            // utterance cannot leave its tail behind for the next one to open
            // with.
            if cancelled.load(Ordering::Relaxed) {
                return Ok(());
            }
            let mut buf = std::mem::take(&mut prod.tail);
            buf.extend_from_slice(&prepared);
            if !prod.started {
                self.fade.fade_in(&mut buf);
                prod.started = true;
            }
            let hold = self.fade.samples().min(buf.len());
            prod.tail = buf.split_off(buf.len() - hold);
            buf
        };
        if !ready.is_empty() {
            self.write_all(&ready, cancelled).await;
        }
        Ok(())
    }

    /// Close the current synthesis chunk: fade out the held tail and write it,
    /// so the next [`push`](Self::push) starts a fresh fade-in.
    ///
    /// A "chunk" is one `POST /v1/synthesize` response, not one call to `push`.
    /// The distinction is the whole reason this method exists: a backend splits
    /// its response into as many frames as it likes, and fading at every frame
    /// would put a dip every few milliseconds — audible as tremolo. Separate
    /// synthesis responses genuinely are discontinuous, because nothing makes
    /// the last sample of one continuous with the first of the next, so those
    /// are the boundaries that need fading. The caller knows which is which;
    /// the pipeline cannot infer it.
    ///
    /// `cancelled` is the utterance's stop flag, as for [`push`](Self::push).
    pub async fn end_chunk(&self, cancelled: &AtomicBool) {
        let tail = {
            let mut prod = self.producer.lock();
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            prod.started = false;
            let mut tail = std::mem::take(&mut prod.tail);
            self.fade.fade_out(&mut tail);
            tail
        };
        if !tail.is_empty() {
            self.write_all(&tail, cancelled).await;
        }
    }

    /// Signal end of utterance: close the final chunk, then let the ring drain
    /// and idle. The stream stays open, so the next utterance does not pay
    /// device wake-up latency again.
    ///
    /// Returns as soon as the last sample is *queued*. The ring holds seconds
    /// of audio the listener has not heard yet, so anything
    /// that reports an utterance over must wait on
    /// [`wait_for_playout`](Self::wait_for_playout) rather than on this.
    ///
    /// `cancelled` is the utterance's stop flag, as for [`push`](Self::push).
    pub async fn finish(&self, cancelled: &AtomicBool) {
        self.end_chunk(cancelled).await;
        let mut guard = self.shared.lock();
        if guard.state != State::Idle {
            // Promote out of prebuffering even if the utterance was shorter
            // than the threshold, or a one-word reply would never be heard.
            guard.state = State::Draining;
        }
    }

    /// Wait until what is queued has actually been played out.
    ///
    /// The gap this closes is the whole reason it exists: a backend faster than
    /// realtime — which is every local GPU backend — finishes synthesizing an
    /// utterance while most of it is still sitting in the ring. Taking
    /// [`finish`](Self::finish) as "done speaking" therefore announces the end
    /// up to a full ring early, and for an utterance shorter than that,
    /// before a single word has been heard.
    ///
    /// Polls rather than being woken by the callback: notifying a waiter from
    /// the audio thread means touching a wait list under a lock, and the whole
    /// design of this module is that the callback does no such thing. Sleeping
    /// for however much audio remains keeps the cost to a couple of wakeups per
    /// utterance.
    ///
    /// Returns `false` if the device stopped consuming for `stall_timeout` with
    /// audio still queued — a disconnected sink, a suspended host. The caller
    /// gets to decide what to do about it, but it does not wait forever: the
    /// utterance slot is what reports the daemon busy, and a stalled device
    /// must not keep it busy for good.
    pub async fn wait_for_playout(&self, stall_timeout: Duration) -> bool {
        let format = self.device;
        let per_ms = u64::from(format.sample_rate) * u64::from(format.channels.max(1)) / 1000;
        if per_ms == 0 {
            // Under a kilohertz of interleaved samples is not a device anyone
            // is listening to. Nothing to wait for, and no rate to wait at.
            return true;
        }
        let mut last_rendered = u64::MAX;
        let mut stalled = Duration::ZERO;
        loop {
            let (state, buffered, rendered, block) = {
                let guard = self.shared.lock();
                (
                    guard.state,
                    guard.ring.available(),
                    guard.stats.rendered_samples,
                    guard.block,
                )
            };
            if state == State::Idle {
                // The ring is empty, but the block the callback last took is
                // still on its way through the device. One block is an
                // approximation of that — hosts buffer more than one period —
                // and it is the part of the delay this side can actually see.
                if block > 0 {
                    let residual = (block as u64 / per_ms).max(1);
                    tokio::time::sleep(Duration::from_millis(residual)).await;
                }
                return true;
            }

            if rendered == last_rendered {
                if stalled >= stall_timeout {
                    return false;
                }
            } else {
                last_rendered = rendered;
                stalled = Duration::ZERO;
            }

            // Sleep for what is left to play. Overshooting costs nothing —
            // the next look finds the ring idle — and undershooting just
            // means another cheap poll.
            let nap = Duration::from_millis((buffered as u64 / per_ms).max(DRAIN_POLL_MS));
            stalled += nap;
            tokio::time::sleep(nap).await;
        }
    }

    /// Stop immediately: discard buffered audio and the held tail.
    ///
    /// This is the barge-in path, so it is measured in how fast it goes quiet —
    /// dropping the ring means the next callback renders silence, bounded by
    /// one device buffer rather than by whatever was queued.
    ///
    /// **Set the utterance's stop flag first.** Clearing the ring does not stop
    /// whoever is filling it: a producer ahead of the speakers has a write
    /// waiting on a full ring, and it refills the ring the moment this empties
    /// it — so a stop would skip a ring's worth of speech and carry on. The
    /// producer methods check the flag under the same locks this takes, which
    /// is what guarantees that nothing written after the clear survives it.
    pub fn cancel(&self) {
        *self.producer.lock() = Producer::default();
        let mut guard = self.shared.lock();
        // The position counters are per-utterance, so a cancel resets them
        // along with the audio they were counting.
        guard.stats.rendered_samples = 0;
        guard.stats.written_samples = 0;
        guard.ring.clear();
        guard.state = State::Idle;
    }

    /// Render one buffer exactly as the audio callback would.
    ///
    /// The null-sink entry point: tests drive this instead of a device, and it
    /// is the same code path the cpal callback runs.
    pub fn render(&self, out: &mut [f32]) {
        self.shared.lock().render(out);
    }

    /// Write every sample, yielding while the ring is full, until `cancelled`
    /// is set — then drop whatever is left.
    async fn write_all(&self, mut samples: &[f32], cancelled: &AtomicBool) {
        loop {
            {
                let mut guard = self.shared.lock();
                // Checked under the lock `cancel` clears the ring under: a
                // write either lands before the clear and is cleared with it,
                // or comes after and sees the flag the canceller set first.
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let n = guard.ring.write(samples);
                guard.stats.written_samples += n as u64;
                samples = &samples[n..];
                if guard.state == State::Idle {
                    guard.state = State::Prebuffering;
                }
                guard.maybe_start();
                if samples.is_empty() {
                    return;
                }
            }
            // The ring is full, which means the consumer is the bottleneck —
            // exactly the situation backpressure is for. Sleep for a fraction
            // of the buffer rather than spinning.
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

#[cfg(test)]
mod tests;
