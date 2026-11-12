// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::device_management::PipelineStage;
use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::backends;
use log::info;
use std::path::PathBuf;

impl SuperSTTDaemon {
    /// The directory backends install into: the configured one, else the
    /// default.
    pub async fn backends_dir(&self) -> PathBuf {
        let configured = self.config.read().await.backends_dir.clone();
        configured.map_or_else(backends::default_backends_dir, PathBuf::from)
    }

    /// Re-scan the backends directory and refresh the in-memory registry of
    /// both features.
    ///
    /// The folder is read once, with every role's models: duplicates are a
    /// property of the folder, not of a feature, so they are removed once and
    /// both features' selected backends repointed. Each feature then keeps
    /// the backends that serve it, with only its own models.
    pub async fn refresh_backends(&self) {
        let dir = self.backends_dir().await;
        let (winners, losers) = backends::discover_all(&dir);
        let speech_to_text = backends::speech_to_text(&winners);
        let text_to_speech = crate::tts::tts_models::backends::synthesis(&winners);
        info!(
            "Backend registry: {} backend(s) from {}, {} for speech to text and {} for \
             text to speech",
            winners.len(),
            dir.display(),
            speech_to_text.len(),
            text_to_speech.len()
        );

        // Skipped while a session holds the switch guard or an utterance is in
        // flight: removing a directory under either would strand state it
        // still depends on, the same reason uninstall refuses. The duplicate
        // is harmless until the next refresh.
        if !losers.is_empty() {
            if self.switch_guard().is_some() || self.tts.is_busy() {
                log::warn!(
                    "{} duplicate backend director(ies) left for a later refresh: \
                     a backend is busy",
                    losers.len()
                );
            } else {
                let bytes = crate::registry::reconcile::reconcile(self, &losers, &winners).await;
                log::info!("Reconciled duplicate backends, reclaiming {bytes} bytes");
            }
        }

        *self.backends.write().await = speech_to_text;
        *self.tts.backends.write().await = text_to_speech;
        *self.all_backends.write().await = winners;
    }

    /// The model `stage` runs: switched on, and pointed at a model that is
    /// installed. Otherwise why not, when that is worth logging. Quiet: it
    /// runs on every request that loads on demand.
    ///
    /// The daemon never picks a model of its own, since loading one can pull
    /// gigabytes: with nothing selected the stage stays empty.
    pub(crate) async fn selected_model(
        &self,
        stage: PipelineStage,
    ) -> Result<(String, String), Option<String>> {
        let (enabled, pref_model, pref_source) = {
            let c = self.config.read().await;
            match stage {
                PipelineStage::Transcription => (
                    c.transcription.enabled,
                    c.transcription.preferred_model.clone(),
                    c.transcription.preferred_source.clone(),
                ),
                PipelineStage::PostProcessor => (
                    c.post_processor.enabled,
                    c.post_processor.model.clone(),
                    c.post_processor.source.clone(),
                ),
            }
        };
        if pref_model.is_empty() {
            return Err(None);
        }
        let position = stage.position();
        // A stage remembers its model across an unload, so the selection
        // alone does not mean "load this": the flag does. Checked after the
        // empty case, so an idle daemon does not log a line naming no model.
        if !enabled {
            return Err(Some(format!(
                "Stage {position} is switched off; staying idle with {pref_model} still selected"
            )));
        }
        // A config predating `preferred_source` names a model but not the
        // backend serving it. Resolve it the same way the wire path does — from
        // the backend selected for the stage — rather than scanning for the
        // first backend that serves the name: two backends may serve the same
        // name, and the scan order is `read_dir` order, so the daemon could
        // come up on a different engine than the one the user chose. With no
        // backend selected either, stay idle and let the user pick.
        let pref_source = if pref_source.is_empty() {
            let Some(resolved) = self.stage_source(stage).await else {
                return Err(Some(format!(
                    "Stage {position} model {pref_model} names no source and no backend is \
                     selected; staying idle"
                )));
            };
            resolved
        } else {
            pref_source
        };
        let backends = self.backends.read().await;
        let (_, def) = backends::find_model(&backends, &pref_model, &pref_source).ok_or(None)?;
        // A required secret is enforced at load, not here.
        Ok((def.name.clone(), def.source.clone()))
    }
}

#[cfg(test)]
mod tests {
    use crate::daemon::types::test_daemon;

    /// A minimal backend manifest at `dir`, sharing `source` with whatever
    /// else is written under the same backends root, at the given `version`.
    fn write_backend(dir: &std::path::Path, version: &str) {
        write_backend_serving(dir, version, "transcription");
    }

