// SPDX-License-Identifier: GPL-3.0-only
//! Daemon-facing operations for the settings app.
//!
//! Mirrors the daemon's server `v1/` tree: one module per endpoint
//! family under [`v1`], built on the shared Unix-socket transport
//! (`super_tts_shared::daemon::http_client::transport`) and the cached
//! `settings`-scope token ([`internal::session`]).
//!
//! `path_contract` is the test that keeps the mirroring honest: the URLs are
//! strings on both sides, and nothing but that test notices when a daemon
//! endpoint is renamed out from under a call site here.

#[cfg(test)]
mod path_contract;

pub(crate) mod internal;
pub(crate) mod v1;

pub use v1::backends::list_backends;
pub use v1::backends::options::{clear_backend_option, set_backend_option};
pub use v1::backends::secrets::{clear_backend_secret, list_backend_secrets, set_backend_secret};
pub use v1::gpu_info::get_gpu_info;
pub use v1::ping::{ping_daemon, test_daemon_connection};
pub use v1::speak::{speak_command, speak_in_voice, stop_speaking_command};

pub use v1::pipeline::backend::list_stage_backends;
pub use v1::pipeline::device::{
    get_model_device, list_model_devices, list_stage_devices, set_model_device,
};
pub use v1::pipeline::model::{
    cancel_download, get_download_status, get_stage_model, list_stage_models, reload_stage_model,
    set_stage_model, unload_stage_model,
};
pub use v1::pipeline::stage::{clear_stage_backend, set_stage_backend};
pub use v1::pipeline::{StageState, get_stage_view};

pub use v1::settings::allow_online_models::set_allow_online_models;
pub use v1::settings::audio_theme::{
    get_current_audio_theme, load_audio_themes, set_and_test_audio_theme, set_audio_theme,
};
pub use v1::settings::custom_models_dir::get_custom_models_dir;
pub use v1::settings::notification_method::{get_notification_method, set_notification_method};
pub use v1::settings::update_beta_optin::set_update_beta_optin;
pub use v1::settings::update_check_enabled::{get_update_check_enabled, set_update_check_enabled};
pub use v1::settings::volume::{get_volume, set_volume};

pub use v1::update::{check_update_now, get_update_status};

pub use v1::voice::{create_voice, delete_voice, list_voices, rename_voice};
