// SPDX-License-Identifier: GPL-3.0-only
//! Microphone capture for a cloned-voice reference clip.
//!
//! **The stream lives on its own thread.** cpal's `Stream` is `!Send` on
//! several backends, so it cannot sit in the app model beside everything the
//! UI touches. The thread opens the device, captures until it is told to stop
//! (or until the cap is reached), and drops the stream on its way out; the UI
//! sees only an `Arc<Mutex<…>>` of samples and a peak level.
//!
//! **Opening the device never blocks the UI.** `start` returns immediately and
//! the thread reports failure through the same shared state the level meter
//! reads, so a missing microphone surfaces on the next tick as a message on
//! the page rather than as a frozen window.
//!
//! Capture is mono at whatever rate the device offers. The daemon does the
//! downmix-and-resample to its canonical shape, so there is nothing to match
//! here — sending the device's own rate is the one conversion that cannot be
//! wrong.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Shared between the capture thread and the UI.
#[derive(Debug, Default)]
struct Capture {
    /// Mono samples captured so far.
    samples: Vec<f32>,
    /// The device's rate, once the stream is open.
    sample_rate: u32,
    /// Peak magnitude of the most recent block, for the level meter.
    peak: f32,
    /// Why capture stopped early, if it did.
    error: Option<String>,
    /// Set when the thread has stopped on its own — the cap was reached, or
    /// the device failed.
    finished: bool,
}

/// What the UI needs to render mid-recording.
#[derive(Debug, Clone, Copy, Default)]
pub struct Status {
    /// Seconds captured so far.
    pub seconds: f32,
    /// Peak level of the last block, `0.0`–`1.0`.
    pub level: f32,
    /// The capture ended by itself (hit the cap, or the device failed).
    pub finished: bool,
}

/// A recording in progress.
pub struct Recorder {
    shared: Arc<Mutex<Capture>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    max_seconds: f32,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder")
            .field("max_seconds", &self.max_seconds)
            .finish_non_exhaustive()
    }
}

/// A finished recording, ready to be wrapped in a WAV and uploaded.
#[derive(Debug, Clone)]
pub struct Recording {
    /// Mono samples at [`Self::sample_rate`].
    pub samples: Vec<f32>,
    /// The rate the device captured at.
    pub sample_rate: u32,
}

impl Recording {
    /// Length in seconds.
    #[must_use]
    pub fn seconds(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let (frames, rate) = (self.samples.len() as f32, self.sample_rate as f32);
        frames / rate
    }

    /// Wrap the samples in a 16-bit WAV file.
    ///
    /// # Errors
    /// Returns a message if the samples cannot be encoded, which for an
    /// in-memory buffer means only a malformed spec.
    pub fn to_wav(&self) -> Result<Vec<u8>, String> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec)
                .map_err(|e| format!("could not start a WAV file: {e}"))?;
            for &s in &self.samples {
                writer
                    .write_sample(to_i16(s))
                    .map_err(|e| format!("could not write the recording: {e}"))?;
            }
            writer
                .finalize()
                .map_err(|e| format!("could not finish the WAV file: {e}"))?;
        }
        Ok(cursor.into_inner())
    }
}

