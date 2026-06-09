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

/// `u64` → `f64` (e.g. byte totals for progress %).
#[allow(clippy::cast_precision_loss)]
pub(crate) fn u64_to_f64(x: u64) -> f64 {
    x as f64
}

/// `f64` → `f32`, truncating. Caller guarantees the value fits in f32 range.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn f64_to_f32(x: f64) -> f32 {
    x as f32
}

/// `f32` → `usize`, truncating toward zero. Caller guarantees non-negative + in range.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn f32_to_usize(x: f32) -> usize {
    x as usize
}

/// `f32` → `u64`, truncating toward zero. Caller guarantees non-negative + in range.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn f32_to_u64(x: f32) -> u64 {
    x as u64
}

/// `f32` → `i16` PCM sample. Caller MUST clamp to [`i16::MIN`] as f32 / [`i16::MAX`] as f32 first.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn f32_to_i16(x: f32) -> i16 {
    x as i16
}
