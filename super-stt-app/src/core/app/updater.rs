// SPDX-License-Identifier: GPL-3.0-only
//! Apply flow: download the installer asset, spawn it with
//! `--non-interactive --json-progress`, and stream its JSON progress lines
//! back as [`UpdateRunEvent`]s.

use std::collections::VecDeque;
use std::path::Path;

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
    let dir = super_stt_shared::paths::cache_dir().join("self-update");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("create {}: {e}", dir.display()))?;
    let bin = dir.join(&asset.name);

    download_installer(&asset.url, &bin, tx).await?;

    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .await
            .map_err(|e| format!("chmod {}: {e}", bin.display()))?;
    }

    let mut cmd = tokio::process::Command::new(&bin);
    cmd.arg("--non-interactive")
        .arg("--json-progress")
        .arg(format!("--version={target_tag}"));
    if target_tag.contains("-beta") || target_tag.contains('-') {
        cmd.arg("--beta"); // prerelease target: resolution must see prereleases
    }
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
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut tail: VecDeque<String> = VecDeque::with_capacity(STDERR_TAIL_LINES);
        while let Ok(Some(line)) = lines.next_line().await {
            if tail.len() == STDERR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        Vec::from(tail)
    });

    let mut lines = BufReader::new(stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => match serde_json::from_str::<InstallerEvent>(&line) {
                Ok(ev) => {
                    let _ = tx.send(UpdateRunEvent::Installer(ev)).await;
                }
                // Forward-compatible: an installer newer than this app may
                // emit a shape this parser doesn't know yet. Skip, don't fail
                // the run over it.
                Err(e) => log::warn!("installer emitted an unparsable progress line: {e}: {line}"),
            },
            Ok(None) => break,
            Err(e) => {
                log::warn!("installer stdout read error: {e}");
                break;
            }
        }
    }

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
    Ok(())
}

/// Stream `url` to `dest`, reporting throttled `FetchProgress` events.
/// Mirrors `super-stt-install`'s `download_to_file` chunk loop.
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
            let _ = tx
                .send(UpdateRunEvent::FetchProgress {
                    bytes_done: done,
                    bytes_total: total,
                })
                .await;
        }
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
    use super::InstallerEvent;

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
}
