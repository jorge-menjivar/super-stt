// SPDX-License-Identifier: GPL-3.0-only
//! Startup-path tests.
//!
//! These cover the seam between "a model is loaded" and "a backend is
//! selected". The two are set by different code paths — `switch.rs` records
//! the selection when a user picks a model, while the startup path resolves
//! its model from the legacy `preferred_model`/`preferred_provider`/
//! `preferred_source` config, which carries no `active_backend` at all. When
//! the startup path forgets to record it the daemon still transcribes, so
//! nothing fails and no log line complains; the only symptom is that the
//! settings app renders its "no backend loaded" empty state and the model
//! picker comes back empty.

use crate::daemon::types::test_daemon;
use crate::stt_models::backends::DiscoveredBackend;
use std::path::PathBuf;

const WHISPER_SOURCE: &str = "github.com/jorge-menjivar/super-stt-whisper";

/// A discovered backend installed at `<backends_dir>/<dir>`, serving `source`.
/// Only `dir` and `source` matter here: the adoption looks a backend up by
/// `source` and stores its install-dir name.
fn discovered(dir: &str, source: &str) -> DiscoveredBackend {
    DiscoveredBackend {
        dir: PathBuf::from("/var/lib/super-stt/backends").join(dir),
        source: source.to_string(),
        name: "Whisper (local)".to_string(),
        kind: "subprocess".to_string(),
        entrypoint: "whisper".to_string(),
        allowed_hosts: Vec::new(),
        secrets: Vec::new(),
        options: Vec::new(),
        models: Vec::new(),
    }
}

/// The regression: a daemon that loads its model from a legacy config must end
/// up with an active backend. A config written before `active_backend` existed
/// deserializes it as `None` while still naming a model, so this is the state
/// every upgrading user starts in.
///
/// Both the runtime lock and the config are asserted — setting only the lock
/// would look correct until the next restart, when the selection would be gone
/// again.
#[tokio::test]
async fn a_startup_load_adopts_the_backend_serving_the_model() {
    let daemon = test_daemon().await;
    *daemon.backends.write().await = vec![discovered("whisper", WHISPER_SOURCE)];

    let adopted = daemon.adopt_active_backend_for(WHISPER_SOURCE).await;

    assert!(adopted, "a startup load left no backend selected");
    assert_eq!(
        daemon.active_backend.read().await.as_deref(),
        Some("whisper"),
        "runtime active_backend was not set to the install dir"
    );
    assert_eq!(
        daemon
            .config
            .read()
            .await
            .transcription
            .active_backend
            .as_deref(),
        Some("whisper"),
        "active_backend was not recorded in config, so it would not survive a restart"
    );
}

/// The stored value is the backend's **install-dir name**, not its `source`.
/// The daemon keys `active_backend` by directory everywhere (`handle_list_models`,
/// `active_backend_payload`) and converts to a source only on the wire, so
/// storing a source here would resolve to nothing and reproduce the same empty
/// UI this fix exists to prevent.
#[tokio::test]
async fn the_adopted_value_is_the_install_dir_not_the_source() {
    let daemon = test_daemon().await;
    *daemon.backends.write().await = vec![discovered("whisper", WHISPER_SOURCE)];

    daemon.adopt_active_backend_for(WHISPER_SOURCE).await;

    let active = daemon.active_backend.read().await.clone();
    assert_eq!(active.as_deref(), Some("whisper"));
    assert_ne!(
        active.as_deref(),
        Some(WHISPER_SOURCE),
        "stored the source where a directory name is expected"
    );
}

/// Adoption fills a gap; it never overrides. A user who selected a backend
/// explicitly has that recorded already, and a startup load of some other
/// backend's model must not silently repoint the selection.
#[tokio::test]
async fn an_existing_selection_is_never_overridden() {
    let daemon = test_daemon().await;
    *daemon.backends.write().await = vec![discovered("whisper", WHISPER_SOURCE)];
    *daemon.active_backend.write().await = Some("openai".to_string());

    let adopted = daemon.adopt_active_backend_for(WHISPER_SOURCE).await;

    assert!(!adopted, "reported a change it did not make");
    assert_eq!(
        daemon.active_backend.read().await.as_deref(),
        Some("openai"),
        "an explicit selection was overwritten by the startup load"
    );
}

/// A model whose backend is no longer discovered leaves the daemon idle rather
/// than inventing a selection that `handle_list_models` could not resolve.
#[tokio::test]
async fn an_undiscovered_source_adopts_nothing() {
    let daemon = test_daemon().await;
    *daemon.backends.write().await = vec![discovered("whisper", WHISPER_SOURCE)];

    let adopted = daemon
        .adopt_active_backend_for("github.com/someone/uninstalled")
        .await;

    assert!(!adopted);
    assert!(
        daemon.active_backend.read().await.is_none(),
        "selected a backend that is not installed"
    );
}
