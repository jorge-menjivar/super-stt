// SPDX-License-Identifier: GPL-3.0-only

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SupportedStreamConfig};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct AudioDeviceCache {
    pub output_device: Device,
    pub output_config: SupportedStreamConfig,
    pub last_verified: Instant,
    pub initialization_verified: bool,
}

impl std::fmt::Debug for AudioDeviceCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioDeviceCache")
            .field("output_config", &self.output_config)
            .field("last_verified", &self.last_verified)
            .field("initialization_verified", &self.initialization_verified)
            .field(
                "device_name",
                &self
                    .output_device
                    .name()
                    .unwrap_or_else(|_| "Unknown".to_string()),
            )
            .finish()
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub struct AudioHealthStatus {
    pub overall_healthy: bool,
    pub output_device_healthy: bool,
    pub input_device_healthy: bool,
    pub audio_permissions_ok: bool,
    pub output_device_info: AudioDeviceInfo,
    pub input_device_info: AudioDeviceInfo,
    pub output_device_error: Option<String>,
    pub input_device_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: String,
    pub buffer_size: String,
}

impl Default for AudioHealthStatus {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioHealthStatus {
    #[must_use]
    pub fn new() -> Self {
        Self {
            overall_healthy: false,
            output_device_healthy: false,
            input_device_healthy: false,
            audio_permissions_ok: false,
            output_device_info: AudioDeviceInfo::default(),
            input_device_info: AudioDeviceInfo::default(),
            output_device_error: None,
            input_device_error: None,
        }
    }
}

impl AudioDeviceInfo {
    fn default() -> Self {
        Self {
            name: "Unknown".to_string(),
            sample_rate: 0,
            channels: 0,
            sample_format: "Unknown".to_string(),
            buffer_size: "Unknown".to_string(),
        }
    }
}

const DEVICE_CACHE_VALIDITY: Duration = Duration::from_secs(30);
const DRIVER_INIT_TIMEOUT: Duration = Duration::from_millis(500);
const DEVICE_VERIFICATION_ATTEMPTS: usize = 3;

/// Get the cached output audio device or initialize a new one.
///
/// # Errors
///
/// Returns an error if there is no default output device or the device
/// configuration cannot be retrieved.
/// Get the cached output audio device or initialize a new one.
///
/// # Panics
///
/// Panics if the internal device cache mutex is poisoned.
///
/// # Errors
///
/// Returns an error if there is no default output device or the device
/// configuration cannot be retrieved.
pub fn get_or_initialize_audio_device(
    audio_device_cache: &Arc<Mutex<Option<AudioDeviceCache>>>,
) -> Result<AudioDeviceCache> {
    let mut cache = match audio_device_cache.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            log::warn!("Audio device cache lock was poisoned, attempting recovery");
            poisoned.into_inner()
        }
    };
    if let Some(ref cached) = *cache
        && cached.last_verified.elapsed() < DEVICE_CACHE_VALIDITY
        && cached.initialization_verified
    {
        log::debug!("Using cached audio device");
        return Ok(cached.clone());
    }

    log::info!("Initializing audio output device...");
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("No output device available"))?;

    let config = device
        .default_output_config()
        .map_err(|e| anyhow::anyhow!("Failed to get output config: {e}"))?;

    log::info!(
        "Audio device initialized: {}Hz, {} channels, {:?}",
        config.sample_rate().0,
        config.channels(),
        config.sample_format()
    );

    let new_cache = AudioDeviceCache {
        output_device: device,
        output_config: config,
        last_verified: Instant::now(),
        initialization_verified: false,
    };
    *cache = Some(new_cache.clone());
    Ok(new_cache)
}

