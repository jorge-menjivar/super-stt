// SPDX-License-Identifier: GPL-3.0-only
//! The context write verbs, at the daemon layer.
//!
//! What is checked here is the part every transport shares: what is refused,
//! what the stored file ends up holding, and which of the three per-backend
//! states a write lands in. The route wrappers are covered by the HTTP smoke
//! tests; the resolution rule itself by `config_tests`.

use crate::daemon::types::{LoadedModel, SuperSTTDaemon, test_daemon};
use crate::stt_models::ModelDefinition;
use crate::stt_models::backends::DiscoveredBackend;
use crate::stt_models::transcribe::{
    BackendContext, ModelInfo, ModelInfoData, ModelState, Transcribe,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_stt_registry_types::manifest::{Capabilities, Device, ModelRole, SttCapabilities};
use super_stt_shared::models::contexts::{DictationContext, MAX_PROMPT_CHARS};
use super_stt_shared::models::protocol::ErrorCode;

fn coding() -> DictationContext {
    DictationContext {
        id: "coding".to_string(),
        name: "Coding".to_string(),
        prompt: "I dictate code.".to_string(),
        vocabulary: vec!["main branch".to_string(), "kubectl".to_string()],
    }
}

#[tokio::test]
async fn a_context_is_stored_and_then_replaced_in_place() {
    let daemon = test_daemon().await;

    assert_eq!(daemon.handle_set_context(coding()).await.status, "success");
    let second = DictationContext {
        id: "email".to_string(),
        name: "Email".to_string(),
        ..DictationContext::default()
    };
    assert_eq!(daemon.handle_set_context(second).await.status, "success");

    let renamed = DictationContext {
        name: "Writing code".to_string(),
        ..coding()
    };
    assert_eq!(daemon.handle_set_context(renamed).await.status, "success");

    let config = daemon.config.read().await;
    let ids: Vec<&str> = config
        .contexts
        .items
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["coding", "email"],
        "a re-save edits the row where it is; the order is the user's"
    );
    assert_eq!(
        config.context("coding").expect("stored").name,
        "Writing code"
    );
}

/// The blank rows the settings UI keeps for the next term never reach the file.
#[tokio::test]
async fn the_vocabulary_is_cleaned_before_it_is_stored() {
    let daemon = test_daemon().await;
    let padded = DictationContext {
        vocabulary: vec![
            "  rebase ".to_string(),
            String::new(),
            "   ".to_string(),
            "kubectl".to_string(),
        ],
        ..coding()
    };

    assert_eq!(daemon.handle_set_context(padded).await.status, "success");

    let config = daemon.config.read().await;
    assert_eq!(
        config.context("coding").expect("stored").vocabulary,
        vec!["rebase".to_string(), "kubectl".to_string()]
    );
}

/// A context too long to deliver is refused rather than stored: the headers it
/// becomes cannot carry it, so storing it would report a success that sends
/// nothing.
#[tokio::test]
async fn a_context_that_cannot_be_delivered_is_refused() {
    let daemon = test_daemon().await;
    let overlong = DictationContext {
        prompt: "x".repeat(MAX_PROMPT_CHARS + 1),
        ..coding()
    };

    let resp = daemon.handle_set_context(overlong).await;

    assert_eq!(resp.status, "error");
    assert_eq!(resp.error_code, Some(ErrorCode::InvalidValue));
    assert!(
        daemon.config.read().await.context("coding").is_none(),
        "nothing is stored when the check fails"
    );
}

/// `active` is a path of its own. A context taking that id would be stored and
/// then be permanently unreachable.
#[tokio::test]
async fn a_context_may_not_take_an_id_the_namespace_uses() {
    let daemon = test_daemon().await;
    let shadowing = DictationContext {
        id: "active".to_string(),
        ..coding()
    };

    let resp = daemon.handle_set_context(shadowing).await;

    assert_eq!(resp.status, "error");
    assert_eq!(resp.error_code, Some(ErrorCode::InvalidValue));
}

#[tokio::test]
async fn deleting_a_context_that_is_not_there_says_so() {
    let daemon = test_daemon().await;

    let resp = daemon.handle_delete_context("ghost".to_string()).await;

    assert_eq!(resp.status, "error");
    assert_eq!(
        resp.error_code,
        Some(ErrorCode::NotFound),
        "a client deleting a row it can see has a stale list, and should hear it"
    );
}

