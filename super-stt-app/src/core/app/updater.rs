// SPDX-License-Identifier: GPL-3.0-only
//! Apply flow: download the installer asset, spawn it with
//! `--non-interactive --json-progress`, and stream its JSON progress lines
//! back as [`UpdateRunEvent`]s.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use futures_util::SinkExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super_stt_shared::models::self_update::InstallerAsset;

/// Mirror of super-stt-install's progress.rs wire shapes. The golden tests
/// below pin both sides to the same strings — change them together.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum InstallerEvent {
    Phase {
        phase: String,
        message: String,
    },
    Progress {
        phase: String,
        bytes_done: u64,
        bytes_total: u64,
    },
    Complete {
        installed_version: String,
        components: Vec<String>,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub enum UpdateRunEvent {
    FetchProgress {
        bytes_done: u64,
        bytes_total: u64,
    },
    Installer(InstallerEvent),
    /// Spawn/download failure, app-side (never from the installer itself).
    Failed(String),
    Finished {
        exit_ok: bool,
        stderr_tail: String,
    },
}

/// Minimum byte delta between `FetchProgress` emissions during the installer
/// download, so a fast transfer doesn't flood the channel with one message
/// per `reqwest` chunk.
const PROGRESS_THROTTLE_BYTES: u64 = 256 * 1024;

/// Trailing stderr lines kept for the failure report shown on
/// `UpdateRunEvent::Finished { exit_ok: false, .. }`.
const STDERR_TAIL_LINES: usize = 30;

/// A sibling run directory older than this is swept before a new run
/// starts — see `sweep_stale_run_dirs`.
const STALE_RUN_AGE: Duration = Duration::from_hours(24);

/// Download `asset`, spawn it against `target_tag`, and stream its progress.
/// Consumed via `cosmic::task::stream(...).abortable()` — dropping the
/// returned stream (on cancel) drops the spawned `Child`, which
/// `kill_on_drop(true)` turns into an actual kill.
pub fn run_update_stream(
    asset: InstallerAsset,
    target_tag: String,
) -> impl futures_util::Stream<Item = UpdateRunEvent> {
    cosmic::iced::stream::channel(32, move |mut tx| async move {
        if let Err(message) = drive(&asset, &target_tag, &mut tx).await {
            let _ = tx.send(UpdateRunEvent::Failed(message)).await;
        }
    })
}

type Tx = cosmic::iced::futures::channel::mpsc::Sender<UpdateRunEvent>;

/// Download the installer, spawn it, and stream its stdout until it exits.
/// `Err` covers only app-side failures (download/spawn); the installer's own
/// reported failures arrive as an `InstallerEvent::Error` over `tx` and are
/// not an `Err` here — the caller still sends `Finished` for them via the
/// normal exit path.
async fn drive(asset: &InstallerAsset, target_tag: &str, tx: &mut Tx) -> Result<(), String> {
    let base = super_stt_shared::paths::cache_dir().join("self-update");
    tokio::fs::create_dir_all(&base)
        .await
        .map_err(|e| format!("create {}: {e}", base.display()))?;

    // A fresh, unpredictably-named directory per run (not `base` itself,
    // which every run shares) — see `installer_run_paths`'s doc comment for
    // why a predictable path here is a privilege-escalation TOCTOU, not just
    // a tidiness concern.
    let (run_dir, bin) = installer_run_paths(&base, &asset.name);
    sweep_stale_run_dirs(&base, &run_dir).await;
    create_run_dir(&run_dir).await?;

    download_installer(&asset.url, &bin, tx).await?;

    // The outermost root-bound link: this binary self-escalates once
    // spawned, so it must be at least as verified as the inner ones it
    // itself checks (its own tarball against the release's SHA256SUMS).
    // Verify before chmod/spawn — a mismatch must never execute.
    verify_installer_checksum(&bin, &asset.sha256).await?;

    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .await
            .map_err(|e| format!("chmod {}: {e}", bin.display()))?;
    }

    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(installer_args(target_tag));
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Cancel (Task 7.3) aborts this stream's task; the dropped future must
        // take the child with it. Only offered before the escalate phase.
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", bin.display()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "installer stdout was not captured".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "installer stderr was not captured".to_string())?;

    // Capped tail of stderr, collected on its own task so a chatty installer
    // can't block reading stdout (or vice versa).
    let stderr_task = tokio::spawn(collect_tail_lines(stderr, STDERR_TAIL_LINES));

    stream_installer_events(stdout, tx).await;

    let status = child
        .wait()
        .await
        .map_err(|e| format!("wait on installer: {e}"))?;
    let stderr_tail = stderr_task.await.unwrap_or_default().join("\n");
    let _ = tx
        .send(UpdateRunEvent::Finished {
            exit_ok: status.success(),
            stderr_tail,
        })
        .await;
    // Best-effort cleanup, only now that `child.wait()` above has actually
    // returned — the installer self-escalates by pkexec-re-execing its own
    // `current_exe()` (this same `run_dir`/`bin`), so removing it any
    // earlier would race that re-exec. A leftover directory in the cache is
    // harmless (e.g. the app may restart itself right after a successful
    // run), so a failure here is not propagated — leaking beats racing.
    let _ = tokio::fs::remove_dir_all(&run_dir).await;
    Ok(())
}

