// SPDX-License-Identifier: GPL-3.0-only

//! The per-model language control, shared by both pipeline stages.
//!
//! The daemon's `/pipeline/{stage}/model/{model}/language` is keyed by the
//! model, not by the stage it runs in, so one control serves the transcription
//! card and the post-processing card alike. It lives here rather than on either
//! card so the two cannot drift apart.

use cosmic::Element;
use cosmic::widget;

use crate::core::app::AppModel;
use crate::daemon::backends::BackendInfo;
use crate::ui::languages::friendly_name;
use crate::ui::messages::{LanguageMessage, Message};

/// The language control for `selected_model`, rendered inline next to the
/// model/device controls rather than on its own row.
///
/// A multilingual model gets a button that opens the language picker, labelled
/// with its resolved language. A model that speaks one language gets the same
/// button showing that language, without a picker: there is nothing to choose,
/// and the daemon refuses a write for it (`unsupported_language`). It is still
/// shown, because hiding it left the card looking as though language did not
/// apply to that model at all.
///
/// `None` only when the model is not in this backend's catalog, or when nothing
/// is known about its language yet.
pub(super) fn language_button<'a>(
    stage: u32,
    backend: &'a BackendInfo,
    selected_model: &str,
    app: &'a AppModel,
) -> Option<Element<'a, Message>> {
    let catalog_model = backend.models.iter().find(|m| m.name == selected_model)?;
    let source = &backend.source;
    // A miss is "not answered yet", never "no language": the label falls back
    // to what the catalog already declares rather than to nothing.
    let block = app
        .language
        .model_languages
        .get(stage, source, selected_model);

    if !catalog_model.multilingual {
        let tag = block
            .map(|b| b.primary.as_str())
            .filter(|t| !t.is_empty())
            .unwrap_or(catalog_model.primary_language.as_str());
        if tag.is_empty() {
            return None;
        }
        // No `on_press`: the button renders disabled, which is what says the
        // language is fixed rather than merely unset.
        return Some(widget::button::standard(friendly_name(tag)).into());
    }

    let label = block.map_or_else(
        // Block not fetched yet — a neutral label, not a guessed language.
        || "Language".to_string(),
        |block| {
            let resolved_from = if block.source.is_empty() {
                "default"
            } else {
                block.source.as_str()
            };
            match (block.effective.as_deref(), resolved_from) {
                (Some(tag), "override") => friendly_name(tag),
                (Some(tag), "global") => format!("{} · global", friendly_name(tag)),
                _ => format!("{} · default", friendly_name(&block.primary)),
            }
        },
    );

    Some(
        widget::button::standard(label)
            .on_press(Message::Language(LanguageMessage::OpenLanguagePicker {
                model: Some((stage, source.clone(), selected_model.to_string())),
            }))
            .into(),
    )
}