/// Clamp before the cast: a sample that arrives slightly hot would otherwise
/// wrap to the opposite polarity, which is a click rather than a clip.
#[allow(clippy::cast_possible_truncation)]
fn to_i16(sample: f32) -> i16 {
    (sample * f32::from(i16::MAX)).clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

impl Recorder {
    /// Open the default input device and start capturing, stopping by itself
    /// after `max_seconds`.
    ///
    /// Returns immediately: the device is opened on the capture thread, and a
    /// failure to open it shows up as [`Recorder::take_error`] on a later tick.
    #[must_use]
    pub fn start(max_seconds: f32) -> Self {
        let shared = Arc::new(Mutex::new(Capture::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("voice-recorder".into())
            .spawn({
                let shared = Arc::clone(&shared);
                let stop = Arc::clone(&stop);
                move || capture(&shared, &stop, max_seconds)
            })
            .ok();
        if thread.is_none()
            && let Ok(mut guard) = shared.lock()
        {
            guard.error = Some("could not start the recording thread".into());
            guard.finished = true;
        }
        Self {
            shared,
            stop,
            thread,
            max_seconds,
        }
    }

    /// The cap this recording stops itself at.
    #[must_use]
    pub fn max_seconds(&self) -> f32 {
        self.max_seconds
    }

    /// A snapshot for the level meter and elapsed readout.
    #[must_use]
    pub fn status(&self) -> Status {
        let Ok(guard) = self.shared.lock() else {
            return Status {
                finished: true,
                ..Status::default()
            };
        };
        Status {
            seconds: seconds_of(&guard),
            level: guard.peak,
            finished: guard.finished,
        }
    }

    /// Stop capturing and take what was recorded.
    ///
    /// # Errors
    /// Returns the capture thread's message when the device failed, or when
    /// nothing was captured at all — an empty clip would be refused by the
    /// daemon anyway, and "no audio" is the more useful thing to say.
    pub fn finish(mut self) -> Result<Recording, String> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            // A capture thread that has wedged must not take the UI with it.
            // Its stream is dropped either way when the process exits.
            let _ = handle.join();
        }
        let mut guard = self
            .shared
            .lock()
            .map_err(|_| "the recording was lost".to_string())?;
        if let Some(e) = guard.error.take() {
            return Err(e);
        }
        if guard.samples.is_empty() {
            return Err("nothing was recorded — is the microphone muted?".into());
        }
        Ok(Recording {
            samples: std::mem::take(&mut guard.samples),
            sample_rate: guard.sample_rate,
        })
    }
}

impl Drop for Recorder {
    /// A recorder dropped without `finish` — the page was left, or the window
    /// closed — must still release the microphone.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Seconds captured, from the sample count and the device rate.
fn seconds_of(capture: &Capture) -> f32 {
    if capture.sample_rate == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let (frames, rate) = (capture.samples.len() as f32, capture.sample_rate as f32);
    frames / rate
}

/// The capture thread: open the device, run until told to stop, drop the
/// stream.
fn capture(shared: &Arc<Mutex<Capture>>, stop: &Arc<AtomicBool>, max_seconds: f32) {
    let stream = match open(shared, max_seconds) {
        Ok(s) => s,
        Err(e) => {
            if let Ok(mut guard) = shared.lock() {
                guard.error = Some(e);
                guard.finished = true;
            }
            return;
        }
    };
    if let Err(e) = stream.play() {
        if let Ok(mut guard) = shared.lock() {
            guard.error = Some(format!("could not start the microphone: {e}"));
            guard.finished = true;
        }
        return;
    }

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let Ok(guard) = shared.lock() else { break };
        if guard.finished {
            break;
        }
    }
    // Dropping the stream closes the device. Explicit so the ordering against
    // the flag below is not left to the reader.
    drop(stream);
    if let Ok(mut guard) = shared.lock() {
        guard.peak = 0.0;
    }
}

/// Build the input stream for the default device.
fn open(shared: &Arc<Mutex<Capture>>, max_seconds: f32) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "no microphone was found".to_string())?;
    let supported = input_config(&device)
        .map_err(|e| format!("the microphone offered no usable format: {e}"))?;
    let config = supported.config();
    let channels = usize::from(config.channels).max(1);
    let sample_rate = config.sample_rate;
    if let Ok(mut guard) = shared.lock() {
        guard.sample_rate = sample_rate;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let cap = (max_seconds * sample_rate as f32) as usize;

    let err_shared = Arc::clone(shared);
    let err_fn = move |e: cpal::Error| {
        if let Ok(mut guard) = err_shared.lock() {
            guard.error = Some(format!("the microphone stopped: {e}"));
            guard.finished = true;
        }
    };

    // Only the two formats the daemon's playback path handles. A device
    // offering something else is rare enough that naming it beats a silent
    // conversion nobody tested.
    match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            let shared = Arc::clone(shared);
            device.build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    accumulate(&shared, data, channels, cap, |s| s);
                },
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let shared = Arc::clone(shared);
            device.build_input_stream(
                config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    accumulate(&shared, data, channels, cap, |s| {
                        f32::from(s) / f32::from(i16::MAX)
                    });
                },
                err_fn,
                None,
            )
        }
        other => return Err(format!("unsupported microphone sample format: {other}")),
    }
    .map_err(|e| format!("could not open the microphone: {e}"))
}

