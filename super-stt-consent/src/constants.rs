// SPDX-License-Identifier: GPL-3.0-only
//! Static content shown in the consent popup.
//!
//! Each `*_PERMISSIONS` array is the bullet list rendered for one
//! scope. Edit the strings here to change what the user is told
//! they're approving — keep each entry concise (≤ one wrapped line
//! on a typical screen) and user-meaningful. These are what the user
//! reads in the dialog, not a developer reference.

/// Bullets shown when the requester asks for the `client` scope.
pub const CLIENT_PERMISSIONS: &[&str] = &[
    "Use your microphone to record speech for this app",
    "Receive this app's own transcription text (preview and final)",
    "Read which speech-to-text model is currently loaded",
];

/// Bullets shown when the requester asks for the `settings` scope.
pub const SETTINGS_PERMISSIONS: &[&str] = &[
    "Use your microphone (same as a regular app)",
    "Choose which speech-to-text model the daemon uses",
    "Allow or block sending audio to online providers (OpenAI, Mistral, Deepgram)",
    "Change audio cues, volume, and recording behavior",
    "Listen in on recordings and transcriptions from every app on this device",
];

/// Bullets shown when the requester asks for the `widget` scope.
pub const WIDGET_PERMISSIONS: &[&str] = &[
    "See when any recording is active on this device",
    "Receive raw audio samples and visualization data while a recording is running",
    "Read live and final transcription text from every app on this device",
];

/// Fallback bullets shown if the daemon spawns the popup with a scope
/// the helper doesn't recognize. Should never appear in production.
pub const UNKNOWN_SCOPE_PERMISSIONS: &[&str] = &[
    "Unknown scope — the requesting app sent something the daemon doesn't recognize. Denying is safe.",
];
