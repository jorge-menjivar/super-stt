// SPDX-License-Identifier: GPL-3.0-only
//! Library face of the Whisper subprocess backend. The actual binary lives in
//! `src/main.rs`; this lib exists so integration tests can `use` the engine
//! directly without round-tripping through the `/v1` socket.

#![allow(clippy::doc_markdown)]

pub mod inference;