    /// [`write_backend`], its one model in the given `role`.
    fn write_backend_serving(dir: &std::path::Path, version: &str, role: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("backend.toml"),
            format!(
                r#"
[backend]
    source     = "github.com/x/y"
    name       = "Y"
    version    = "{version}"
    kind       = "subprocess"
    entrypoint = "y"
    contract   = "v4"
    id = "app.test.y"
    product = "super-voice"
    description = "Test backend."

[[models]]
    name                = "m"
    role = "{role}"
    primary_language    = "en"
    supported_languages = ["en"]
    supported_devices   = ["cpu"]
"#
            ),
        )
        .unwrap();
    }

    /// Reconciliation must not touch the filesystem while a session holds the
    /// switch guard: removing a directory under an in-flight recording would
    /// strand state it still depends on. The duplicate stays in place — only
    /// the winner is served — until a later, idle refresh cleans it up.
    #[tokio::test]
    async fn reconciliation_is_skipped_while_a_backend_is_busy() {
        let root = tempfile::tempdir().unwrap();
        let winner = root.path().join("app.super-stt.y");
        let loser = root.path().join("super-stt-y");
        write_backend(&winner, "1.0.1");
        write_backend(&loser, "1.0.0");

        let daemon = test_daemon().await;
        // A recording under way, for as long as its claim is held.
        let (recording, _cancelled) = daemon.recording.reserve().expect("free");
        let _stop = recording.start().expect("not cancelled");
        daemon.config.write().await.backends_dir = Some(root.path().to_string_lossy().into_owned());

        daemon.refresh_backends().await;

        assert!(
            loser.exists(),
            "a duplicate must not be removed while a backend is busy"
        );
        assert!(winner.exists());
        let backends = daemon.backends.read().await;
        assert_eq!(
            backends.len(),
            1,
            "the winner is still the only one served, even though the \
             duplicate was left in place"
        );
    }

    /// The mirror case: once idle, a refresh reconciles the duplicate away.
    #[tokio::test]
    async fn reconciliation_runs_and_removes_the_loser_when_idle() {
        let root = tempfile::tempdir().unwrap();
        let winner = root.path().join("app.super-stt.y");
        let loser = root.path().join("super-stt-y");
        write_backend(&winner, "1.0.1");
        write_backend(&loser, "1.0.0");

        let daemon = test_daemon().await;
        daemon.config.write().await.backends_dir = Some(root.path().to_string_lossy().into_owned());

        daemon.refresh_backends().await;

        assert!(
            !loser.exists(),
            "an idle refresh must reconcile the duplicate away"
        );
        assert!(winner.exists());
        let backends = daemon.backends.read().await;
        assert_eq!(backends.len(), 1);
    }

    /// The upgrade path, end to end: a user who already has two directories
    /// for one source gets them reconciled on the first refresh after the
    /// daemon starts. If `active_backend` named the loser, that refresh
    /// deletes the directory the pointer names — so the refresh has to move
    /// the pointer with it, or the daemon comes up reporting no active
    /// backend and no models while the backend sits installed next door.
    #[tokio::test]
    async fn a_refresh_that_reconciles_the_active_directory_repoints_it() {
        let root = tempfile::tempdir().unwrap();
        let winner = root.path().join("app.super-stt.y");
        let loser = root.path().join("super-stt-y");
        write_backend(&winner, "1.0.1");
        write_backend(&loser, "1.0.0");

        let daemon = test_daemon().await;
        {
            let mut cfg = daemon.config.write().await;
            cfg.backends_dir = Some(root.path().to_string_lossy().into_owned());
            cfg.transcription.active_backend = Some("super-stt-y".to_string());
            cfg.update_preferred_model("m".to_string(), "github.com/x/y".to_string());
        }
        *daemon.active_backend.write().await = Some("super-stt-y".to_string());

        daemon.refresh_backends().await;

        assert!(!loser.exists());
        let cfg = daemon.config.read().await;
        assert_eq!(
            cfg.transcription.active_backend.as_deref(),
            Some("app.super-stt.y"),
            "the pointer must never be left naming a directory the refresh deleted"
        );
        assert_eq!(cfg.transcription.preferred_model, "m");
        drop(cfg);
        assert_eq!(
            daemon.active_backend.read().await.as_deref(),
            Some("app.super-stt.y")
        );
    }

    /// Text to speech holds the folder too: a duplicate is left in place while
    /// an utterance is in flight, for the same reason as a recording.
    #[tokio::test]
    async fn reconciliation_is_skipped_while_speech_is_busy() {
        let root = tempfile::tempdir().unwrap();
        let winner = root.path().join("app.super-tts.y");
        let loser = root.path().join("super-tts-y");
        write_backend_serving(&winner, "1.0.1", "synthesis");
        write_backend_serving(&loser, "1.0.0", "synthesis");

        let daemon = test_daemon().await;
        let _claim = daemon.tts.speech.claim_for_test("u-reconcile");
        daemon.config.write().await.backends_dir = Some(root.path().to_string_lossy().into_owned());

        daemon.refresh_backends().await;

        assert!(
            loser.exists(),
            "a duplicate must not be removed while speech is in flight"
        );
        assert_eq!(daemon.tts.backends.read().await.len(), 1);
    }

    /// The refresh repoints text to speech's selected backend as well as
    /// speech to text's: a reconcile that deleted the directory the synthesis
    /// pointer names would leave the feature with no backend and no models.
    #[tokio::test]
    async fn a_refresh_that_reconciles_the_active_synthesis_directory_repoints_it() {
        let root = tempfile::tempdir().unwrap();
        let winner = root.path().join("app.super-tts.y");
        let loser = root.path().join("super-tts-y");
        write_backend_serving(&winner, "1.0.1", "synthesis");
        write_backend_serving(&loser, "1.0.0", "synthesis");

        let daemon = test_daemon().await;
        {
            let mut cfg = daemon.config.write().await;
            cfg.backends_dir = Some(root.path().to_string_lossy().into_owned());
            cfg.synthesis.active_backend = Some("super-tts-y".to_string());
            cfg.update_synthesis_model("m".to_string(), "github.com/x/y".to_string());
        }
        *daemon.tts.active_backend.write().await = Some("super-tts-y".to_string());

        daemon.refresh_backends().await;

        assert!(!loser.exists());
        let cfg = daemon.config.read().await;
        assert_eq!(
            cfg.synthesis.active_backend.as_deref(),
            Some("app.super-tts.y"),
            "the pointer must never be left naming a directory the refresh deleted"
        );
        assert_eq!(cfg.synthesis.preferred_model, "m");
        drop(cfg);
        assert_eq!(
            daemon.tts.active_backend.read().await.as_deref(),
            Some("app.super-tts.y")
        );
    }
}
