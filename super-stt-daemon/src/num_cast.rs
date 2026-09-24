// SPDX-License-Identifier: GPL-3.0-only
//! Centralized lossy numeric conversions. Each carries a single scoped clippy
//! allow with justification, so call sites stay lint-clean and the rationale
//! lives in one place. Values passed here are bounded by construction
//! (sample rates, sample counts, byte totals) so the loss is inconsequential.

/// `u32` → `f32` (e.g. sample rates ≤ `384_000`; exact for values < 2^24).
#[allow(clippy::cast_precision_loss)]
pub(crate) fn u32_to_f32(x: u32) -> f32 {
    x as f32
}

/// `usize` → `f32` (sample counts / buffer lengths).
#[allow(clippy::cast_precision_loss)]
pub(crate) fn usize_to_f32(x: usize) -> f32 {
    x as f32
}

/// `usize` → `f64`.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn usize_to_f64(x: usize) -> f64 {
    x as f64
}

/// `f32` → `usize`, truncating toward zero. Caller guarantees non-negative + in range.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn f32_to_usize(x: f32) -> usize {
    x as usize
}
