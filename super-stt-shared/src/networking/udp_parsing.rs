use log::error;

// SPDX-License-Identifier: GPL-3.0-only
use crate::{
    daemon_state::RecordingStateData,
    models::audio::{AudioSamplesData, FrequencyBandsData},
    udp::{AUDIO_SAMPLES_PACKET, FREQUENCY_BANDS_PACKET, RECORDING_STATE_PACKET},
};

const MAX_SAMPLES: u32 = 192_000; // ~4 seconds at 48kHz (reasonable limit)
const MAX_PACKET_SIZE: usize = 8192; // Maximum UDP packet size

/// Parse an audio samples UDP packet into `AudioSamplesData`.
///
/// # Errors
///
/// Returns an error if the buffer is too short, the packet type is wrong,
/// or the payload does not contain enough bytes for the declared sample count.
pub fn parse_audio_samples_from_udp(data: &[u8]) -> Result<AudioSamplesData, String> {
    // Parse UDP packet for audio samples
    // Packet structure: Header (11 bytes) + sample_rate (4) + channels (2) + num_samples (4) + samples (4*n)
    let data_len = data.len();

    if data_len < 21 {
        error!("Packet too short: {data_len} bytes (need at least 21)");
        return Err("Packet too short for audio samples".to_string());
    }

    // Parse header
    let packet_type = data[0];
    if packet_type != AUDIO_SAMPLES_PACKET {
        return Err("Not an audio samples packet".to_string());
    }

    // Skip header (11 bytes) and parse audio samples data
    let audio_data = &data[11..];
    let audio_len = audio_data.len();
    if audio_len < 10 {
        error!("Audio data too short: {audio_len} bytes (need at least 10)");
        return Err("Audio samples data too short".to_string());
    }

    // Parse sample rate (4 bytes)
    let sample_rate_bytes = [audio_data[0], audio_data[1], audio_data[2], audio_data[3]];
    let sample_rate = f32::from_le_bytes(sample_rate_bytes);

    // Parse channels (2 bytes)
    let channels_bytes = [audio_data[4], audio_data[5]];
    let channels = u16::from_le_bytes(channels_bytes);

    // Parse number of samples (4 bytes)
    let num_samples_bytes = [audio_data[6], audio_data[7], audio_data[8], audio_data[9]];
    let num_samples = u32::from_le_bytes(num_samples_bytes);

    // Security: Validate num_samples to prevent memory exhaustion attacks

    if num_samples > MAX_SAMPLES {
        return Err(format!(
            "Sample count {num_samples} exceeds maximum {MAX_SAMPLES}"
        ));
    }

    // Parse samples (4 bytes each)
    let samples_start = 10;
    let expected_data_len = samples_start + (num_samples as usize * 4);

    // Additional security: Check total packet size
    if audio_len > MAX_PACKET_SIZE {
        error!(
            "ERROR: Packet size {audio_len} exceeds maximum {MAX_PACKET_SIZE} - potential DoS attack",
        );
        return Err(format!(
            "Packet size {audio_len} exceeds maximum {MAX_PACKET_SIZE}",
        ));
    }

    if audio_len < expected_data_len {
        error!(
            "ERROR: Insufficient data for audio samples - expected {expected_data_len} bytes, got {audio_len} bytes",
        );
        return Err("Insufficient data for audio samples".to_string());
    }

    // Safe allocation with validated bounds
    let mut samples = Vec::with_capacity(num_samples.min(MAX_SAMPLES) as usize);
    for i in 0..num_samples {
        let offset = samples_start + (i as usize * 4);
        let sample_bytes = [
            audio_data[offset],
            audio_data[offset + 1],
            audio_data[offset + 2],
            audio_data[offset + 3],
        ];
        let sample = f32::from_le_bytes(sample_bytes);
        samples.push(sample);
    }

    Ok(AudioSamplesData {
        samples,
        sample_rate,
        channels,
    })
}

/// Parse a recording state UDP packet into `RecordingStateData`.
///
/// # Errors
///
/// Returns an error if the buffer is too short or the packet type is wrong.
pub fn parse_recording_state_from_udp(data: &[u8]) -> Result<RecordingStateData, String> {
    // Parse UDP packet following daemon's protocol
    // Packet structure: Header (11 bytes) + RecordingStateData (9 bytes) = 20 bytes
    if data.len() < 20 {
        return Err("Packet too short".to_string());
    }

    // Parse header
    let packet_type = data[0];
    if packet_type != RECORDING_STATE_PACKET {
        return Err("Not a recording state packet".to_string());
    }

    // Skip header (11 bytes) and parse recording state data
    let state_data = &data[11..];
    if state_data.len() < 9 {
        return Err("Recording state data too short".to_string());
    }

    let is_recording = state_data[0] != 0;
    let timestamp_bytes = [
        state_data[1],
        state_data[2],
        state_data[3],
        state_data[4],
        state_data[5],
        state_data[6],
        state_data[7],
        state_data[8],
    ];
    let timestamp_ms = u64::from_le_bytes(timestamp_bytes);

    Ok(RecordingStateData {
        is_recording,
        timestamp_ms,
    })
}

