// SPDX-License-Identifier: GPL-3.0-only
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct AudioLevel {
    pub level: f32,
    pub is_speech: bool,
    pub timestamp: Instant,
}
