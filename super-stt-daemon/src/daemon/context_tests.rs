// SPDX-License-Identifier: GPL-3.0-only
//! The context write verbs, at the daemon layer.
//!
//! What is checked here is the part every transport shares: what is refused,
//! what the stored file ends up holding, and which of the three per-backend
//! states a write lands in. The route wrappers are covered by the HTTP smoke
//! tests; the resolution rule itself by `config_tests`.

use crate::daemon::types::test_daemon;
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