/// Both selections point at an id, and only one of them is cleaned up when the
/// context goes: the active selection, which has nowhere else to fall back to.
/// The backend's pin survives so re-creating the context restores it.
#[tokio::test]
async fn deleting_a_context_clears_the_active_selection_and_keeps_a_pin() {
    let daemon = test_daemon().await;
    let source = "github.com/x/whisper";
    daemon.handle_set_context(coding()).await;
    daemon
        .handle_set_active_context(Some("coding".to_string()))
        .await;
    daemon
        .handle_set_backend_context(source.to_string(), Some("coding".to_string()))
        .await;

    assert_eq!(
        daemon
            .handle_delete_context("coding".to_string())
            .await
            .status,
        "success"
    );

    {
        let config = daemon.config.read().await;
        assert!(config.active_context().is_none());
        assert_eq!(
            config.backend_context_override(source),
            Some("coding"),
            "the pin is what the user set; deleting is not undoing it"
        );
        assert!(
            config.resolve_context(source).is_none(),
            "a pin to nothing resolves to nothing — never to the active context"
        );
    }

    daemon.handle_set_context(coding()).await;
    let config = daemon.config.read().await;
    assert_eq!(
        config.resolve_context(source).map(|c| c.id.as_str()),
        Some("coding"),
        "re-creating the context restores the pin"
    );
}

#[tokio::test]
async fn a_selection_must_name_a_context_that_exists() {
    let daemon = test_daemon().await;
    let source = "github.com/x/whisper";

    let active = daemon
        .handle_set_active_context(Some("ghost".to_string()))
        .await;
    assert_eq!(active.error_code, Some(ErrorCode::NotFound));

    let pinned = daemon
        .handle_set_backend_context(source.to_string(), Some("ghost".to_string()))
        .await;
    assert_eq!(pinned.error_code, Some(ErrorCode::NotFound));

    let config = daemon.config.read().await;
    assert!(config.contexts.active.is_none());
    assert_eq!(config.backend_context_override(source), None);
}

/// The three states a backend can be in, and the one that is not an id.
#[tokio::test]
async fn a_backend_follows_the_active_context_pins_one_or_takes_none() {
    let daemon = test_daemon().await;
    let source = "github.com/x/whisper";
    daemon.handle_set_context(coding()).await;
    daemon
        .handle_set_context(DictationContext {
            id: "email".to_string(),
            name: "Email".to_string(),
            prompt: "I dictate email.".to_string(),
            vocabulary: Vec::new(),
        })
        .await;
    daemon
        .handle_set_active_context(Some("coding".to_string()))
        .await;

    // Unpinned: follows whatever is active.
    assert_eq!(
        daemon
            .config
            .read()
            .await
            .resolve_context(source)
            .map(|c| c.id.as_str()),
        Some("coding")
    );

    // Pinned elsewhere.
    daemon
        .handle_set_backend_context(source.to_string(), Some("email".to_string()))
        .await;
    assert_eq!(
        daemon
            .config
            .read()
            .await
            .resolve_context(source)
            .map(|c| c.id.as_str()),
        Some("email")
    );

    // Pinned to nothing at all. The empty string is not a valid id, so it
    // cannot be confused with a context the user made.
    let resp = daemon
        .handle_set_backend_context(source.to_string(), Some(String::new()))
        .await;
    assert_eq!(resp.status, "success");
    assert!(daemon.config.read().await.resolve_context(source).is_none());

    // Back to following the active one.
    daemon
        .handle_set_backend_context(source.to_string(), None)
        .await;
    assert_eq!(
        daemon
            .config
            .read()
            .await
            .resolve_context(source)
            .map(|c| c.id.as_str()),
        Some("coding")
    );
}

#[tokio::test]
async fn clearing_the_active_selection_leaves_the_contexts_alone() {
    let daemon = test_daemon().await;
    daemon.handle_set_context(coding()).await;
    daemon
        .handle_set_active_context(Some("coding".to_string()))
        .await;

    assert_eq!(
        daemon.handle_set_active_context(None).await.status,
        "success"
    );

    let config = daemon.config.read().await;
    assert!(config.active_context().is_none());
    assert_eq!(config.contexts.items.len(), 1);
}

// ---------- a loaded stage that records what it is handed ------------------

