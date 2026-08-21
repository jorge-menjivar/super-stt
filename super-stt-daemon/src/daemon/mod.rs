// SPDX-License-Identifier: GPL-3.0-only

pub mod backend_config_handlers;
pub mod core;
pub mod device_management;
pub mod download_handlers;
pub mod events;
pub mod http;
pub(crate) mod language;
pub mod language_handlers;
pub mod model_management;
pub mod recording;
pub mod self_update_handlers;
pub mod settings_handlers;
pub mod startup;
pub mod status_handlers;
#[cfg(test)]
pub(crate) mod test_fixtures;
pub mod theme_handlers;
pub mod transcription;
pub mod types;
