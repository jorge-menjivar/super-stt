// SPDX-License-Identifier: GPL-3.0-only
//! The synthesis model's voice: reading which one it speaks in, and setting it.
//!
//! The sibling of [`super::language`], and shaped the same way — the two values
//! (what is set, what may be set) arrive from two endpoints and are held apart,
//! because choosing a voice rewrites the first and cannot change the second.

use crate::core::app::AppModel;
use crate::daemon::client::v1::pipeline::voice as model_voice;
use crate::state::{ErrorScope, VoiceResolution};
use crate::ui::messages::{Message, VoiceMessage};
use cosmic::prelude::*;
use super_stt_shared::daemon::http_client::HttpResult;

impl AppModel {
    /// Route every voice message.
    pub(in crate::core::app) fn handle_voice_messages(
        &mut self,
        message: VoiceMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            // The block names the pair it describes, and the list is chained
            // off it: issuing the list before the pair is held would have its
            // answer discarded on arrival as another model's.
            VoiceMessage::ModelVoiceLoaded {
                source,
                model,
                block,
            } => {
                self.voice.resolution = Some(block);
                self.voice.target = Some((source.clone(), model.clone()));
                self.voice.choices.clear();
                list_model_voices(source, model)
            }
            VoiceMessage::ModelVoicesListed {
                source,
                model,
                voices,
            } => {
                // Guarded: a list that arrives after the user has moved to
                // another model belongs to the model they left.
                if self.voice.target.as_ref() == Some(&(source, model)) {
                    self.voice.choices = voices;
                }
                Task::none()
            }
            VoiceMessage::ModelVoiceSelected(voice) => self.write_model_voice(voice),
            VoiceMessage::VoiceError(e) => {
                log::warn!("Voice settings error: {e}");
                self.set_action_error(
                    ErrorScope::Synthesis,
                    format!("Couldn't set the voice: {e}"),
                );
                Task::none()
            }
        }
    }

    /// Write the picked voice, or clear it when the user chose the model's own
    /// default.
    ///
    /// Both verbs answer with the resolution that results, so the card renders
    /// its own click without a second read — and both are addressed to the pair
    /// the card is currently showing, never to "the active model", so a click
    /// landing after a model switch writes nothing rather than the wrong
    /// model's preference.
    fn write_model_voice(&mut self, voice: Option<String>) -> Task<cosmic::Action<Message>> {
        let Some((source, model)) = self.voice.target.clone() else {
            return Task::none();
        };
        self.clear_action_error(ErrorScope::Synthesis);
        let finish = voice_loaded(source, model.clone());
        match voice {
            Some(voice) => Task::perform(model_voice::set_model_voice(model, voice), finish),
            None => Task::perform(model_voice::clear_model_voice(model), finish),
        }
    }
}

/// Fetch a model's voice resolution. Called when a synthesis model is staged or
/// selected, so the card can show its voice before Load.
pub(in crate::core::app) fn load_model_voice(
    source: &str,
    model: String,
) -> Task<cosmic::Action<Message>> {
    Task::perform(
        model_voice::get_model_voice(model.clone()),
        voice_loaded(source.to_string(), model),
    )
}

/// Re-read the voices one model can be pinned to.
fn list_model_voices(source: String, model: String) -> Task<cosmic::Action<Message>> {
    Task::perform(
        model_voice::list_model_voices(model.clone()),
        move |res| match res {
            Ok(voices) => cosmic::Action::App(Message::Voice(VoiceMessage::ModelVoicesListed {
                source: source.clone(),
                model: model.clone(),
                voices,
            })),
            Err(e) => cosmic::Action::App(Message::Voice(VoiceMessage::VoiceError(e.to_string()))),
        },
    )
}

/// The message a voice read or write answers with: the block for the pair it
/// was asked about, or the failure.
fn voice_loaded(
    source: String,
    model: String,
) -> impl Fn(HttpResult<VoiceResolution>) -> cosmic::Action<Message> {
    move |res| match res {
        Ok(block) => cosmic::Action::App(Message::Voice(VoiceMessage::ModelVoiceLoaded {
            source: source.clone(),
            model: model.clone(),
            block,
        })),
        Err(e) => cosmic::Action::App(Message::Voice(VoiceMessage::VoiceError(e.to_string()))),
    }
}