/// The contexts one seeded stage has been handed, newest last.
type SeenContexts = Arc<Mutex<Vec<BackendContext>>>;

struct RecordingStage {
    info: ModelInfoData,
    reconfigured: SeenContexts,
}

impl ModelInfo for RecordingStage {
    fn info(&self) -> &ModelInfoData {
        &self.info
    }
}
impl ModelState for RecordingStage {
    fn device(&self) -> String {
        "cpu".to_string()
    }
}
#[async_trait::async_trait]
impl Transcribe for RecordingStage {
    async fn transcribe_audio(
        &mut self,
        _audio: &[f32],
        _sample_rate: u32,
        _language: Option<&str>,
    ) -> anyhow::Result<String> {
        Ok(String::new())
    }

    fn reconfigure(&self, context: BackendContext) {
        self.reconfigured.lock().unwrap().push(context);
    }
}

/// A discovered backend that declares it can be handed a context, and nothing
/// else — no secrets to reach the keyring for, no options to resolve.
fn context_capable(source: &str) -> DiscoveredBackend {
    DiscoveredBackend {
        description: String::new(),
        dir: std::path::PathBuf::from("/tmp").join(source.replace('/', "-")),
        source: source.to_string(),
        id: None,
        name: source.to_string(),
        version: "1.0.0".to_string(),
        kind: "wasm".to_string(),
        entrypoint: "x.wasm".to_string(),
        allowed_hosts: Vec::new(),
        secrets: Vec::new(),
        options: Vec::new(),
        capabilities: Capabilities {
            websocket: false,
            product: SttCapabilities { context: true },
        },
        models: Vec::new(),
    }
}

fn definition(name: &str, source: &str, role: ModelRole) -> ModelDefinition {
    ModelDefinition {
        name: name.to_string(),
        source: source.to_string(),
        is_multilingual: true,
        primary_language: "en".to_string(),
        supported_languages: vec!["en".to_string()],
        estimated_vram_bytes: 0,
        processing_interval: Duration::from_secs(1),
        supported_devices: vec![Device::None],
        realtime: false,
        force_preview_support: true,
        role,
        provider: None,
    }
}

fn stage(name: &str, source: &str, role: ModelRole) -> (LoadedModel, SeenContexts) {
    let reconfigured: SeenContexts = Arc::new(Mutex::new(Vec::new()));
    let loaded = LoadedModel {
        definition: definition(name, source, role),
        instance: Box::new(RecordingStage {
            info: ModelInfoData::new(name, source, true, true, Duration::from_secs(1)),
            reconfigured: Arc::clone(&reconfigured),
        }),
    };
    (loaded, reconfigured)
}

/// The prompt header one stage was handed on its most recent reconfigure, or
/// `None` when it has not been reconfigured or was handed no prompt.
fn last_prompt(seen: &SeenContexts) -> Option<String> {
    let seen = seen.lock().unwrap();
    let context = seen.last()?;
    context
        .headers
        .iter()
        .find(|(k, _)| k == "x-stt-prompt")
        .map(|(_, v)| v.clone())
}

/// Seed both stages from *different* backends and put a context in force.
async fn two_stages_on_two_backends(
    daemon: &SuperSTTDaemon,
) -> (&'static str, &'static str, SeenContexts, SeenContexts) {
    let transcription = "github.com/x/whisper";
    let processor = "github.com/x/tidy";
    *daemon.backends.write().await =
        vec![context_capable(transcription), context_capable(processor)];

    let (stage1, seen1) = stage("whisper-1", transcription, ModelRole::Transcription);
    let (stage2, seen2) = stage("tidy", processor, ModelRole::PostProcessor);
    *daemon.model.write().await = Some(stage1);
    *daemon.post_processor.write().await = Some(stage2);

    (transcription, processor, seen1, seen2)
}

