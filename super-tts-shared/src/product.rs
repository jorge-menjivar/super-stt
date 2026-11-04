// SPDX-License-Identifier: GPL-3.0-only
//! Super Voice's names: everything the daemon and its clients meet on, from
//! the socket to the scopes a token can carry. Text to speech is one of its
//! features. See `super_engine_protocol::ProductSpec`.
//!
//! [`SUPER_VOICE`] is defined in `super-voice-registry-types`, where the
//! installer can reach it, and its tests are `super-stt-shared`'s.

pub use super_voice_registry_types::product::SUPER_VOICE;
