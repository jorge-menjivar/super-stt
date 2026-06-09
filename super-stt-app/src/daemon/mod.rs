// SPDX-License-Identifier: GPL-3.0-only
//! Daemon communication for the settings app: the HTTP [`client`] (a mirror
//! of the daemon's `v1/` endpoint tree), the [`registry`] facade, the bundled
//! backend [`catalog`], and the installed-backend [`backends`] types.
pub mod backends;
pub mod catalog;
pub mod client;
pub mod registry;
