// SPDX-License-Identifier: GPL-3.0-only
//! `/custom_models_dir` — optional override for where local models are stored.

settings_getter!(
    get_custom_models_dir -> Option<String>, "/custom_models_dir", "get_custom_models_dir",
    |resp| resp.custom_models_dir.unwrap_or(None)
);
