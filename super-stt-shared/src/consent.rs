// SPDX-License-Identifier: GPL-3.0-only
//! Static content shown in the consent popup.
//!
//! Lives here rather than in the popup because there is more than one popup.
//! Linux renders the libcosmic `super-stt-consent` helper; macOS has no such
//! helper, so the daemon puts the same question up through `osascript`. Both
//! describe the same grant, and a user who is told different things by the
//! two is being asked to approve something neither sentence pins down — so
//! the sentences are written once, here, and the daemon's scope list is held
//! against them by `every_known_scope_has_specific_permissions` below.
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

/// The bullet list for one scope, or [`UNKNOWN_SCOPE_PERMISSIONS`] when the
/// scope is not one this build knows.
///
/// Every scope in [`crate::daemon::scopes::known_scopes`] must have an arm
/// here; the fallback is a warning shown to the user, not a default.
#[must_use]
pub fn permissions_for_scope(scope: &str) -> &'static [&'static str] {
    match scope {
        "transcribe" => TRANSCRIBE_PERMISSIONS,
        "status" => STATUS_PERMISSIONS,
        "settings" => SETTINGS_PERMISSIONS,
        "recording_events" => RECORDING_EVENTS_PERMISSIONS,
        "audio_visualization" => AUDIO_VISUALIZATION_PERMISSIONS,
        "global_transcriptions" => GLOBAL_TRANSCRIPTIONS_PERMISSIONS,
        "daemon_status" => DAEMON_STATUS_PERMISSIONS,
        "secrets" => SECRETS_PERMISSIONS,
        _ => UNKNOWN_SCOPE_PERMISSIONS,
    }
}

/// Union of the per-scope bullet lists for every scope the app asked
/// for, de-duplicated and order-preserving. Falls back to the unknown
/// bullet if the set is empty.
#[must_use]
pub fn permissions_for_scopes(scopes: &[String]) -> Vec<&'static str> {
    let mut lines: Vec<&'static str> = Vec::new();
    if scopes.is_empty() {
        lines.extend_from_slice(UNKNOWN_SCOPE_PERMISSIONS);
        return lines;
    }
    for scope in scopes {
        for &line in permissions_for_scope(scope) {
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{UNKNOWN_SCOPE_PERMISSIONS, permissions_for_scope, permissions_for_scopes};

    /// Every scope the daemon accepts must have a specific consent description.
    /// A daemon scope that falls through to `UNKNOWN_SCOPE_PERMISSIONS` would
    /// render the "unknown scope — deny is safe" warning on a legitimate prompt,
    /// so this pins the two lists together (Tier 2 #8).
    #[test]
    fn every_known_scope_has_specific_permissions() {
        for scope in crate::daemon::scopes::known_scopes() {
            assert!(
                !std::ptr::eq(permissions_for_scope(scope), UNKNOWN_SCOPE_PERMISSIONS),
                "scope `{scope}` has no specific consent description; add an arm to permissions_for_scope"
            );
        }
    }

    /// The union is de-duplicated and keeps the order the scopes were asked
    /// in, so the dialog reads as one list rather than a concatenation with
    /// repeats.
    #[test]
    fn scope_union_dedupes_and_keeps_order() {
        let lines = permissions_for_scopes(&["transcribe".into(), "transcribe".into()]);
        assert_eq!(lines, super::TRANSCRIBE_PERMISSIONS.to_vec());
    }

    /// No scopes at all is not "nothing to warn about" — it is a request the
    /// dialog cannot describe, and the user is told so.
    #[test]
    fn empty_scope_list_warns() {
        assert_eq!(
            permissions_for_scopes(&[]),
            UNKNOWN_SCOPE_PERMISSIONS.to_vec()
        );
    }
}
