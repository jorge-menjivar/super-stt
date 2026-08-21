// SPDX-License-Identifier: GPL-3.0-only
//! Self-update checking. Contract: docs/protocol/endpoints/v1/update.md

use std::path::PathBuf;
use super_stt_forge::{ForgeClient, Release, ReleaseAsset, ReleaseKind, RepoRef};
use super_stt_shared::models::self_update::{InstallerAsset, SelfUpdateStatus};
use super_stt_shared::models::update_beta_optin::UpdateBetaOptIn;

pub(crate) const REPO: &str = "github.com/jorge-menjivar/super-stt";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn target_triple() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64-unknown-linux-gnu"),
        "aarch64" => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

fn effective_beta_optin_for(current: &str, optin: UpdateBetaOptIn) -> bool {
    match optin {
        UpdateBetaOptIn::Enabled => true,
        UpdateBetaOptIn::Disabled => false,
        UpdateBetaOptIn::Auto => super_stt_registry_types::version::parse_version(current)
            .is_some_and(|v| !v.pre.is_empty()),
    }
}

pub(crate) fn effective_beta_optin(optin: UpdateBetaOptIn) -> bool {
    effective_beta_optin_for(CURRENT_VERSION, optin)
}

pub(crate) fn select_candidate(
    releases: &[Release],
    include_prereleases: bool,
) -> Option<&Release> {
    releases
        .iter()
        .filter(|r| match r.kind {
            ReleaseKind::Published => true,
            ReleaseKind::Prerelease => include_prereleases,
            ReleaseKind::Draft => false,
        })
        .filter_map(|r| super_stt_registry_types::version::parse_version(&r.tag).map(|v| (v, r)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, r)| r)
}

/// Find `release`'s installer asset for `triple` by exact name match. Pure
/// name-matching only — the release's `SHA256SUMS` asset isn't consulted
/// here (see [`resolve_installer_asset`]).
fn find_installer_asset<'a>(release: &'a Release, triple: &str) -> Option<&'a ReleaseAsset> {
    let want = format!("super-stt-install-{triple}");
    release.assets.iter().find(|a| a.name == want)
}

/// Build the candidate's [`InstallerAsset`], populating `sha256` from the
/// release's `SHA256SUMS` asset — downloaded via `client` (the same
/// `ForgeClient` `run_check` already used for `list_releases`), so this only
/// ever adds a second network call on the update-available path.
///
/// The outermost root-bound link (the app spawning this installer, which
/// self-escalates) must be at least as verified as the inner ones the
/// installer itself checks against its own tarball — so a caller here is
/// never handed an `InstallerAsset` without a real digest to verify against.
/// Returns `None` (logging why) rather than a digest-less asset when the
/// release has no `SHA256SUMS` asset, downloading it fails, or it doesn't
/// list the installer's exact filename; the app already renders a
/// curl-fallback caption when `installer_asset` is `None`, so this is a
/// graceful degradation, not a hard failure of the check itself.
async fn resolve_installer_asset(
    client: &dyn ForgeClient,
    release: &Release,
    triple: &str,
) -> Option<InstallerAsset> {
    let asset = find_installer_asset(release, triple)?;
    let Some(sums_asset) = release.assets.iter().find(|a| a.name == "SHA256SUMS") else {
        log::warn!(
            "release {} has installer asset {} but no SHA256SUMS asset; omitting installer_asset",
            release.tag,
            asset.name
        );
        return None;
    };
    let bytes = match client
        .download(
            &sums_asset.download_url,
            super_stt_registry_types::verify::MAX_SHA256SUMS_BYTES,
        )
        .await
    {
        Ok(b) => b,
        Err(e) => {
            log::warn!(
                "failed to download SHA256SUMS for release {}: {e}; omitting installer_asset",
                release.tag
            );
            return None;
        }
    };
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(e) => {
            log::warn!(
                "SHA256SUMS for release {} was not valid UTF-8: {e}; omitting installer_asset",
                release.tag
            );
            return None;
        }
    };
    let sums = super_stt_registry_types::verify::parse_sha256sums(&text);
    let Some((sha256, _)) = sums.into_iter().find(|(_, name)| *name == asset.name) else {
        log::warn!(
            "SHA256SUMS for release {} does not list {}; omitting installer_asset",
            release.tag,
            asset.name
        );
        return None;
    };
    Some(InstallerAsset {
        name: asset.name.clone(),
        url: asset.download_url.clone(),
        size: asset.size,
        sha256,
    })
}

