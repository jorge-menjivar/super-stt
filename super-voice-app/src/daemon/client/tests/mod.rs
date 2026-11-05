// SPDX-License-Identifier: GPL-3.0-only
//! What each `v1/` wrapper puts on the wire, and what it makes of the answer.
//!
//! One module per module under `v1/`, each driving the real functions against
//! [`fake_daemon`]. The companion is [`super::path_contract`], which checks the
//! *paths* against the daemon's OpenAPI document without running anything;
//! these check everything else about the request, and the parsing on the way
//! back.
//!
//! Under a `tests/` directory inside `src/` on purpose: the crate is a binary,
//! so there is no library for an external test to link against, and the
//! coverage run excludes `tests/` — test code counting itself as covered
//! product code is how a suite flatters its own numbers.

mod backends;
mod fake_daemon;
mod gpu_info;
mod ping;
mod pipeline;
mod registry;
mod session;
mod settings;
mod transcribe;
mod update;
