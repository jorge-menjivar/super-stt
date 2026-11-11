// SPDX-License-Identifier: GPL-3.0-only
//! Text to speech: Super Voice's second feature.
//!
//! What is the feature's own lives here: its models and their transports, the
//! speech engine and the output device, cloned voices, text normalization, and
//! the routes under `/v1/tts`. What the daemon has once — the config file, the
//! event bus, secrets, the registry, settings, auth — is the crate's, and this
//! module reaches it through the handles [`daemon::types::SuperTTSDaemon`] is
//! built with.

pub mod audio;
pub mod daemon;
pub mod download_progress;
pub mod input;
pub mod output;
pub mod text;
pub mod tts_models;
pub mod voices;

mod num_cast;
