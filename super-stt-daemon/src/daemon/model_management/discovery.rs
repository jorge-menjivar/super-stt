// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::backends;
use log::info;
use std::path::PathBuf;

impl SuperSTTDaemon {
    /// Re-scan the backends directory and refresh the in-memory registry.
    pub async fn refresh_backends(&self) {
        let configured = {
            let c = self.config.read().await;
            c.transcription.backends_dir.clone()
        };
        let dir = configured.map_or_else(backends::default_backends_dir, PathBuf::from);
        let (winners, losers) = backends::discover(&dir);
        info!(
            "Backend registry: {} backend(s) from {}",
            winners.len(),
            dir.display()
        );

        // Skipped while a session holds the switch guard: removing a
        // directory under an in-flight recording would strand state it
        // still depends on, the same reason uninstall refuses. The
        // duplicate is harmless until the next refresh.
        if !losers.is_empty() {
            if self.switch_guard().await.is_some() {
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

        *self.backends.write().await = winners;
    }

    /// Choose the model to load at startup: the configured preference, but only
    /// if it is installed and usable. Online models are "usable" only when the
    /// online toggle is on and a key exists. Returns `None` (daemon stays idle)
    /// when there is no preference or it can't be loaded — the daemon never
    /// auto-picks an arbitrary model, since loading one can pull gigabytes.
    pub async fn pick_startup_model(&self) -> Option<(String, String)> {
        let (pref_model, pref_source, allow_online) = {
            let c = self.config.read().await;
            (
                c.transcription.preferred_model.clone(),
                c.transcription.preferred_source.clone(),
                c.online.allow_online_models,
            )
        };
        if pref_model.is_empty() {
            return None;
        }
        // A config predating `preferred_source` names a model but not the
        // backend serving it. Resolve it the same way the wire path does — from
        // the selected backend — rather than scanning for the first backend
        // that serves the name: two backends may serve the same name, and the
        // scan order is `read_dir` order, so the daemon could come up on a
        // different engine than the one the user chose. With no selection
        // recorded either, stay idle and let the user pick.
        let pref_source = if pref_source.is_empty() {
            let Some(resolved) = self.active_backend_source().await else {
                info!(
                    "Startup model {pref_model} names no source and no backend is selected; \
                     staying idle"
                );
                return None;
            };
            resolved
        } else {
            pref_source
        };
        let backends = self.backends.read().await;
        let (_, def) = backends::find_model(&backends, &pref_model, &pref_source)?;
        // Online models need the online gate on to be usable; local models are
        // always usable (the required secret is enforced at load).
        let usable = !def.is_online() || allow_online;
        usable.then(|| (def.name.clone(), def.source.clone()))
    }

    /// First discovered local (non-online) model, if any. Used as the safe
    /// fallback when online models are turned off.
    pub async fn first_local_model(&self) -> Option<(String, String)> {
        let backends = self.backends.read().await;
        for backend in backends.iter() {
            for def in &backend.models {
                if !def.is_online() {
                    return Some((def.name.clone(), def.source.clone()));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::daemon::types::test_daemon;

    /// A minimal backend manifest at `dir`, sharing `source` with whatever
    /// else is written under the same backends root, at the given `version`.
    fn write_backend(dir: &std::path::Path, version: &str) {
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
    contract   = "v1"
    description = "Test backend."

[[models]]
    name                = "m"
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
        *daemon.busy.write().await = true;
        daemon.config.write().await.transcription.backends_dir =
            Some(root.path().to_string_lossy().into_owned());

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
        daemon.config.write().await.transcription.backends_dir =
            Some(root.path().to_string_lossy().into_owned());

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
            cfg.transcription.backends_dir = Some(root.path().to_string_lossy().into_owned());
            cfg.transcription.active_backend = Some("super-stt-y".to_string());
            cfg.update_preferred_model(
                "m".to_string(),
                "github.com/x/y".to_string(),
                Some("local_y".to_string()),
            );
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
}