/// On-disk shape of the notify-once state: the last release tag a desktop
/// notification was already sent for. A missing or corrupt file reads as "no
/// version notified yet" — losing this file must never suppress a real
/// notification, only (at worst) repeat one.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct NotifyState {
    #[serde(default)]
    last_notified_version: Option<String>,
}

/// Read the notify-state file leniently: missing or corrupt reads as
/// `NotifyState::default()` (no version recorded). Runs on a blocking thread
/// since it's plain `std::fs`, mirroring `registry::client::load_from_disk`.
async fn read_notify_state(path: &std::path::Path) -> NotifyState {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// Internal checker state: the last completed check's wire-visible result,
/// plus the effective beta opt-in its candidate fields were resolved under.
///
/// `resolved_include_pre` is deliberately NOT part of [`SelfUpdateStatus`] —
/// that struct is a documented wire contract (`docs/protocol/endpoints/v1/update.md`)
/// and gains no new field for this. It exists purely so a subsequent
/// *failed* check can tell whether the cached candidate is still valid for
/// its channel (same effective beta opt-in) or must be cleared because the
/// channel changed underneath it — e.g. the user toggled beta off and the
/// app chained an immediate `CheckNow` that then hit a network error. See
/// `run_check`'s `Err` arm.
struct CheckerState {
    status: SelfUpdateStatus,
    /// `include_pre` under which `status`'s candidate fields
    /// (`latest_version`/`update_available`/`installer_asset`) were last
    /// resolved by a SUCCESSFUL check. `None` before any check has ever
    /// succeeded.
    resolved_include_pre: Option<bool>,
}

/// Self-update check state: the last completed check's result, plus
/// notify-once persistence so a restart doesn't re-notify for a version the
/// user has already been told about.
pub struct SelfUpdateChecker {
    state: tokio::sync::RwLock<CheckerState>,
    // Serializes concurrent `run_check` calls: `POST /update/check`'s
    // contract is that a caller arriving mid-check waits for it and gets the
    // same fresh result rather than starting a second one. `run_check`
    // pairs this with `generation` to turn "waits" into "coalesces onto
    // the in-flight check's result" instead of "waits, then makes its own
    // second network call".
    check_lock: tokio::sync::Mutex<()>,
    // Bumped once per completed check (success or failure). A caller
    // snapshots this before locking; if it has moved by the time the lock
    // is acquired, some other caller's check completed in the meantime, so
    // this caller returns that fresh state instead of checking again.
    generation: std::sync::atomic::AtomicU64,
    notify_path: PathBuf,
}

impl SelfUpdateChecker {
    #[must_use]
    pub fn new(notify_path: PathBuf) -> Self {
        Self {
            state: tokio::sync::RwLock::new(CheckerState {
                status: SelfUpdateStatus {
                    current_version: CURRENT_VERSION.to_string(),
                    latest_version: None,
                    update_available: false,
                    checked_at: None,
                    last_check_error: None,
                    beta_optin_effective: false,
                    installer_asset: None,
                },
                resolved_include_pre: None,
            }),
            check_lock: tokio::sync::Mutex::new(()),
            generation: std::sync::atomic::AtomicU64::new(0),
            notify_path,
        }
    }

    /// The last completed check's result — a read-only snapshot. Never
    /// triggers a new check.
    pub async fn status(&self) -> SelfUpdateStatus {
        self.state.read().await.status.clone()
    }

    /// Run a check against `client`, updating and returning the new status
    /// paired with whether *this call* actually performed the network
    /// check (`true`) versus coalesced onto another caller's check
    /// (`false`, see below). A network failure records the error in
    /// `last_check_error` and keeps the previous successful result's
    /// `latest_version`/`update_available`/`installer_asset` — but only
    /// when this call's effective beta opt-in matches the one that
    /// candidate was last resolved under; if the channel changed (e.g. the
    /// user just toggled beta off) those fields are cleared instead of
    /// carrying forward a candidate from the other channel (F1). Either
    /// way `beta_optin_effective` always reflects *this* call's opt-in.
    ///
    /// When an update is found, a *second* request fetches the release's
    /// `SHA256SUMS` asset via the same `client` so `installer_asset.sha256`
    /// is populated ([`resolve_installer_asset`]) — only on this
    /// update-available path, never on every check.
    ///
    /// Concurrent callers coalesce onto a single in-flight check: a caller
    /// that arrives while another is already running waits for it on
    /// `check_lock`, then — since `generation` will have moved — returns
    /// that check's fresh result rather than making its own network call
    /// (contract: `docs/protocol/endpoints/v1/update/check.md`). Callers
    /// with side effects gated on "a new result showed up" (event publish,
    /// notify-once) must key that on the returned `bool`, not merely on the
    /// status — two overlapping calls both see a stale "before" snapshot
    /// and would otherwise both fire those side effects for the coalesced
    /// call too (task review round 1, Important finding).
    ///
    /// # Panics
    /// Never in practice: `REPO` is a hardcoded, valid
    /// `<host>/<owner>/<repo>` reference.
    pub async fn run_check(
        &self,
        client: &dyn ForgeClient,
        optin: UpdateBetaOptIn,
    ) -> (SelfUpdateStatus, bool) {
        use std::sync::atomic::Ordering;

        let before_generation = self.generation.load(Ordering::SeqCst);
        let _guard = self.check_lock.lock().await;
        if self.generation.load(Ordering::SeqCst) != before_generation {
            // Someone else's check completed while we waited for the lock:
            // reuse it instead of starting a second one. The returned
            // state's `beta_optin_effective` reflects *their* `optin`, not
            // necessarily ours, if the two differed — the doc's "same
            // fresh result" contract blesses this: coalescing means
            // genuinely sharing the one check that ran.
            return (self.state.read().await.status.clone(), false);
        }

        let include_pre = effective_beta_optin(optin);
        let repo = RepoRef::parse(REPO).expect("static repo ref");
        let result = client.list_releases(&repo).await;
        let checked_at = Some(chrono::Utc::now().to_rfc3339());

        let new_checker_state = match result {
            Ok(releases) => {
                let candidate = select_candidate(&releases, include_pre);
                let update_available = candidate.is_some_and(|c| {
                    super_stt_registry_types::version::update_available(CURRENT_VERSION, &c.tag)
                });
                let installer = match (update_available, candidate, target_triple()) {
                    (true, Some(c), Some(t)) => resolve_installer_asset(client, c, t).await,
                    _ => None,
                };
                CheckerState {
                    status: SelfUpdateStatus {
                        current_version: CURRENT_VERSION.to_string(),
                        latest_version: candidate.map(|c| c.tag.clone()),
                        update_available,
                        checked_at,
                        last_check_error: None,
                        beta_optin_effective: include_pre,
                        installer_asset: installer,
                    },
                    resolved_include_pre: Some(include_pre),
                }
            }
            Err(e) => {
                let prev = self.state.read().await;
                // Only carry the cached candidate forward when this call's
                // channel matches the one it was last resolved under —
                // otherwise (e.g. the user just flipped beta off) it is a
                // stale answer for a different channel and must be cleared
                // rather than reported alongside the fresh
                // `beta_optin_effective` (F1).
                let same_channel = prev.resolved_include_pre == Some(include_pre);
                let (latest_version, update_available, installer_asset) = if same_channel {
                    (
                        prev.status.latest_version.clone(),
                        prev.status.update_available,
                        prev.status.installer_asset.clone(),
                    )
                } else {
                    (None, false, None)
                };
                // A failed check never establishes a new resolved channel:
                // keep whatever the last success recorded (or `None` if
                // there never was one), so a *later* failed check still
                // compares against the last real success.
                let resolved_include_pre = prev.resolved_include_pre;
                CheckerState {
                    status: SelfUpdateStatus {
                        current_version: CURRENT_VERSION.to_string(),
                        latest_version,
                        update_available,
                        checked_at,
                        last_check_error: Some(e.to_string()),
                        beta_optin_effective: include_pre,
                        installer_asset,
                    },
                    resolved_include_pre,
                }
            }
        };

        let new_status = new_checker_state.status.clone();
        *self.state.write().await = new_checker_state;
        self.generation.fetch_add(1, Ordering::SeqCst);
        (new_status, true)
    }

    /// Whether a desktop notification for `tag` has not yet been sent.
    pub async fn should_notify(&self, tag: &str) -> bool {
        let state = read_notify_state(&self.notify_path).await;
        state.last_notified_version.as_deref() != Some(tag)
    }

    /// Record that a desktop notification for `tag` was sent, so a later
    /// check for the same tag doesn't notify again — including across a
    /// daemon restart, since this persists to `notify_path`.
    pub async fn record_notified(&self, tag: &str) {
        let path = self.notify_path.clone();
        let state = NotifyState {
            last_notified_version: Some(tag.to_string()),
        };
        let outcome = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let bytes = serde_json::to_vec(&state).unwrap_or_default();
            super_stt_registry_types::fs::write_atomic(&path, &bytes)
        })
        .await;
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::warn!("failed to persist self-update notify state: {e}"),
            Err(e) => log::warn!("self-update notify state task panicked: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use super_stt_forge::{Release, ReleaseAsset, ReleaseKind};
    use super_stt_shared::models::update_beta_optin::UpdateBetaOptIn;

    fn rel(tag: &str, kind: ReleaseKind) -> Release {
        Release {
            tag: tag.into(),
            kind,
            assets: vec![],
        }
    }

    fn test_notify_path() -> PathBuf {
        // Unique per test AND per pid: parallel tests must not share state.
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "sstt-update-notify-{}-{n}.json",
            std::process::id()
        ))
    }

    #[test]
    fn candidate_is_highest_stable_when_prereleases_excluded() {
        let releases = vec![
            rel("v0.2.3-beta.1", ReleaseKind::Prerelease),
            rel("v0.2.1", ReleaseKind::Published),
            rel("v0.2.2", ReleaseKind::Published),
        ];
        assert_eq!(select_candidate(&releases, false).unwrap().tag, "v0.2.2");
    }

    #[test]
    fn candidate_includes_prereleases_when_opted_in() {
        let releases = vec![
            rel("v0.2.3-beta.1", ReleaseKind::Prerelease),
            rel("v0.2.2", ReleaseKind::Published),
        ];
        assert_eq!(
            select_candidate(&releases, true).unwrap().tag,
            "v0.2.3-beta.1"
        );
    }

    #[test]
    fn drafts_and_malformed_tags_never_win() {
        let releases = vec![
            rel("v9.9.9", ReleaseKind::Draft),
            rel("nightly", ReleaseKind::Published),
            rel("v0.2.2", ReleaseKind::Published),
        ];
        assert_eq!(select_candidate(&releases, true).unwrap().tag, "v0.2.2");
    }

    #[test]
    fn no_candidate_from_empty_or_all_filtered() {
        assert!(select_candidate(&[], true).is_none());
        let only_pre = vec![rel("v0.3.0-beta.1", ReleaseKind::Prerelease)];
        assert!(select_candidate(&only_pre, false).is_none());
    }

    #[test]
    fn effective_optin_matrix() {
        use UpdateBetaOptIn::*;
        assert!(effective_beta_optin_for("0.2.2-beta.2", Auto));
        assert!(!effective_beta_optin_for("0.2.2", Auto));
        assert!(effective_beta_optin_for("0.2.2", Enabled));
        assert!(!effective_beta_optin_for("0.2.2-beta.2", Disabled));
        assert!(!effective_beta_optin_for("garbage", Auto)); // unparsable = stable
    }

    #[test]
    fn find_installer_asset_picked_by_exact_name() {
        let mut r = rel("v0.3.0", ReleaseKind::Published);
        r.assets = vec![
            ReleaseAsset {
                name: "super-stt-x86_64-unknown-linux-gnu.tar.gz".into(),
                download_url: "https://dl/t".into(),
                size: 1,
            },
            ReleaseAsset {
                name: "super-stt-install-x86_64-unknown-linux-gnu".into(),
                download_url: "https://dl/i".into(),
                size: 2,
            },
        ];
        let asset = find_installer_asset(&r, "x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(asset.download_url, "https://dl/i");
        assert!(find_installer_asset(&r, "aarch64-unknown-linux-gnu").is_none());
    }

    /// Two installer assets in the same release for different triples: the
    /// exact-name match must pick the one for the *requested* triple, not
    /// whichever the iterator sees first — proving this is a real `==`
    /// match rather than a looser `contains`/`starts_with` that could
    /// cross-match one triple onto another entirely.
    #[test]
    fn find_installer_asset_does_not_cross_match_other_triples() {
        let mut r = rel("v0.3.0", ReleaseKind::Published);
        r.assets = vec![
            ReleaseAsset {
                name: "super-stt-install-x86_64-unknown-linux-gnu".into(),
                download_url: "https://dl/x86".into(),
                size: 1,
            },
            ReleaseAsset {
                name: "super-stt-install-aarch64-unknown-linux-gnu".into(),
                download_url: "https://dl/aarch64".into(),
                size: 2,
            },
        ];
        assert_eq!(
            find_installer_asset(&r, "aarch64-unknown-linux-gnu")
                .unwrap()
                .download_url,
            "https://dl/aarch64"
        );
        assert_eq!(
            find_installer_asset(&r, "x86_64-unknown-linux-gnu")
                .unwrap()
                .download_url,
            "https://dl/x86"
        );
    }

    /// A release with no `SHA256SUMS` asset at all: `installer_asset` must
    /// degrade to `None` rather than publish a digest-less asset — no
    /// network call happens since `find_installer_asset` never gets that
    /// far, so a deliberately-unreachable base URL proves no request occurs.
    #[tokio::test]
    async fn resolve_installer_asset_none_without_sums_asset() {
        crate::install_crypto_provider();
        let mut r = rel("v0.3.0", ReleaseKind::Published);
        r.assets = vec![ReleaseAsset {
            name: "super-stt-install-x86_64-unknown-linux-gnu".into(),
            download_url: "https://dl/i".into(),
            size: 2,
        }];
        let gh = super_stt_forge::Github::new("http://127.0.0.1:1", None);
        assert!(
            resolve_installer_asset(&gh, &r, "x86_64-unknown-linux-gnu")
                .await
                .is_none()
        );
    }

    /// The `SHA256SUMS` asset exists but its download fails (a 404 here,
    /// standing in for any transport/HTTP failure): degrade to `None`.
    #[tokio::test]
    async fn resolve_installer_asset_none_when_sums_download_fails() {
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/sums").with_status(404).create_async().await;
        let mut r = rel("v0.3.0", ReleaseKind::Published);
        r.assets = vec![
            ReleaseAsset {
                name: "super-stt-install-x86_64-unknown-linux-gnu".into(),
                download_url: "https://dl/i".into(),
                size: 2,
            },
            ReleaseAsset {
                name: "SHA256SUMS".into(),
                download_url: format!("{}/sums", s.url()),
                size: 10,
            },
        ];
        let gh = super_stt_forge::Github::new(s.url(), None);
        assert!(
            resolve_installer_asset(&gh, &r, "x86_64-unknown-linux-gnu")
                .await
                .is_none()
        );
    }

    /// The `SHA256SUMS` listing downloads fine but doesn't list the
    /// installer's exact filename: degrade to `None`.
    #[tokio::test]
    async fn resolve_installer_asset_none_when_entry_missing() {
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/sums")
            .with_status(200)
            .with_body(format!("{}  some-other-file.tar.gz\n", "b".repeat(64)))
            .create_async()
            .await;
        let mut r = rel("v0.3.0", ReleaseKind::Published);
        r.assets = vec![
            ReleaseAsset {
                name: "super-stt-install-x86_64-unknown-linux-gnu".into(),
                download_url: "https://dl/i".into(),
                size: 2,
            },
            ReleaseAsset {
                name: "SHA256SUMS".into(),
                download_url: format!("{}/sums", s.url()),
                size: 10,
            },
        ];
        let gh = super_stt_forge::Github::new(s.url(), None);
        assert!(
            resolve_installer_asset(&gh, &r, "x86_64-unknown-linux-gnu")
                .await
                .is_none()
        );
    }

    /// The happy path: the `SHA256SUMS` listing is fetched and the entry
    /// matching the installer's filename populates `sha256`.
    #[tokio::test]
    async fn resolve_installer_asset_populates_sha256_from_sums_listing() {
        crate::install_crypto_provider();
        let target_digest = "deadbeef".repeat(8); // 64 hex chars
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/sums")
            .with_status(200)
            .with_body(format!(
                "{target_digest}  super-stt-install-x86_64-unknown-linux-gnu\n\
                 111111  some-other-file.tar.gz\n",
            ))
            .create_async()
            .await;
        let mut r = rel("v0.3.0", ReleaseKind::Published);
        r.assets = vec![
            ReleaseAsset {
                name: "super-stt-install-x86_64-unknown-linux-gnu".into(),
                download_url: "https://dl/i".into(),
                size: 2,
            },
            ReleaseAsset {
                name: "SHA256SUMS".into(),
                download_url: format!("{}/sums", s.url()),
                size: 10,
            },
        ];
        let gh = super_stt_forge::Github::new(s.url(), None);
        let asset = resolve_installer_asset(&gh, &r, "x86_64-unknown-linux-gnu")
            .await
            .unwrap();
        assert_eq!(asset.url, "https://dl/i");
        assert_eq!(asset.sha256, target_digest);
    }

    /// A `SHA256SUMS` entry for the installer's exact filename exists, but
    /// its digest isn't shaped like a real SHA-256 hex string (too short
    /// here): F6's shape guard skips it, so this is indistinguishable from
    /// "entry missing" and degrades to `None` — never surfaced as a
    /// mismatch at apply time.
    #[tokio::test]
    async fn resolve_installer_asset_none_when_entry_has_malformed_hex() {
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/sums")
            .with_status(200)
            .with_body("not-a-real-digest  super-stt-install-x86_64-unknown-linux-gnu\n")
            .create_async()
            .await;
        let mut r = rel("v0.3.0", ReleaseKind::Published);
        r.assets = vec![
            ReleaseAsset {
                name: "super-stt-install-x86_64-unknown-linux-gnu".into(),
                download_url: "https://dl/i".into(),
                size: 2,
            },
            ReleaseAsset {
                name: "SHA256SUMS".into(),
                download_url: format!("{}/sums", s.url()),
                size: 10,
            },
        ];
        let gh = super_stt_forge::Github::new(s.url(), None);
        assert!(
            resolve_installer_asset(&gh, &r, "x86_64-unknown-linux-gnu")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn run_check_success_and_failure_paths() {
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/repos/jorge-menjivar/super-stt/releases?per_page=100")
            .with_status(200)
            .with_body(
                r#"[{"tag_name":"v99.0.0","prerelease":false,"assets":[
            {"name":"super-stt-install-x86_64-unknown-linux-gnu","browser_download_url":"https://dl/i","size":5}]}]"#,
            )
            .create_async()
            .await;
        let gh = super_stt_forge::Github::new(s.url(), None);
        let checker = SelfUpdateChecker::new(test_notify_path());
        let (st, did_check) = checker.run_check(&gh, UpdateBetaOptIn::Disabled).await;
        assert!(did_check, "uncontended call must perform its own check");
        assert!(st.update_available);
        assert_eq!(st.latest_version.as_deref(), Some("v99.0.0"));
        assert!(st.checked_at.is_some());
        assert!(st.last_check_error.is_none());

        // Network failure: previous result survives, error recorded.
        drop(s); // server gone -> request fails
        let (st2, did_check2) = checker.run_check(&gh, UpdateBetaOptIn::Disabled).await;
        assert!(did_check2, "uncontended call must perform its own check");
        assert_eq!(st2.latest_version.as_deref(), Some("v99.0.0"));
        assert!(st2.update_available);
        assert!(st2.last_check_error.is_some());
    }

    /// End-to-end wiring: a release carrying BOTH the installer binary asset
    /// and a `SHA256SUMS` asset whose body lists it must come out of
    /// `run_check` with `installer_asset` fully populated (name, url, size,
    /// sha256) — not just the unit-level `resolve_installer_asset`, which
    /// doesn't prove `run_check` actually calls it with the right release
    /// and target triple.
    #[tokio::test]
    async fn run_check_populates_installer_asset_end_to_end() {
        crate::install_crypto_provider();
        let digest = "c".repeat(64);
        let mut s = mockito::Server::new_async().await;
        let sums_url = format!("{}/sums", s.url());
        s.mock("GET", "/sums")
            .with_status(200)
            .with_body(format!(
                "{digest}  super-stt-install-x86_64-unknown-linux-gnu\n"
            ))
            .create_async()
            .await;
        s.mock("GET", "/repos/jorge-menjivar/super-stt/releases?per_page=100")
            .with_status(200)
            .with_body(format!(
                r#"[{{"tag_name":"v100.0.0","prerelease":false,"assets":[
                {{"name":"super-stt-install-x86_64-unknown-linux-gnu","browser_download_url":"https://dl/i","size":42}},
                {{"name":"SHA256SUMS","browser_download_url":"{sums_url}","size":10}}]}}]"#
            ))
            .create_async()
            .await;
        let gh = super_stt_forge::Github::new(s.url(), None);
        let checker = SelfUpdateChecker::new(test_notify_path());
        let (st, did_check) = checker.run_check(&gh, UpdateBetaOptIn::Disabled).await;
        assert!(did_check);
        assert!(st.update_available);
        let asset = st
            .installer_asset
            .expect("installer_asset must be populated end-to-end");
        assert_eq!(asset.name, "super-stt-install-x86_64-unknown-linux-gnu");
        assert_eq!(asset.url, "https://dl/i");
        assert_eq!(asset.size, 42);
        assert_eq!(asset.sha256, digest);
    }

    /// A failed check preserves the previous successful candidate only
    /// while the caller's effective beta opt-in matches the one the
    /// candidate was resolved under. When the channel changes (beta
    /// toggled off) underneath a still-live beta candidate, a subsequent
    /// failed check must clear it rather than report a mismatched snapshot
    /// (F1: the app chaining a `CheckNow` after flipping beta off must not
    /// see a stale beta candidate alongside `beta_optin_effective: false`).
    #[tokio::test]
    async fn run_check_clears_stale_candidate_when_channel_changes_on_failure() {
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock(
            "GET",
            "/repos/jorge-menjivar/super-stt/releases?per_page=100",
        )
        .with_status(200)
        .with_body(r#"[{"tag_name":"v0.3.0-beta.1","prerelease":true,"assets":[]}]"#)
        .create_async()
        .await;
        let gh = super_stt_forge::Github::new(s.url(), None);
        let checker = SelfUpdateChecker::new(test_notify_path());

        // Succeed with beta ON: resolves a beta candidate.
        let (st1, did_check1) = checker.run_check(&gh, UpdateBetaOptIn::Enabled).await;
        assert!(did_check1);
        assert_eq!(st1.latest_version.as_deref(), Some("v0.3.0-beta.1"));
        assert!(st1.update_available);
        assert!(st1.beta_optin_effective);

        // Server gone (network failure) AND the caller now passes beta OFF
        // — the channel the cached candidate was resolved under no longer
        // matches this call's.
        drop(s);
        let (st2, did_check2) = checker.run_check(&gh, UpdateBetaOptIn::Disabled).await;
        assert!(did_check2);
        assert!(
            st2.latest_version.is_none(),
            "a channel change must clear the stale candidate, not carry it forward"
        );
        assert!(!st2.update_available);
        assert!(st2.installer_asset.is_none());
        assert!(!st2.beta_optin_effective);
        assert!(st2.last_check_error.is_some());
        assert!(st2.checked_at.is_some());
    }

    /// A caller that arrives while a check is already in flight must not
    /// make its own second network call — it waits and gets the same fresh
    /// result (contract: docs/protocol/endpoints/v1/update/check.md).
    /// `.expect(1)` records the hit count; `mock.assert_async()` is what
    /// actually panics if it isn't exactly 1. `tokio::join!` polls both
    /// `run_check` futures on the same task so the second is genuinely
    /// still waiting when the first's network `.await` is in flight, rather
    /// than running after it has finished.
    #[tokio::test]
    async fn concurrent_run_check_coalesces_into_one_network_call() {
        crate::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        let mock = s
            .mock(
                "GET",
                "/repos/jorge-menjivar/super-stt/releases?per_page=100",
            )
            .with_status(200)
            .with_body(r#"[{"tag_name":"v42.0.0","prerelease":false,"assets":[]}]"#)
            .expect(1)
            .create_async()
            .await;
        let gh = super_stt_forge::Github::new(s.url(), None);
        let checker = SelfUpdateChecker::new(test_notify_path());

        let ((a, a_did_check), (b, b_did_check)) = tokio::join!(
            checker.run_check(&gh, UpdateBetaOptIn::Disabled),
            checker.run_check(&gh, UpdateBetaOptIn::Disabled),
        );

        assert_eq!(a.latest_version.as_deref(), Some("v42.0.0"));
        assert_eq!(a, b, "the coalesced caller must get the same fresh result");
        assert_ne!(
            a_did_check, b_did_check,
            "exactly one of the two overlapping calls must have performed the check"
        );
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn notify_once_per_version_persists() {
        let path = test_notify_path();
        let c = SelfUpdateChecker::new(path.clone());
        assert!(c.should_notify("v0.3.0").await);
        c.record_notified("v0.3.0").await;
        assert!(!c.should_notify("v0.3.0").await);
        assert!(c.should_notify("v0.3.1").await);
        // A fresh checker on the same path (daemon restart) still remembers.
        let c2 = SelfUpdateChecker::new(path);
        assert!(!c2.should_notify("v0.3.0").await);
    }
}
