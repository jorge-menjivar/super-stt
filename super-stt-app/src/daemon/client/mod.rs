// SPDX-License-Identifier: GPL-3.0-only
//! Daemon-facing operations for the settings app.
//!
//! Mirrors the daemon's server `v1/` tree: one module per endpoint
//! family under [`v1`], built on the shared Unix-socket transport
//! (`super_stt_shared::daemon::http_client::transport`) and the cached
//! `settings`-scope token ([`internal::session`]).

pub(crate) mod internal;
pub(crate) mod v1;

pub use v1::health::{ping_daemon, test_daemon_connection};
pub use v1::transcribe::{RecordEvent, record_command_stream, stop_record_command};

pub use v1::settings::active_device::{get_current_device, set_device};
pub use v1::settings::active_model::{
    cancel_download, get_current_model, get_download_status, list_available_models, set_model,
    unload_active_model,
};
pub use v1::settings::allow_online_models::set_allow_online_models;
pub use v1::settings::audio_theme::{
    get_current_audio_theme, load_audio_themes, set_and_test_audio_theme, set_audio_theme,
};
pub use v1::settings::backend_secrets::{
    clear_backend_secret, list_backend_secrets, set_backend_secret,
};
pub use v1::settings::backends::{
    clear_active_backend, clear_backend_option, get_active_backend, get_gpu_info, list_backends,
    set_active_backend, set_backend_option,
};
pub use v1::settings::custom_models_dir::{get_custom_models_dir, set_custom_models_dir};
pub use v1::settings::preview_typing::{get_preview_typing, set_preview_typing};
pub use v1::settings::recording_stop_mode::{get_recording_stop_mode, set_recording_stop_mode};
pub use v1::settings::volume::{get_volume, set_volume};
pub use v1::settings::write_method::{get_write_method, set_write_method};
