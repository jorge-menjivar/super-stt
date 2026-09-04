// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline` — the ordered stages a transcript passes through.
//!
//! Mirrors the daemon's `v1/pipeline/` tree: [`stage`] wraps `/pipeline/{stage}`,
//! [`model`] wraps `/pipeline/{stage}/model` and its verbs, [`device`] wraps the
//! device lists and the per-model device preference.

pub(crate) mod device;
pub(crate) mod model;
pub(crate) mod stage;