/// Verify output device readiness by opening a short test stream.
///
/// # Errors
///
/// Returns an error if the device cannot open a stream or does not
/// complete the verification within the timeout.
pub fn verify_audio_device_readiness(
    audio_device_cache: &Arc<Mutex<Option<AudioDeviceCache>>>,
    device_cache: &AudioDeviceCache,
) -> Result<()> {
    for attempt in 1..=DEVICE_VERIFICATION_ATTEMPTS {
        match attempt_device_verification(device_cache, attempt) {
            Ok(()) => {
                log::debug!("Audio device verification successful on attempt {attempt}");
                if let Ok(mut cache) = audio_device_cache.lock()
                    && let Some(ref mut cached) = cache.as_mut()
                {
                    cached.initialization_verified = true;
                    cached.last_verified = Instant::now();
                }

                return Ok(());
            }
            Err(e) => {
                log::warn!("Verification attempt {attempt} failed: {e}");
                if attempt < DEVICE_VERIFICATION_ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(50 * attempt as u64));
                }
            }
        }
    }
    Err(anyhow::anyhow!(
        "Failed to verify audio device readiness after {DEVICE_VERIFICATION_ATTEMPTS} attempts"
    ))
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn attempt_device_verification(device_cache: &AudioDeviceCache, _attempt: usize) -> Result<()> {
    let sample_rate = device_cache.output_config.sample_rate().0 as f32;
    let channels = device_cache.output_config.channels() as usize;
    let verification_duration_ms = 50u64;
    let samples_needed = (sample_rate * verification_duration_ms as f32 / 1000.0) as usize;
    let mut sample_count = 0usize;
    let completed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let completed_clone = completed.clone();

    let stream = match device_cache.output_config.sample_format() {
        cpal::SampleFormat::F32 => device_cache.output_device.build_output_stream(
            &device_cache.output_config.config(),
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels) {
                    if sample_count >= samples_needed {
                        completed_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    for sample in frame {
                        *sample = 0.0;
                    }
                    sample_count += 1;
                }
            },
            |err| log::warn!("Device verification stream error: {err}"),
            None,
        )?,
        cpal::SampleFormat::I16 => device_cache.output_device.build_output_stream(
            &device_cache.output_config.config(),
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels) {
                    if sample_count >= samples_needed {
                        completed_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    for sample in frame {
                        *sample = 0;
                    }
                    sample_count += 1;
                }
            },
            |err| log::warn!("Device verification stream error: {err}"),
            None,
        )?,
        _ => {
            return Err(anyhow::anyhow!("Unsupported sample format"));
        }
    };

    stream.play()?;
    let start_time = Instant::now();
    while !completed.load(std::sync::atomic::Ordering::Relaxed) {
        if start_time.elapsed() > DRIVER_INIT_TIMEOUT {
            return Err(anyhow::anyhow!("Device verification timed out"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(stream);
    std::thread::sleep(Duration::from_millis(20));
    Ok(())
}

/// Check health and details of the output audio device.
///
/// # Panics
///
/// Panics if the device cache mutex is poisoned.
///
/// # Errors
///
/// Returns an error if no default output device is available or
/// if querying configuration fails.
pub fn check_output_device_health(
    audio_device_cache: &Arc<Mutex<Option<AudioDeviceCache>>>,
) -> Result<AudioDeviceInfo> {
    let device_result = {
        let cache_guard = match audio_device_cache.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                log::warn!(
                    "Audio device cache lock was poisoned during device check, attempting recovery"
                );
                poisoned.into_inner()
            }
        };
        if let Some(ref cached) = *cache_guard {
            if cached.last_verified.elapsed() < DEVICE_CACHE_VALIDITY {
                Ok((cached.output_device.clone(), cached.output_config.clone()))
            } else {
                drop(cache_guard);
                Err(anyhow::anyhow!("Cached device expired"))
            }
        } else {
            drop(cache_guard);
            Err(anyhow::anyhow!("No cached device"))
        }
    };

    let (device, config) = if let Ok(cached) = device_result {
        cached
    } else {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("No output device available"))?;
        let config = device.default_output_config()?;
        (device, config)
    };

    verify_audio_device_readiness(
        audio_device_cache,
        &AudioDeviceCache {
            output_device: device.clone(),
            output_config: config.clone(),
            last_verified: Instant::now(),
            initialization_verified: false,
        },
    )?;

    Ok(AudioDeviceInfo {
        name: device
            .name()
            .unwrap_or_else(|_| "Unknown Device".to_string()),
        sample_rate: config.sample_rate().0,
        channels: config.channels(),
        sample_format: format!("{:?}", config.sample_format()),
        buffer_size: format!("{:?}", config.buffer_size()),
    })
}

/// Check health and details of the input audio device.
///
/// # Errors
///
/// Returns an error if there is no default input device or
/// if querying supported configurations fails.
pub fn check_input_device_health() -> Result<AudioDeviceInfo> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("No input device available"))?;

    let supported_configs: Vec<_> = device.supported_input_configs()?.collect();
    if supported_configs.is_empty() {
        return Err(anyhow::anyhow!(
            "Input device has no supported configurations"
        ));
    }

    let config = device.default_input_config()?;

    let _test_stream = device.build_input_stream(
        &config.config(),
        move |_data: &[f32], _: &cpal::InputCallbackInfo| {},
        |err| log::debug!("Input verification stream error: {err}"),
        None,
    )?;

    Ok(AudioDeviceInfo {
        name: device
            .name()
            .unwrap_or_else(|_| "Unknown Device".to_string()),
        sample_rate: config.sample_rate().0,
        channels: config.channels(),
        sample_format: format!("{:?}", config.sample_format()),
        buffer_size: format!("{:?}", config.buffer_size()),
    })
}

#[must_use]
pub fn check_audio_permissions() -> bool {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(mut devices) => {
            if devices.next().is_some() {
                match host.output_devices() {
                    Ok(mut out_devices) => out_devices.next().is_some(),
                    Err(_) => false,
                }
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

/// Perform a comprehensive audio system health check and return status.
///
/// # Errors
///
/// Returns an error if any of the device checks fail in a fatal way
/// (nonfatal errors are captured in the health status but critical
/// errors propagate).
pub fn perform_audio_health_check(
    audio_device_cache: &Arc<Mutex<Option<AudioDeviceCache>>>,
) -> Result<AudioHealthStatus> {
    log::info!("Performing comprehensive audio system health check...");
    let mut health_status = AudioHealthStatus::new();

    match check_output_device_health(audio_device_cache) {
        Ok(output_status) => {
            health_status.output_device_healthy = true;
            health_status.output_device_info = output_status;
            log::info!("Output device health check: PASSED");
        }
        Err(e) => {
            health_status.output_device_healthy = false;
            health_status.output_device_error = Some(e.to_string());
            log::warn!("Output device health check: FAILED - {e}");
        }
    }

    match check_input_device_health() {
        Ok(input_status) => {
            health_status.input_device_healthy = true;
            health_status.input_device_info = input_status;
            log::info!("Input device health check: PASSED");
        }
        Err(e) => {
            health_status.input_device_healthy = false;
            health_status.input_device_error = Some(e.to_string());
            log::warn!("Input device health check: FAILED - {e}");
        }
    }

    health_status.audio_permissions_ok = check_audio_permissions();
    health_status.overall_healthy = health_status.output_device_healthy
        && health_status.input_device_healthy
        && health_status.audio_permissions_ok;

    if health_status.overall_healthy {
        log::info!("Audio system health check: ALL SYSTEMS HEALTHY");
    } else {
        log::warn!("Audio system health check: ISSUES DETECTED");
    }

    Ok(health_status)
}
