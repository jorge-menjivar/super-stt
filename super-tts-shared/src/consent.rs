// SPDX-License-Identifier: GPL-3.0-only
//! Static content shown in the consent popup.
//!
//! Lives here rather than in the popup so the daemon can describe a grant
//! with the same sentences: the shared auth code in `super-engine-daemon`
//! asks each product for them, and its macOS dialog renders them where Linux
//! runs the `super-tts-consent` helper. A user told different things by the
//! two would be asked to approve something neither sentence pins down.
//!
//! Each `*_PERMISSIONS` array is the bullet list for one scope. A token
//! can carry several scopes, so the popup renders the union of these for
//! every scope the requesting app asked for. Edit the strings here to
//! change what the user is told they're approving — keep each entry
//! concise (≤ one wrapped line on a typical screen) and user-meaningful.
//! These are what the user reads in the dialog, not a developer reference.
//!
//! How the daemon hands the request to the Linux dialog, and how the dialog
//! answers, is [`contract`]: the variable names and the three answers, shared
//! with Super STT.

pub use super_engine_protocol::consent as contract;

/// Bullets for the `speak` scope.
///
/// Phrased around what the user gives up — control of the speakers — rather
/// than what the app gains. Interrupting is called out because it is the part
/// that surprises people: granting this lets the app cut off whatever else is
/// being read aloud.
pub const SPEAK_PERMISSIONS: &[&str] = &[
    "Play synthesized speech through your speakers",
    "Interrupt or stop speech this or another app started",
];

/// Bullets for the `playback_events` scope.
///
/// Reading these events reveals *when* the machine is speaking and how far
/// through it is — not the text — so the bullets say exactly that rather than
/// implying access to content the scope does not grant.
pub const PLAYBACK_EVENTS_PERMISSIONS: &[&str] = &[
    "See when speech starts and stops",
    "See how far through the current speech playback has got",
];

/// Bullets for the `status` scope.
pub const STATUS_PERMISSIONS: &[&str] = &[
    "Read which voice model and device are currently active",
    "See whether the machine is speaking right now",
];

/// Bullets for the `settings` scope.
pub const SETTINGS_PERMISSIONS: &[&str] = &[
    "Read and change every daemon setting (model, device, audio cues, volume)",
    "Allow or block sending text to online providers",
    "Install, update, and remove voice backends",
];

/// Bullets for the `audio_visualization` scope.
pub const AUDIO_VISUALIZATION_PERMISSIONS: &[&str] =
    &["Receive audio visualization data (frequency bars) while speech is playing"];

/// Bullets for the `daemon_status` scope.
pub const DAEMON_STATUS_PERMISSIONS: &[&str] =
    &["Monitor model changes, downloads, and backend installation progress"];

/// Bullets for the `voices` scope.
///
/// These are recordings of somebody speaking, so the bullets name the
/// recordings themselves rather than "manage voices" — the thing the user is
/// deciding about is audio of a person, not a settings list.
pub const VOICES_PERMISSIONS: &[&str] = &[
    "List, play back, and delete your saved voice recordings",
    "Add new voice recordings to clone a voice from",
];

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
        "speak" => SPEAK_PERMISSIONS,
        "playback_events" => PLAYBACK_EVENTS_PERMISSIONS,
        "status" => STATUS_PERMISSIONS,
        "settings" => SETTINGS_PERMISSIONS,
        "audio_visualization" => AUDIO_VISUALIZATION_PERMISSIONS,
        "daemon_status" => DAEMON_STATUS_PERMISSIONS,
        "secrets" => SECRETS_PERMISSIONS,
        "voices" => VOICES_PERMISSIONS,
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
        let lines = permissions_for_scopes(&["speak".into(), "speak".into()]);
        assert_eq!(lines, super::SPEAK_PERMISSIONS.to_vec());
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
