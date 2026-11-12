// SPDX-License-Identifier: GPL-3.0-only
//! Unloading a model when idle and loading it again on demand. The setting is
//! [`crate::config::IdleUnloadConfig`]; the model's slot
//! ([`super_engine_daemon::model_slot::ModelSlot`]) keeps its last use and
//! does the unload, and each feature's own `model_management::idle` decides
//! what "in use" means for it.

use std::time::Duration;

/// How often a daemon looks at whether its model has gone idle.
pub const CHECK_EVERY: Duration = Duration::from_secs(5);