/// The installer CLI's argument list for `target_tag`: always
/// `--non-interactive --json-progress --version=<tag>`, plus `--beta` iff
/// `target_tag` is a prerelease (a semver `-<identifier>` suffix, e.g.
/// `v0.2.3-beta.1`) — the installer's own resolution must consider
/// prereleases too, or a beta target could resolve back down to the latest
/// stable release instead. Mirrors `ui/views/updates.rs::tag_is_prerelease`,
/// which uses the same rule to decide the curl-fallback caption's flag.
fn installer_args(target_tag: &str) -> Vec<String> {
    let mut args = vec![
        "--non-interactive".to_string(),
        "--json-progress".to_string(),
        format!("--version={target_tag}"),
    ];
    if target_tag.contains('-') {
        args.push("--beta".to_string());
    }
    args
}

/// Collect the trailing `cap` lines from `reader` (the installer's stderr),
/// dropping older ones as new ones arrive. Extracted from `drive` so the
/// capping behavior is exercisable against an in-memory reader in a test,
/// without spawning a real child process.
async fn collect_tail_lines<R: tokio::io::AsyncRead + Unpin>(reader: R, cap: usize) -> Vec<String> {
    let mut lines = BufReader::new(reader).lines();
    let mut tail: VecDeque<String> = VecDeque::with_capacity(cap);
    while let Ok(Some(line)) = lines.next_line().await {
        if tail.len() == cap {
            tail.pop_front();
        }
        tail.push_back(line);
    }
    Vec::from(tail)
}

/// Read newline-delimited JSON lines from `reader` (the installer's
/// stdout), parsing each as an [`InstallerEvent`] and forwarding it over
/// `tx`. Extracted from `drive` so this behavior is exercisable against an
/// in-memory reader in a test.
async fn stream_installer_events<R: tokio::io::AsyncRead + Unpin>(reader: R, tx: &mut Tx) {
    let mut lines = BufReader::new(reader).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => match serde_json::from_str::<InstallerEvent>(&line) {
                Ok(ev) => {
                    let _ = tx.send(UpdateRunEvent::Installer(ev)).await;
                }
                // Forward-compatible: an installer newer than this app may
                // emit a shape this parser doesn't know yet. Skip, don't fail
                // the run over it — subsequent, parsable lines still land.
                Err(e) => log::warn!("installer emitted an unparsable progress line: {e}: {line}"),
            },
            Ok(None) => break,
            Err(e) => {
                log::warn!("installer stdout read error: {e}");
                break;
            }
        }
    }
}

/// Build the (per-run directory, installer-binary path) for one apply-flow
/// run under `base` (`cache_dir()/self-update`, shared across runs). The
/// directory name embeds this process's pid plus 8 random bytes (via
/// `super_stt_registry_types::verify::random_hex_suffix`, the same
/// unpredictable-name pattern `super-stt-install`'s own `StagingGuard` uses
/// for its staging directory) so a same-UID process can neither predict nor
/// pre-create/race it.
///
/// This matters because the installer this app spawns self-escalates by
/// pkexec-re-execing its own `current_exe()` — which resolves to this same
/// path. A predictable path (e.g. `base` joined with the bare asset name)
/// would let another same-UID process swap the binary out between this
/// module's checksum verify and the installer's own re-exec open, turning
/// user-level code execution into root. Two calls always return different
/// paths.
fn installer_run_paths(base: &Path, asset_name: &str) -> (PathBuf, PathBuf) {
    let name = format!(
        "{}-{}",
        std::process::id(),
        super_stt_registry_types::verify::random_hex_suffix(8)
    );
    let dir = base.join(name);
    let bin = dir.join(asset_name);
    (dir, bin)
}

