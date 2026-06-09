// SPDX-License-Identifier: GPL-3.0-only
//! HTTP server for the daemon protocol. See `server.rs` for the listener
//! and `v1/` for the versioned endpoint tree.

mod internal;
mod server;
mod state;
mod v1;

pub use server::AUTO_APPROVE_ENV;
pub use server::start_http_server;
