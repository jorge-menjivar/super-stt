// SPDX-License-Identifier: GPL-3.0-only
//! Pipeline stage numbers, as `/pipeline/{stage}` spells them.
//!
//! A transcript passes through ordered stages: stage 1 turns audio into text,
//! stage 2 rewrites it. The number is the address in every pipeline path, the
//! `stage` field on the events a stage emits, and how a client tells whose
//! model a load or a download belongs to — so it is defined once, here, rather
//! than spelled `1` and `2` at each of those sites.

/// Stage 1: audio to text.
pub const TRANSCRIPTION_STAGE: u32 = 1;

/// Stage 2: the transcript rewriter.
pub const POST_PROCESSOR_STAGE: u32 = 2;

/// The stage a payload carrying no `stage` field belongs to.
///
/// Transcription is the only stage that existed before the field, so an older
/// daemon's events and download reports read as stage 1 — which is what they
/// always were. Used as the serde default on every `stage` field.
#[must_use]
pub(crate) const fn default_stage() -> u32 {
    TRANSCRIPTION_STAGE
}
