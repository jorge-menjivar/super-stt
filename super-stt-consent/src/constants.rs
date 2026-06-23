// SPDX-License-Identifier: GPL-3.0-only
//! Static content shown in the consent popup.
//!
//! Each `*_PERMISSIONS` array is the bullet list for one scope. A token
//! can carry several scopes, so the popup renders the union of these for
//! every scope the requesting app asked for. Edit the strings here to
//! change what the user is told they're approving — keep each entry
//! concise (≤ one wrapped line on a typical screen) and user-meaningful.
//! These are what the user reads in the dialog, not a developer reference.

/// Bullets for the `transcribe` scope.
pub const TRANSCRIBE_PERMISSIONS: &[&str] = &[
    "Use your microphone to record speech for this app",
    "Receive this app's own transcription text (preview and final)",
];

/// Bullets for the `status` scope.
pub const STATUS_PERMISSIONS: &[&str] =
    &["Read which speech-to-text model and device are currently active"];

/// Bullets for the `settings` scope.
pub const SETTINGS_PERMISSIONS: &[&str] = &[
    "Read and change every daemon setting (model, device, audio cues, volume, recording behavior)",
    "Allow or block sending audio to online providers (OpenAI, Mistral, Deepgram)",
    "Install, update, and remove speech-to-text backends",
];

/// Bullets for the `recording_events` scope.
pub const RECORDING_EVENTS_PERMISSIONS: &[&str] =
    &["See when any recording starts and stops on this device"];

/// Bullets for the `audio_visualization` scope.
pub const AUDIO_VISUALIZATION_PERMISSIONS: &[&str] =
    &["Receive audio visualization data (frequency bars) while a recording is running"];

/// Bullets for the `global_transcriptions` scope.
pub const GLOBAL_TRANSCRIPTIONS_PERMISSIONS: &[&str] =
    &["Read live and final transcription text from every app on this device"];

/// Bullets for the `daemon_status` scope.
pub const DAEMON_STATUS_PERMISSIONS: &[&str] =
    &["Monitor model changes, downloads, and backend installation progress"];

/// Bullets for the `secrets` scope shown in the consent popup.
pub const SECRETS_PERMISSIONS: &[&str] = &[
    "Store, update, and clear this backend's API credentials",
    "Cannot read or display any stored credential value",
];

/// Fallback bullets shown if the daemon spawns the popup with a scope
/// the helper doesn't recognize. Should never appear in production.
pub const UNKNOWN_SCOPE_PERMISSIONS: &[&str] = &[
    "Unknown scope — the requesting app sent something the daemon doesn't recognize. Denying is safe.",
];
