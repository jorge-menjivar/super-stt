// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/custom_models_dir` — where the daemon looks for models a user
//! supplied themselves, rather than ones a backend installed.

use super::super::wire::CustomModelsDirState;

settings_setter!(
    set_custom_models_dir,
    CustomModelsDirBody { path: Option<String> },
    "set_custom_models_dir",
    "path",
    "/settings/custom_models_dir",
    CustomModelsDirState,
    "Point the daemon at a models directory of your own",
    "Overrides where the daemon looks for model files supplied out of band. The directory is \
scanned as soon as the setting lands, so anything found in it shows up in \
`GET /pipeline/{stage}/model/list` under the `custom` source and can be selected from there \
without a restart. Send `null` to clear the override and fall back to the default location. \
Backends installed from the registry live elsewhere and are unaffected.",
    "Absolute path to a directory the daemon can read, or `null` to clear the override.",
);
settings_dispatch!(
    get_custom_models_dir,
    "get_custom_models_dir",
    get "/settings/custom_models_dir",
    CustomModelsDirState,
    "Read the custom models directory",
    "Answers with the configured path, or `null` when no override is set. The key is always \
present — `null` is the answer, not an absent field — so a client can render the row without \
guessing whether the daemon simply declined to speak to it."
);
