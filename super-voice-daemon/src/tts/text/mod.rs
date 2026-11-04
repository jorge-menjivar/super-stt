// SPDX-License-Identifier: GPL-3.0-only
//! Preparing text for a synthesis backend.
//!
//! Both stages live in the daemon rather than in each backend, for the same
//! reason audio capture did on the STT side: doing it once centrally means
//! every backend benefits, and the policy is tunable per model instead of
//! being whatever each author happened to implement.
//!
//! - [`normalize`] strips markup that would otherwise be read out literally.
//! - [`chunk`] splits normalized text on sentence boundaries.
//!
//! Both are streaming-first, because text arrives from an LLM a token at a
//! time and neither markup nor sentence boundaries can be resolved from a
//! prefix alone.

pub mod chunk;
pub mod normalize;