/// The case the `source`-keyed reconfigure helper gets wrong.
///
/// It takes one `source`, resolves one `BackendContext` from it, and hands that
/// same object to both slots — so it only ever does the right thing because it
/// returns early unless the source it was given is loaded. A context switch is
/// global and the two stages may run different backends, which is exactly the
/// shape that helper cannot express.
#[tokio::test]
async fn switching_the_active_context_reaches_both_stages_on_different_backends() {
    let daemon = test_daemon().await;
    let (_t, _p, seen1, seen2) = two_stages_on_two_backends(&daemon).await;
    daemon
        .handle_set_context(DictationContext {
            id: "coding".to_string(),
            name: "Coding".to_string(),
            prompt: "I dictate code.".to_string(),
            vocabulary: vec!["kubectl".to_string()],
        })
        .await;

    let resp = daemon
        .handle_set_active_context(Some("coding".to_string()))
        .await;

    assert_eq!(resp.status, "success");
    let message = resp.message.unwrap_or_default();
    assert!(
        !message.contains("kept the old value"),
        "both stages were reachable, so nothing should be warned about: {message}"
    );
    assert_eq!(
        last_prompt(&seen1).as_deref(),
        Some(r#""I dictate code.""#),
        "stage 1 was not handed the new context"
    );
    assert_eq!(
        last_prompt(&seen2).as_deref(),
        Some(r#""I dictate code.""#),
        "stage 2 runs a different backend and was left behind"
    );
}

/// Editing a context in place is a change to what both stages are sent, the
/// same as switching to another one.
#[tokio::test]
async fn editing_the_active_context_reaches_both_stages() {
    let daemon = test_daemon().await;
    let (_t, _p, seen1, seen2) = two_stages_on_two_backends(&daemon).await;
    let mut context = DictationContext {
        id: "coding".to_string(),
        name: "Coding".to_string(),
        prompt: "I dictate code.".to_string(),
        vocabulary: Vec::new(),
    };
    daemon.handle_set_context(context.clone()).await;
    daemon
        .handle_set_active_context(Some("coding".to_string()))
        .await;

    context.prompt = "I dictate prose.".to_string();
    daemon.handle_set_context(context).await;

    for (stage, seen) in [("stage 1", &seen1), ("stage 2", &seen2)] {
        assert_eq!(
            last_prompt(seen).as_deref(),
            Some(r#""I dictate prose.""#),
            "{stage} is still on the old text"
        );
    }
}

/// A pin moves one backend and leaves the other where it was. Both stages are
/// running, so a helper that reconfigured everything on every write would pass
/// the first assertion and fail the second.
#[tokio::test]
async fn pinning_one_backend_leaves_the_other_stage_alone() {
    let daemon = test_daemon().await;
    let (transcription, _p, seen1, seen2) = two_stages_on_two_backends(&daemon).await;
    daemon
        .handle_set_context(DictationContext {
            id: "coding".to_string(),
            name: "Coding".to_string(),
            prompt: "I dictate code.".to_string(),
            vocabulary: Vec::new(),
        })
        .await;
    daemon
        .handle_set_context(DictationContext {
            id: "email".to_string(),
            name: "Email".to_string(),
            prompt: "I dictate email.".to_string(),
            vocabulary: Vec::new(),
        })
        .await;
    daemon
        .handle_set_active_context(Some("coding".to_string()))
        .await;
    let before = seen2.lock().unwrap().len();

    daemon
        .handle_set_backend_context(transcription.to_string(), Some("email".to_string()))
        .await;

    assert_eq!(
        last_prompt(&seen1).as_deref(),
        Some(r#""I dictate email.""#),
        "the pinned backend takes what it was pinned to"
    );
    assert_eq!(
        seen2.lock().unwrap().len(),
        before,
        "the other stage was not pinned and should not have been touched"
    );
    assert_eq!(
        last_prompt(&seen2).as_deref(),
        Some(r#""I dictate code.""#),
        "and still follows the active context"
    );
}

/// Pinning a backend to no context takes the headers away, rather than leaving
/// the last one in place.
#[tokio::test]
async fn sending_a_backend_no_context_clears_what_it_was_given() {
    let daemon = test_daemon().await;
    let (transcription, _p, seen1, _seen2) = two_stages_on_two_backends(&daemon).await;
    daemon
        .handle_set_context(DictationContext {
            id: "coding".to_string(),
            name: "Coding".to_string(),
            prompt: "I dictate code.".to_string(),
            vocabulary: Vec::new(),
        })
        .await;
    daemon
        .handle_set_active_context(Some("coding".to_string()))
        .await;
    assert!(last_prompt(&seen1).is_some());

    daemon
        .handle_set_backend_context(transcription.to_string(), Some(String::new()))
        .await;

    assert_eq!(
        last_prompt(&seen1),
        None,
        "a backend sent no context is handed no prompt header at all"
    );
}
