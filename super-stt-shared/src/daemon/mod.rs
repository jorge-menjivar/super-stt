// SPDX-License-Identifier: GPL-3.0-only
//! Shared daemon communication functionality for Super STT applications

pub mod client;
pub mod http_client;
pub mod session;
pub mod widget_subscription;

pub use client::*;