/// Parse a frequency bands UDP packet into `FrequencyBandsData`.
///
/// # Errors
///
/// Returns an error if the buffer is too short, the packet type is wrong,
/// or the payload does not contain enough bytes for the declared band count.
pub fn parse_frequency_bands_from_udp(data: &[u8]) -> Result<FrequencyBandsData, String> {
    // Parse UDP packet for frequency bands
    // Packet structure: Header (11 bytes) + sample_rate (4) + total_energy (4) + num_bands (4) + bands (4*n)
    if data.len() < 23 {
        // 11 header + 4 + 4 + 4 = 23 minimum
        return Err("Packet too short for frequency bands".to_string());
    }

    // Parse header
    let packet_type = data[0];
    if packet_type != FREQUENCY_BANDS_PACKET {
        return Err("Not a frequency bands packet".to_string());
    }

    // Skip header (11 bytes) and parse frequency bands data
    let bands_data = &data[11..];
    if bands_data.len() < 12 {
        return Err("Frequency bands data too short".to_string());
    }

    // Parse sample rate (4 bytes)
    let sample_rate_bytes = [bands_data[0], bands_data[1], bands_data[2], bands_data[3]];
    let sample_rate = f32::from_le_bytes(sample_rate_bytes);

    // Parse total energy (4 bytes)
    let total_energy_bytes = [bands_data[4], bands_data[5], bands_data[6], bands_data[7]];
    let total_energy = f32::from_le_bytes(total_energy_bytes);

    // Parse number of bands (4 bytes)
    let num_bands_bytes = [bands_data[8], bands_data[9], bands_data[10], bands_data[11]];
    let num_bands = u32::from_le_bytes(num_bands_bytes);

    // Parse band data (4 bytes each)
    let bands_start = 12;
    let expected_data_len = bands_start + (num_bands as usize * 4);
    if bands_data.len() < expected_data_len {
        return Err("Insufficient data for frequency bands".to_string());
    }

    let mut bands = Vec::with_capacity(num_bands as usize);
    for i in 0..num_bands {
        let offset = bands_start + (i as usize * 4);
        let band_bytes = [
            bands_data[offset],
            bands_data[offset + 1],
            bands_data[offset + 2],
            bands_data[offset + 3],
        ];
        let band = f32::from_le_bytes(band_bytes);
        bands.push(band);
    }

    Ok(FrequencyBandsData {
        bands,
        sample_rate,
        total_energy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_audio_samples_memory_exhaustion_protection() {
        // Test protection against excessive sample count
        let mut malicious_packet = vec![0u8; 100];

        // Set packet type to audio samples
        malicious_packet[0] = AUDIO_SAMPLES_PACKET;

        // Set malicious sample count (exceeds MAX_SAMPLES = 192,000)
        // Sample count is at audio_data[6-9], where audio_data starts at offset 11
        // So we need to set bytes at positions 17-20 in the full packet
        let malicious_count: u32 = 500_000; // Way over the limit
        let count_bytes = malicious_count.to_le_bytes();
        malicious_packet[17] = count_bytes[0]; // 11 + 6
        malicious_packet[18] = count_bytes[1]; // 11 + 7
        malicious_packet[19] = count_bytes[2]; // 11 + 8
        malicious_packet[20] = count_bytes[3]; // 11 + 9

        // Attempt to parse should fail with security error
        let result = parse_audio_samples_from_udp(&malicious_packet);
        assert!(result.is_err());
        let error_msg = result.unwrap_err();
        assert!(
            error_msg.contains("exceeds maximum"),
            "Expected error about exceeding maximum, got: {}",
            error_msg
        );
    }

    #[test]
    fn test_parse_audio_samples_packet_size_protection() {
        // Test protection against oversized packets
        let mut large_packet = vec![0u8; 10_000]; // Way over MAX_PACKET_SIZE = 8192

        // Set packet type to audio samples
        large_packet[0] = AUDIO_SAMPLES_PACKET;

        // Set reasonable sample count
        let sample_count: u32 = 1000;
        let count_bytes = sample_count.to_le_bytes();
        large_packet[6] = count_bytes[0];
        large_packet[7] = count_bytes[1];
        large_packet[8] = count_bytes[2];
        large_packet[9] = count_bytes[3];

        // Attempt to parse should fail with packet size error
        let result = parse_audio_samples_from_udp(&large_packet);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Packet size"));
    }

    #[test]
    fn test_parse_audio_samples_reasonable_limits() {
        // Test that reasonable values don't trigger security errors
        let sample_count: u32 = 1000; // Well under MAX_SAMPLES = 192,000
        let packet_size = 30 + (sample_count as usize * 4); // Large enough for all headers
        let mut packet = vec![0u8; packet_size];

        // Set packet type
        packet[0] = AUDIO_SAMPLES_PACKET;

        // Set sample count at the correct position (11 + 6 = 17)
        let count_bytes = sample_count.to_le_bytes();
        packet[17] = count_bytes[0];
        packet[18] = count_bytes[1];
        packet[19] = count_bytes[2];
        packet[20] = count_bytes[3];

        // This should NOT trigger security errors (though it may fail for format reasons)
        let result = parse_audio_samples_from_udp(&packet);
        // The main test is that we don't get security-related errors
        if let Err(error_msg) = result {
            assert!(
                !error_msg.contains("exceeds maximum") && !error_msg.contains("Packet size"),
                "Should not get security errors for reasonable values, got: {}",
                error_msg
            );
        }
    }
}