/// The microphone's default config, unless that is a format [`open`] does not
/// read.
///
/// cpal 0.18 ranks `I32` and `I24` above `I16` for a device with no `F32`,
/// where 0.17 ranked `I16` first. Such a device still offers `I16`, so this
/// asks for it at the default's rate and channel count rather than refusing a
/// microphone that used to work.
fn input_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig, cpal::Error> {
    let default = device.default_input_config()?;
    if matches!(
        default.sample_format(),
        cpal::SampleFormat::F32 | cpal::SampleFormat::I16
    ) {
        return Ok(default);
    }
    let i16 = device
        .supported_input_configs()?
        .filter(|r| r.sample_format() == cpal::SampleFormat::I16)
        .filter(|r| r.channels() == default.channels())
        .find_map(|r| r.try_with_sample_rate(default.sample_rate()));
    Ok(i16.unwrap_or(default))
}

/// Downmix one callback block to mono, append it, and record its peak.
///
/// Averaging rather than taking the first channel: a two-microphone array
/// carries the same voice twice, and dropping half of it throws away
/// signal-to-noise for nothing.
fn accumulate<T: Copy>(
    shared: &Arc<Mutex<Capture>>,
    data: &[T],
    channels: usize,
    cap: usize,
    to_f32: impl Fn(T) -> f32,
) {
    // `try_lock`, never `lock`: this runs on the device's callback thread and
    // blocking it drops audio. The UI holds the lock only to copy two numbers,
    // so a contended block is both rare and harmless to skip.
    let Ok(mut guard) = shared.try_lock() else {
        return;
    };
    if guard.finished {
        return;
    }
    #[allow(clippy::cast_precision_loss)]
    let scale = channels as f32;
    let mut peak = 0.0_f32;
    for frame in data.chunks(channels) {
        let sample = frame.iter().map(|&s| to_f32(s)).sum::<f32>() / scale;
        peak = peak.max(sample.abs());
        guard.samples.push(sample);
    }
    guard.peak = peak.min(1.0);
    if cap > 0 && guard.samples.len() >= cap {
        // The cap is the archive bound; stopping here rather than letting the
        // upload be refused means the user keeps what they recorded.
        guard.samples.truncate(cap);
        guard.finished = true;
    }
}

#[cfg(test)]
mod tests {
    use super::{Recording, to_i16};

    #[test]
    fn a_recording_reports_its_length() {
        let r = Recording {
            samples: vec![0.0; 48_000],
            sample_rate: 48_000,
        };
        assert!((r.seconds() - 1.0).abs() < 1e-6, "{}", r.seconds());
    }

    /// A rate of zero can only come from a device that never opened; the
    /// length readout must not divide by it.
    #[test]
    fn an_unopened_recording_is_zero_seconds() {
        let r = Recording {
            samples: vec![0.0; 10],
            sample_rate: 0,
        };
        assert!((r.seconds() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn encodes_a_playable_wav() {
        let r = Recording {
            samples: vec![0.5, -0.5, 0.25],
            sample_rate: 24_000,
        };
        let wav = r.to_wav().expect("encodes");
        assert_eq!(&wav[..4], b"RIFF");
        let read = hound::WavReader::new(std::io::Cursor::new(&wav)).expect("reads back");
        assert_eq!(read.spec().channels, 1);
        assert_eq!(read.spec().sample_rate, 24_000);
        assert_eq!(read.len(), 3);
    }

    /// Hot samples clamp to full scale instead of wrapping, which is the
    /// difference between a loud recording and a broken one.
    #[test]
    fn clamps_rather_than_wrapping() {
        assert_eq!(to_i16(2.0), i16::MAX);
        assert_eq!(to_i16(-2.0), i16::MIN);
        assert_eq!(to_i16(0.0), 0);
    }
}
