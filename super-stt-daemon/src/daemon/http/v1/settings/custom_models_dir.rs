// SPDX-License-Identifier: GPL-3.0-only
settings_setter!(
    set_custom_models_dir,
    CustomModelsDirBody { path: Option<String> },
    "set_custom_models_dir",
    "path"
);
settings_dispatch!(get_custom_models_dir, "get_custom_models_dir");
