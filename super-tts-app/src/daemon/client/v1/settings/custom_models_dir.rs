// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/custom_models_dir` — where downloaded models are stored.
//!
//! `None` is the answer, not a failure: it means no override is set and the
//! daemon uses its default cache. Read-only here because the app shows the
//! effective directory and does not offer to change it.

settings_getter!(
    get_custom_models_dir -> Option<String>, "/settings/custom_models_dir", "get_custom_models_dir",
    |resp| resp.custom_models_dir.unwrap_or(None)
);
