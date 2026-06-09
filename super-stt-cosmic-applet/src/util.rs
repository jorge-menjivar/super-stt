// SPDX-License-Identifier: GPL-3.0-only
//! Numeric-cast helpers that localize the lossy-cast lint allows to one
//! audited place instead of scattering `#[allow(...)]` across the
//! geometry and rendering code. UI dimensions and band counts never
//! approach the `f32` mantissa limit, so the precision loss is
//! irrelevant in practice.

/// Cast a count, length, or index to `f32` for geometry math.
#[allow(clippy::cast_precision_loss)]
pub fn usize_to_f32(value: usize) -> f32 {
    value as f32
}

/// Cast a `u32` pixel/size value to `f32`.
#[allow(clippy::cast_precision_loss)]
pub fn u32_to_f32(value: u32) -> f32 {
    value as f32
}

/// Truncate a non-negative `f32` pixel measure to `usize`. Negative
/// inputs clamp to `0`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn f32_to_usize(value: f32) -> usize {
    value.max(0.0) as usize
}

/// Narrow an `f64` (e.g. a JSON number) to `f32`.
#[allow(clippy::cast_possible_truncation)]
pub fn f64_to_f32(value: f64) -> f32 {
    value as f32
}
