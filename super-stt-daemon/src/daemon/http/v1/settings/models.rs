// SPDX-License-Identifier: GPL-3.0-only
//! `/models` \u{2014} the models the backend filling stage 1 serves.
//!
//! Selecting and loading one happens through `/pipeline/{stage}/model` (see
//! `super::pipeline`); this is only the flat catalog read that a picker fills
//! itself from.

use super::wire::ModelList;

settings_dispatch!(
    list_models,
    "list_models",
    get "/models",
    ModelList,
    "List the models the active backend can transcribe with",
    "Scoped to the backend currently filling stage 1 \u{2014} only its models are \
switchable. Post-processor models are excluded, since selecting one as a \
transcription model would fail every recording; the full catalog with roles is at \
`GET /backends`."
);