/// Best-effort cleanup of abandoned run directories under `base`, before
/// this run's own directory (`keep`, not yet created) is created.
///
/// Every error path in `drive` before the spawned installer exits
/// deliberately leaks its `run_dir` (see this function's caller, and the
/// tail of `drive`) rather than race the child that self-escalates by
/// re-execing that same path — correct, but it means abandoned directories
/// accumulate under `self-update/` across failed runs. This sweeps them,
/// but only ones old enough that no plausible run — this app's own
/// concurrent instance, or another install-in-progress entirely — could
/// still be using them: age-gating, not liveness-checking, is what keeps a
/// directory belonging to a live run from ever being force-removed out from
/// under it. `keep` is skipped unconditionally so this run's own
/// about-to-be-created directory is never swept, live or not.
///
/// Every error (permission, a directory vanishing mid-sweep, `read_dir`
/// itself failing) is ignored: this is tidiness, not correctness — a
/// leftover directory is harmless clutter, so failing to remove one must
/// never fail the run that triggered the sweep.
async fn sweep_stale_run_dirs(base: &Path, keep: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(base).await else {
        return;
    };
    let cutoff = SystemTime::now() - STALE_RUN_AGE;
    loop {
        let Ok(Some(entry)) = entries.next_entry().await else {
            break;
        };
        let path = entry.path();
        if path == keep {
            continue;
        }
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if modified < cutoff {
            let _ = tokio::fs::remove_dir_all(&path).await;
        }
    }
}

/// Create `dir` with mode `0700` (matching `super-stt-install`'s own staging
/// directory) off the async runtime, since `DirBuilder::create` is a
/// blocking syscall.
async fn create_run_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    let path = dir.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::DirBuilder::new().mode(0o700).create(&path))
        .await
        .map_err(|e| format!("run dir task panicked: {e}"))?
        .map_err(|e| format!("create {}: {e}", dir.display()))
}

/// Hash `bin` (off the async runtime, since hashing is sync `std::io`) and
/// compare it against `expected_hex` (case-insensitive; see
/// `super_stt_registry_types::verify::sha256_matches`). On mismatch, deletes
/// `bin` — a corrupted or tampered-with installer binary must never be
/// chmod'd executable or spawned — and returns an error describing why.
async fn verify_installer_checksum(bin: &Path, expected_hex: &str) -> Result<(), String> {
    let path = bin.to_path_buf();
    let actual = tokio::task::spawn_blocking(move || {
        super_stt_registry_types::verify::file_sha256_hex(&path)
    })
    .await
    .map_err(|e| format!("checksum task panicked: {e}"))?
    .map_err(|e| format!("hash {}: {e}", bin.display()))?;

    if super_stt_registry_types::verify::sha256_matches(&actual, expected_hex) {
        return Ok(());
    }

    if let Err(e) = tokio::fs::remove_file(bin).await {
        log::warn!(
            "failed to remove checksum-mismatched installer {}: {e}",
            bin.display()
        );
    }
    Err("installer checksum mismatch".to_string())
}

