// SPDX-License-Identifier: GPL-3.0-only

//! The applet's single daemon identity: the session `AppId`, the human-facing
//! app name, the requested scopes, and the SSE topics. Both the `/ping`
//! liveness client and the `/events` subscription must present the SAME
//! `AppId` + scopes so they share one cached widget-scope token; keeping them
//! here (instead of a copy in each) makes that invariant a fact rather than an
//! eyeball check (App-let Tier 3 #19). The two copies previously even disagreed
//! on the display name.

use super_stt_shared::daemon::session::AppId;

/// Stable identity caching the applet's widget-scope session token under
/// `(super-stt-session, super-stt-cosmic-applet)`.
pub const APP_ID: AppId = AppId("super-stt-cosmic-applet");

/// Human-facing name shown in the daemon's consent prompt.
pub const APP_NAME: &str = "Super STT COSMIC Applet";

/// Scopes the applet requests: `recording_events` for the recording indicator,
/// `audio_visualization` for the frequency-band meter.
pub const SCOPES: &[&str] = &["recording_events", "audio_visualization"];

/// `/events` SSE topics the applet subscribes to.
pub const TOPICS: &[&str] = &[
    "recording_state",
    "frequency_bands",
    "transcribing_started",
    "transcribing_stopped",
];
