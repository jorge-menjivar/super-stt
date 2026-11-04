// SPDX-License-Identifier: GPL-3.0-only
//! Per-`Store` host state for running a WASM backend component, including the
//! outbound-host allowlist that confines a component's network egress:
//! `super_engine_daemon::wasm::host`, shared with Super STT.

pub use super_engine_daemon::wasm::host::*;