/// Stream `url` to `dest`, reporting throttled `FetchProgress` events.
/// Mirrors `super-stt-install`'s `download_to_file` chunk loop, including its
/// guaranteed final emission (see the `any_progress`/`last_reported` check
/// below) — keep the two in step per that function's own doc comment.
async fn download_installer(url: &str, dest: &Path, tx: &mut Tx) -> Result<(), String> {
    let client = super_stt_forge::http::download_client();
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download {url}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("download {url}: {e}"))?;
    let total = resp.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| format!("create {}: {e}", dest.display()))?;
    let mut done: u64 = 0;
    let mut last_reported: u64 = 0;
    let mut any_progress = false;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("download {url}: {e}"))?
    {
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("write {}: {e}", dest.display()))?;
        done += chunk.len() as u64;
        if done.saturating_sub(last_reported) >= PROGRESS_THROTTLE_BYTES || done == total {
            last_reported = done;
            any_progress = true;
            let _ = tx
                .send(UpdateRunEvent::FetchProgress {
                    bytes_done: done,
                    bytes_total: total,
                })
                .await;
        }
    }
    // C6: guarantee at least one emission even when nothing above ever
    // fired — the case for a response under the throttle threshold when the
    // server sends no `Content-Length` (`total` stays 0 for the whole
    // transfer, so `done == total` is only ever true for a zero-byte body).
    // When the true total was never known, report the actual bytes
    // downloaded as the total too, so a consumer computing a percentage
    // sees a clean "done" rather than a `bytes_done > 0` over an
    // unknown/zero total.
    if !any_progress || last_reported != done {
        let final_total = if total == 0 { done } else { total };
        let _ = tx
            .send(UpdateRunEvent::FetchProgress {
                bytes_done: done,
                bytes_total: final_total,
            })
            .await;
    }
    // Drive tokio::fs::File's pending buffered write to completion before the
    // file is dropped and the (freshly chmod'd) binary is spawned — the same
    // truncation hazard `super-stt-install::download::download_to_file`
    // guards against.
    file.flush()
        .await
        .map_err(|e| format!("flush {}: {e}", dest.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        InstallerEvent, UpdateRunEvent, collect_tail_lines, create_run_dir, download_installer,
        installer_args, installer_run_paths, stream_installer_events, sweep_stale_run_dirs,
        verify_installer_checksum,
    };
    use futures_util::StreamExt;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    /// The TOCTOU this closes: a predictable `base/<asset.name>` path would
    /// let a same-UID process swap the installer binary between this
    /// module's checksum verify and the installer's own pkexec re-exec of
    /// `current_exe()`. Two calls must never reuse a path, and the binary
    /// must never sit directly under `base`.
    #[test]
    fn installer_run_paths_are_unpredictable_and_not_a_bare_base_join() {
        let base = Path::new("/tmp/fake-self-update-base");
        let (dir1, bin1) = installer_run_paths(base, "installer-bin");
        let (dir2, bin2) = installer_run_paths(base, "installer-bin");

        assert_ne!(
            dir1, dir2,
            "two calls must not reuse the same run directory"
        );
        assert_ne!(bin1, bin2);
        assert_eq!(bin1.file_name().unwrap(), "installer-bin");
        assert_eq!(bin2.file_name().unwrap(), "installer-bin");
        // The predictable path this closes: `base` joined directly with the
        // asset name.
        assert_ne!(bin1, base.join("installer-bin"));
        assert_ne!(bin1.parent().unwrap(), base);
        assert!(dir1.starts_with(base));
        assert!(dir2.starts_with(base));
    }

    /// Back-date `dir`'s mtime by `age` (Unix: opening a directory read-only
    /// is legal, and `set_modified` on that handle is a plain `utimes`
    /// call).
    fn backdate(dir: &Path, age: Duration) {
        let f = std::fs::OpenOptions::new().read(true).open(dir).unwrap();
        f.set_modified(SystemTime::now() - age).unwrap();
    }

    /// F2: a sibling run dir older than the 24h threshold is swept, a fresh
    /// one is left alone (it could belong to another instance's live run —
    /// age, not liveness, is the only safe signal here), and the caller's
    /// own about-to-be-created directory (`keep`) is never touched even if
    /// it happens to already exist with an old mtime.
    #[tokio::test]
    async fn sweep_stale_run_dirs_removes_only_old_siblings_and_never_touches_keep() {
        let base = std::env::temp_dir().join(format!(
            "sstt-app-updater-sweep-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let old_dir = base.join("12345-deadbeef");
        std::fs::create_dir_all(&old_dir).unwrap();
        backdate(&old_dir, Duration::from_secs(25 * 60 * 60));

        let fresh_dir = base.join("12345-cafef00d");
        std::fs::create_dir_all(&fresh_dir).unwrap();

        let keep_dir = base.join("12345-keepme00");
        std::fs::create_dir_all(&keep_dir).unwrap();
        backdate(&keep_dir, Duration::from_secs(25 * 60 * 60));

        sweep_stale_run_dirs(&base, &keep_dir).await;

        assert!(!old_dir.exists(), "an old sibling run dir must be swept");
        assert!(fresh_dir.exists(), "a fresh sibling run dir must survive");
        assert!(
            keep_dir.exists(),
            "the caller's own about-to-be-created run dir must never be swept"
        );
    }

    /// A run dir younger than the threshold must never be removed, no
    /// matter how young — it may belong to another same-UID process's
    /// still-running installer.
    #[tokio::test]
    async fn sweep_stale_run_dirs_leaves_a_just_created_sibling_alone() {
        let base = std::env::temp_dir().join(format!(
            "sstt-app-updater-sweep-fresh-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let live_dir = base.join("99999-livehex01");
        std::fs::create_dir_all(&live_dir).unwrap();

        sweep_stale_run_dirs(&base, Path::new("/does/not/matter")).await;

        assert!(
            live_dir.exists(),
            "a fresh/live sibling must never be removed"
        );
    }

    /// A `base` that doesn't exist yet (first run ever) must not panic —
    /// this sweep is best-effort tidiness, called every run.
    #[tokio::test]
    async fn sweep_stale_run_dirs_tolerates_a_missing_base_dir() {
        let base = std::env::temp_dir().join(format!(
            "sstt-app-updater-sweep-missing-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        sweep_stale_run_dirs(&base, &base.join("keep")).await;
    }

    #[tokio::test]
    async fn create_run_dir_makes_a_private_0700_directory() {
        let base = std::env::temp_dir().join(format!(
            "sstt-app-updater-rundir-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let (dir, _bin) = installer_run_paths(&base, "installer-bin");

        create_run_dir(&dir).await.unwrap();

        assert!(dir.is_dir());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "run dir must be private to this user");
    }

    #[tokio::test]
    async fn verify_installer_checksum_matches_and_rejects_corruption() {
        let dir = std::env::temp_dir().join(format!(
            "sstt-app-updater-checksum-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("installer-bin");
        std::fs::write(&bin, b"hello world").unwrap();
        let good = super_stt_registry_types::verify::file_sha256_hex(&bin).unwrap();

        assert!(verify_installer_checksum(&bin, &good).await.is_ok());
        assert!(bin.exists(), "a matching checksum must not delete the file");

        // A wrong pin: the mismatch must be reported and the file removed so
        // it can never be chmod'd/spawned.
        let bad = "0".repeat(64);
        let err = verify_installer_checksum(&bin, &bad).await.unwrap_err();
        assert_eq!(err, "installer checksum mismatch");
        assert!(
            !bin.exists(),
            "a checksum mismatch must delete the downloaded file"
        );
    }

    #[test]
    fn parses_installer_golden_lines() {
        let lines = [
            r#"{"event":"phase","phase":"download","message":"downloading super-stt-x86_64-unknown-linux-gnu-beta.tar.gz"}"#,
            r#"{"event":"progress","phase":"download","bytes_done":512,"bytes_total":2048}"#,
            r#"{"event":"complete","installed_version":"v0.2.3-beta.1","components":["daemon","app"]}"#,
            r#"{"event":"error","code":"checksum_mismatch","message":"boom"}"#,
        ];
        let parsed: Vec<InstallerEvent> = lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(matches!(&parsed[0], InstallerEvent::Phase { phase, .. } if phase == "download"));
        assert!(matches!(
            &parsed[1],
            InstallerEvent::Progress {
                bytes_done: 512,
                ..
            }
        ));
        assert!(
            matches!(&parsed[2], InstallerEvent::Complete { components, .. } if components.contains(&"app".to_string()))
        );
        assert!(
            matches!(&parsed[3], InstallerEvent::Error { code, .. } if code == "checksum_mismatch")
        );
        // Unknown future phases must not break the parser.
        let future: InstallerEvent =
            serde_json::from_str(r#"{"event":"phase","phase":"defragment","message":"x"}"#)
                .unwrap();
        assert!(matches!(future, InstallerEvent::Phase { .. }));
    }

    /// F3: the documented cap — only the last `STDERR_TAIL_LINES` survive,
    /// oldest dropped first — actually holds when fed more lines than that.
    #[tokio::test]
    async fn collect_tail_lines_keeps_only_the_last_n_lines() {
        let cap = 5;
        let mut data = String::new();
        for i in 0..(cap + 3) {
            data.push_str(&format!("line-{i}\n"));
        }
        let reader = std::io::Cursor::new(data.into_bytes());

        let tail = collect_tail_lines(reader, cap).await;

        assert_eq!(tail.len(), cap, "must be capped to exactly `cap` lines");
        assert_eq!(tail, vec!["line-3", "line-4", "line-5", "line-6", "line-7"]);
    }

    #[tokio::test]
    async fn collect_tail_lines_returns_everything_when_under_the_cap() {
        let reader = std::io::Cursor::new(b"only\ntwo\n".to_vec());
        let tail = collect_tail_lines(reader, 30).await;
        assert_eq!(tail, vec!["only", "two"]);
    }

    /// F3: an unparsable stdout line is logged and skipped, not treated as a
    /// stream-ending error — lines before AND after it must still parse.
    #[tokio::test]
    async fn stream_installer_events_skips_an_unparsable_line_without_aborting() {
        let data = concat!(
            r#"{"event":"phase","phase":"resolve","message":"a"}"#,
            "\n",
            "not json at all, just installer chatter\n",
            r#"{"event":"phase","phase":"stage","message":"b"}"#,
            "\n",
        );
        let reader = std::io::Cursor::new(data.as_bytes().to_vec());
        let (mut tx, mut rx) = cosmic::iced::futures::channel::mpsc::channel::<UpdateRunEvent>(8);

        stream_installer_events(reader, &mut tx).await;
        drop(tx);

        let mut received = Vec::new();
        while let Some(ev) = rx.next().await {
            received.push(ev);
        }

        assert_eq!(
            received.len(),
            2,
            "the unparsable middle line must be skipped, not abort the stream"
        );
        assert!(
            matches!(&received[0], UpdateRunEvent::Installer(InstallerEvent::Phase { phase, .. }) if phase == "resolve")
        );
        assert!(
            matches!(&received[1], UpdateRunEvent::Installer(InstallerEvent::Phase { phase, .. }) if phase == "stage")
        );
    }

    #[tokio::test]
    async fn stream_installer_events_emits_nothing_for_an_empty_stream() {
        let reader = std::io::Cursor::new(Vec::new());
        let (mut tx, mut rx) = cosmic::iced::futures::channel::mpsc::channel::<UpdateRunEvent>(8);
        stream_installer_events(reader, &mut tx).await;
        drop(tx);
        assert!(rx.next().await.is_none());
    }

    // ---- F3: --beta argument -----------------------------------------------

    #[test]
    fn installer_args_passes_beta_only_for_a_prerelease_tag() {
        assert!(installer_args("v0.2.3-beta.1").contains(&"--beta".to_string()));
        assert!(!installer_args("v0.2.3").contains(&"--beta".to_string()));
    }

    // ---- C6: `download_installer` must guarantee a final `FetchProgress`
    // emission, mirroring `super-stt-install::download::download_to_file`'s
    // own guarantee (its doc comment says to keep the two in step). ----

    #[tokio::test]
    async fn download_installer_reports_final_progress_with_no_content_length() {
        // A chunked body (no `Content-Length`) under the throttle threshold:
        // `total` stays 0 for the whole transfer, so the old `done == total`
        // check could never fire, and nothing else in the loop would either
        // — the fetch would silently report zero progress.
        super_stt_forge::install_crypto_provider();
        let mut s = mockito::Server::new_async().await;
        s.mock("GET", "/blob")
            .with_status(200)
            .with_chunked_body(|w| w.write_all(&[3u8; 1000]))
            .create_async()
            .await;
        let dir =
            std::env::temp_dir().join(format!("sstt-app-updater-dl-nolen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("blob");

        let (mut tx, mut rx) = cosmic::iced::futures::channel::mpsc::channel::<UpdateRunEvent>(8);
        download_installer(&format!("{}/blob", s.url()), &dest, &mut tx)
            .await
            .unwrap();
        drop(tx);

        let mut events = Vec::new();
        while let Some(ev) = rx.next().await {
            events.push(ev);
        }
        assert!(
            !events.is_empty(),
            "a no-Content-Length response must still emit at least one FetchProgress event"
        );
        match events.last().unwrap() {
            UpdateRunEvent::FetchProgress {
                bytes_done,
                bytes_total,
            } => {
                assert_eq!(*bytes_done, 1000);
                assert_eq!(
                    *bytes_total, 1000,
                    "unknown total reports bytes actually downloaded"
                );
            }
            other => panic!("expected FetchProgress, got {other:?}"),
        }
    }

    #[test]
    fn installer_args_always_carries_the_fixed_flags() {
        let args = installer_args("v0.2.3");
        assert_eq!(
            args,
            vec![
                "--non-interactive".to_string(),
                "--json-progress".to_string(),
                "--version=v0.2.3".to_string(),
            ]
        );
    }
}
