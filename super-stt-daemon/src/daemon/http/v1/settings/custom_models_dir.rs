// SPDX-License-Identifier: GPL-3.0-only
use super::wire::CustomModelsDirState;

settings_setter!(
    set_custom_models_dir,
    CustomModelsDirBody { path: Option<String> },
    "set_custom_models_dir",
    "path",
    "/custom_models_dir",
    CustomModelsDirState,
    "Point the daemon at a models directory of your own",
    "Overrides where the daemon looks for model files. Send `null` to clear the \
override and fall back to the default location. Backends installed from the registry \
are unaffected \u{2014} this is for models supplied out of band.",
    "Absolute path to the directory, or `null` to clear the override.",
);
settings_dispatch!(
    get_custom_models_dir,
    "get_custom_models_dir",
    get "/custom_models_dir",
    CustomModelsDirState,
    "Read the custom models directory",
    "Answers with the configured path, or `null` when no override is set."
);
