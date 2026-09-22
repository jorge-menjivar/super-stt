// SPDX-License-Identifier: GPL-3.0-only
//! macOS sandbox lifecycle for subprocess backends: spawning the backend
//! under a `sandbox-exec` profile, sweeping processes left by a prior daemon
//! run, and the profile itself (shared with the enforcement test so the two
//! stay in lock-step).
//!
//! This is the macOS counterpart to [`super::systemd`], and the two are not
//! shaped the same, because what supervises the backend is not the same.
//! systemd owns the transient unit: the daemon asks for it by name, and can
//! stop it, read its logs, and sweep leftovers by name without ever holding a
//! handle. Here the daemon *is* the supervisor — the backend is its own child
//! process — so each of those three jobs needs its own mechanism:
//!
//! | job | Linux | macOS |
//! |---|---|---|
//! | confinement | unit properties | an SBPL profile ([`profile`]) |
//! | teardown | `systemctl --user stop` | signal the child's process group |
//! | logs | `journalctl --user -u` | a log file the child's stdio is pointed at |
//! | orphan sweep | `systemctl` by unit glob | pid files ([`sweep_orphans`]) |

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::{info, warn};

use crate::stt_models::backends::manifest::Device;

/// How long a backend gets to exit on `SIGTERM` before it is `SIGKILL`ed.
///
/// Teardown happens on the model-switch path, which a user is waiting on, so
/// this is short. A backend that has not unmapped its weights by now is not
/// about to.
const TERM_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// How long [`Sandboxed::stop_blocking`] waits before escalating.
///
/// Shorter than [`TERM_GRACE`] because it is spent blocking a runtime worker
/// thread rather than awaiting, and it is only reached when a backend was
/// dropped without the awaited `shutdown()` — a path that should not happen
/// in normal operation.
const DROP_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// How often [`Sandboxed::stop_blocking`] checks during [`DROP_GRACE`].
const DROP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// Lines of backend log kept for a diagnostic. Matches the `journalctl -n 30`
/// the Linux side asks for.
const LOG_TAIL_LINES: usize = 30;

/// A running backend process and the things needed to supervise it.
pub(super) struct Sandboxed {
    /// Instance name, used in log lines and to derive the sibling file paths.
    label: String,
    /// The `sandbox-exec` child. `None` once it has been reaped, so a
    /// [`Self::stop`] followed by `Drop` does not signal a dead pid.
    child: Option<tokio::process::Child>,
    /// Process *group* id — the child is made a group leader at spawn so
    /// teardown reaches anything it forked, not just `sandbox-exec` itself.
    pgid: i32,
    /// Where the child's stdout and stderr are pointed.
    log_path: PathBuf,
    /// The pid file that lets a *future* daemon find this process if this one
    /// dies without running teardown.
    pid_path: PathBuf,
    /// The backend's private scratch directory (its `$TMPDIR`).
    tmp_dir: PathBuf,
}

/// Spawn the backend binary under a `sandbox-exec` profile.
///
/// `devices` is accepted for signature parity with the Linux spawner and
/// deliberately unused. There, it decides whether the unit is granted the GPU
/// *device nodes* — `/dev/nvidia*`, `/dev/kfd`, the DRM render nodes — because
/// on Linux the GPU is reached by opening a character device, and handing a
/// CPU-only backend that device would be handing it privileged kernel attack
/// surface for nothing. Metal is not reached that way: there is no node to
/// withhold, the GPU is behind `IOKit` services that the profile below leaves
/// open, and a CPU-only backend simply never asks. So the distinction has
/// nothing to attach to here.
pub(super) async fn spawn_sandboxed(
    label: &str,
    binary: &Path,
    backend_dir: &Path,
    socket_dir: &Path,
    socket: &Path,
    _devices: &[Device],
) -> Result<Sandboxed> {
    // A writable scratch directory, standing in for systemd's `PrivateTmp`.
    // The profile below denies every write outside the socket directory, so
    // without this a backend that stages anything to `$TMPDIR` — which is
    // ordinary behavior while loading weights — would fail on a write it has
    // no reason to expect to fail.
    let tmp_dir = socket_dir.join(format!("{label}.tmp"));
    // Cleared rather than merely created: a previous run of this same
    // instance may have left files here, and a backend that finds a stale
    // partial from a crash is worse off than one that finds nothing.
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir)
        .with_context(|| format!("create backend scratch dir {}", tmp_dir.display()))?;

    let log_path = socket_dir.join(format!("{label}.log"));
    let log = std::fs::File::create(&log_path)
        .with_context(|| format!("create backend log {}", log_path.display()))?;
    let log_err = log
        .try_clone()
        .context("duplicate the backend log handle for stderr")?;

    let profile = profile(socket_dir);

    let mut cmd = tokio::process::Command::new(SANDBOX_EXEC);
    cmd.arg("-p")
        .arg(&profile)
        .arg(binary)
        .env("SUPER_STT_BACKEND_SOCKET", socket)
        .env("SUPER_STT_BACKEND_DIR", backend_dir)
        .env("TMPDIR", &tmp_dir)
        .env("RUST_LOG", "info")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(log_err))
        // Its own process group, so teardown can signal the whole tree. The
        // backend is `sandbox-exec`'s child, not ours, so signalling the pid
        // we hold would leave the process that matters running.
        .process_group(0)
        // Backstop for the paths that drop the handle without teardown. It
        // reaches only `sandbox-exec` itself, which is why `stop` signals the
        // group rather than relying on this.
        .kill_on_drop(true);

    let child = cmd
        .spawn()
        .with_context(|| format!("spawn {SANDBOX_EXEC} for backend {label}"))?;
    let child_pid = child
        .id()
        .context("sandbox-exec child had no pid immediately after spawn")?;
    // The child was made a group leader at spawn, so its group id is its pid.
    let group = i32::try_from(child_pid).context("backend pid does not fit in a pid_t")?;

    // Written only after a successful spawn, and *with* the binary path: the
    // sweep below refuses to signal a pid whose current executable is not
    // this one, which is what makes recovering from a killed daemon safe
    // rather than a pid-recycling hazard.
    let pid_path = socket_dir.join(format!("{label}.pid"));
    if let Err(e) = std::fs::write(&pid_path, format!("{child_pid}\n{}\n", binary.display())) {
        // Not fatal: this daemon can still stop the backend through the
        // handle it is holding. What is lost is the *next* daemon's ability
        // to clean up after an ungraceful exit of this one.
        warn!(
            "could not write {}: {e}; a crash of this daemon would orphan backend {label}",
            pid_path.display()
        );
    }

    info!("spawned sandboxed backend {label} (pid {child_pid})");
    Ok(Sandboxed {
        label: label.to_string(),
        child: Some(child),
        pgid: group,
        log_path,
        pid_path,
        tmp_dir,
    })
}

impl Sandboxed {
    /// The instance name, for log lines and error messages.
    pub(super) fn label(&self) -> &str {
        &self.label
    }

    /// Recent backend output, for a diagnostic.
    pub(super) fn logs(&self) -> String {
        let Ok(contents) = std::fs::read_to_string(&self.log_path) else {
            return String::new();
        };
        let lines: Vec<&str> = contents.lines().collect();
        let tail = lines.len().saturating_sub(LOG_TAIL_LINES);
        lines[tail..].join("\n")
    }

    /// Stop the backend, awaiting its exit.
    ///
    /// `SIGTERM` to the process group, then `SIGKILL` to the group if it has
    /// not exited within [`TERM_GRACE`]. The group, not the pid, because the
    /// backend is `sandbox-exec`'s child.
    pub(super) async fn stop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        signal_group(self.pgid, libc::SIGTERM, &self.label);

        match tokio::time::timeout(TERM_GRACE, child.wait()).await {
            Ok(Ok(status)) => info!("backend {} exited with {status}", self.label),
            Ok(Err(e)) => warn!("waiting on backend {} failed: {e}", self.label),
            Err(_) => {
                warn!(
                    "backend {} did not exit within {TERM_GRACE:?}; killing",
                    self.label
                );
                signal_group(self.pgid, libc::SIGKILL, &self.label);
                // Reap, so the killed process does not linger as a zombie for
                // the life of the daemon.
                if let Err(e) = child.wait().await {
                    warn!("reaping backend {} failed: {e}", self.label);
                }
            }
        }
        self.cleanup_files();
    }

    /// Stop the backend from a synchronous context (`Drop`).
    ///
    /// Confirms the exit rather than assuming it, which costs a bounded
    /// block on the calling thread. The Linux `Drop` path does the same —
    /// `systemctl --user stop` does not return until the unit is down — and
    /// the reason is the same on both: this is the last code that will ever
    /// hold a handle to this process. Signalling and walking away leaves a
    /// backend with a multi-gigabyte model resident and nothing left to kill
    /// it, because [`cleanup_files`](Self::cleanup_files) is about to delete
    /// the pid file a future daemon's sweep would have found it by.
    ///
    /// [`DROP_GRACE`] rather than [`TERM_GRACE`]: this runs on a runtime
    /// worker thread, so the stall is worth less here than on the awaited
    /// path, and a backend dropped without a `shutdown()` is already an
    /// exceptional case.
    pub(super) fn stop_blocking(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        signal_group(self.pgid, libc::SIGTERM, &self.label);

        let deadline = std::time::Instant::now() + DROP_GRACE;
        loop {
            // Non-blocking: reaps the child if it has exited, tells us
            // nothing has changed otherwise.
            match child.try_wait() {
                Ok(Some(status)) => {
                    info!("backend {} exited with {status}", self.label);
                    break;
                }
                Err(e) => {
                    warn!("could not check on backend {}: {e}", self.label);
                    break;
                }
                Ok(None) if std::time::Instant::now() >= deadline => {
                    warn!(
                        "backend {} did not exit within {DROP_GRACE:?}; killing",
                        self.label
                    );
                    // The group, so the backend goes too and not just the
                    // `sandbox-exec` wrapper this handle points at.
                    signal_group(self.pgid, libc::SIGKILL, &self.label);
                    let _ = child.start_kill();
                    let _ = child.try_wait();
                    break;
                }
                Ok(None) => std::thread::sleep(DROP_POLL_INTERVAL),
            }
        }
        self.cleanup_files();
    }

    /// Remove the socket-directory files this instance owns. Best effort:
    /// they are all under a per-user directory, and a leftover is at worst
    /// swept by the next spawn of the same instance.
    fn cleanup_files(&self) {
        let _ = std::fs::remove_file(&self.pid_path);
        let _ = std::fs::remove_dir_all(&self.tmp_dir);
    }
}

impl Drop for Sandboxed {
    fn drop(&mut self) {
        self.stop_blocking();
    }
}

/// Send `signal` to the process group `pgid`.
///
/// Refuses a non-positive `pgid` outright, in release builds as well as
/// debug. This is not defensive padding: `killpg(0, …)` signals *this
/// daemon's own* process group, and the underlying `kill(-1, …)` signals
/// every process the user is permitted to signal. One of the two `pgid`
/// sources is a number parsed out of a file on disk
/// ([`sweep_orphans`]), so "it cannot be negative" is an argument about
/// a file's contents, and the wrong place to be making one.
fn signal_group(pgid: i32, signal: libc::c_int, label: &str) {
    if pgid <= 0 {
        warn!("refusing to signal non-positive process group {pgid} for backend {label}");
        return;
    }
    // SAFETY: `killpg` is async-signal-safe and takes no pointers. `pgid` is
    // positive per the guard above, so this addresses one real group.
    if unsafe { libc::killpg(pgid, signal) } != 0 {
        let err = std::io::Error::last_os_error();
        // ESRCH just means it already exited, which is the outcome we wanted.
        if err.raw_os_error() != Some(libc::ESRCH) {
            warn!("killpg({pgid}, {signal}) for backend {label} failed: {err}");
        }
    }
}

/// Absolute path to the system sandbox wrapper. Absolute, never resolved
/// through `PATH`: a writable directory ahead of `/usr/bin` in the daemon's
/// `PATH` would otherwise be enough to run backends unconfined.
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The SBPL profile applied to every spawned backend. Shared by the spawner
/// and the sandbox-enforcement tests so they stay in lock-step.
///
/// It confines the two things that actually matter for a component that has
/// already been handed its model files and only has to answer on a socket:
///
/// - **No network.** `(deny network*)` with Unix sockets carved back out. The
///   backend's whole interface is the pathname socket, and the daemon has
///   already downloaded everything it needs, so any outbound connection is
///   either exfiltration or a bug. This is the `PrivateNetwork=yes` of the
///   Linux unit.
/// - **A read-only filesystem, bar one directory.** `(deny file-write*)` with
///   the socket directory carved back out, covering `ProtectSystem=strict`,
///   `ProtectHome=read-only`, `PrivateTmp` and `ReadWritePaths` in one rule.
///   Reads stay open, which is what lets the backend load its own weights
///   from under the data directory.
///
/// **What it deliberately does not claim.** The profile opens with
/// `(allow default)` and denies from there, rather than `(deny default)` with
/// an allowlist. A deny-by-default SBPL profile has to enumerate every mach
/// service, `IOKit` class and dyld path an arbitrary third-party binary might
/// legitimately need; get that wrong and backends fail to start with errors
/// that point nowhere near the sandbox. Since backends are out-of-tree and
/// this daemon cannot know what any given one links against, the allowlist
/// would be guesswork that silently rots. The two denies above are the ones
/// that can be written correctly without knowing the program.
///
/// It is therefore genuinely weaker than the Linux unit in one respect:
/// there is no counterpart to `SystemCallFilter=@system-service`, and no
/// counterpart to `NoNewPrivileges=yes`. A backend that is *already* running
/// arbitrary code can still make arbitrary syscalls; what it cannot do is
/// reach the network or write anywhere but its own socket directory.
pub(super) fn profile(socket_dir: &Path) -> String {
    format!(
        r#"(version 1)
(allow default)
(deny network*)
(allow network* (local unix-socket) (remote unix-socket))
(deny file-write*)
(allow file-write* (subpath "{}"))
(allow file-write-data
    (literal "/dev/null")
    (literal "/dev/zero")
    (literal "/dev/random")
    (literal "/dev/urandom")
    (literal "/dev/dtracehelper"))
"#,
        sbpl_escape(socket_dir)
    )
}

/// Escape a path for an SBPL double-quoted string literal.
///
/// Only backslash and double quote are special there. The paths reaching this
/// are the daemon's own runtime directory rather than anything a caller
/// supplies, but a quote in a path would not merely break the profile — it
/// would end the `subpath` literal early and change which directory is
/// writable, so it is escaped rather than assumed absent.
fn sbpl_escape(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .flat_map(|c| match c {
            '\\' | '"' => vec!['\\', c],
            other => vec![other],
        })
        .collect()
}

/// Kill backend processes left behind by a previous daemon run.
///
/// The macOS counterpart to [`super::systemd::cleanup_orphan_units`], and it
/// has to work differently. There, each backend is a systemd unit whose name
/// carries the spawning daemon's pid, so leftovers can be found by globbing
/// unit names and stopped by asking systemd. Here the backend is a plain
/// process that this daemon parented, and a daemon killed with `SIGKILL`
/// leaves it reparented to `launchd` with nothing recording that it was ours.
/// The pid file written at spawn is that record.
///
/// **A pid file alone would not be safe to act on.** Pids are recycled, and a
/// stale file naming a pid that now belongs to something else would have this
/// function kill an unrelated process of the user's. So the pid is only a
/// candidate: the file also stores the backend binary that was spawned, and
/// nothing is signalled unless the pid's *current* executable still matches
/// it. A recycled pid fails that check and the stale file is simply removed.
pub(super) async fn sweep_orphans(socket_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(socket_dir) else {
        // No socket directory yet — nothing has ever been spawned.
        return;
    };

    let mut swept = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pid") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut lines = contents.lines();
        // `> 0` and not merely "parses as i32": see [`signal_group`] for what
        // a zero or negative value would go on to signal.
        let (Some(pid), Some(expected_exe)) = (
            lines
                .next()
                .and_then(|l| l.trim().parse::<i32>().ok())
                .filter(|pid| *pid > 0),
            lines.next().map(str::trim),
        ) else {
            warn!("ignoring malformed backend pid file {}", path.display());
            let _ = std::fs::remove_file(&path);
            continue;
        };

        match exe_of(pid) {
            Some(actual) if actual == Path::new(expected_exe) => {
                info!("sweeping orphaned backend pid {pid} ({expected_exe})");
                terminate_orphan(pid).await;
                // The socket, log and scratch dir the orphan owned. Its
                // socket in particular has to go: the next spawn of this
                // instance binds the same path, and a leftover file there
                // makes the bind fail with `EADDRINUSE`.
                let stem = path.with_extension("");
                let _ = std::fs::remove_file(stem.with_extension("sock"));
                let _ = std::fs::remove_file(stem.with_extension("log"));
                let _ = std::fs::remove_dir_all(stem.with_extension("tmp"));
                swept += 1;
            }
            Some(actual) => {
                warn!(
                    "backend pid file {} names {expected_exe} but pid {pid} is now {}; \
                     leaving it alone",
                    path.display(),
                    actual.display()
                );
            }
            None => {
                // The process is gone; only the file is left.
            }
        }
        let _ = std::fs::remove_file(&path);
    }

    if swept > 0 {
        info!("swept {swept} orphaned backend process(es) from a previous run");
    }
}

/// `SIGTERM` an orphaned backend's process group, then `SIGKILL` it if it is
/// still there.
///
/// Polled rather than waited on: an orphan was reparented to `launchd` when
/// the previous daemon died, so it is not this process's child and
/// `waitpid` will not report it. `kill(pid, 0)` is the available signal that
/// it is gone.
///
/// Escalating matters more here than in [`Sandboxed::stop`]. This runs at
/// daemon startup, and anything it fails to kill keeps whatever GPU memory
/// and model weights it had resident for as long as the machine is up, while
/// a fresh copy of the same backend is about to be spawned beside it.
async fn terminate_orphan(pid: i32) {
    signal_group(pid, libc::SIGTERM, "orphan");
    for _ in 0..ORPHAN_TERM_POLLS {
        tokio::time::sleep(ORPHAN_TERM_POLL_INTERVAL).await;
        // SAFETY: signal 0 performs the permission and existence checks
        // without delivering anything.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return;
        }
    }
    warn!("orphaned backend pid {pid} ignored SIGTERM; killing");
    signal_group(pid, libc::SIGKILL, "orphan");
}

/// Polls, and the gap between them, that [`terminate_orphan`] gives a
/// `SIGTERM` before escalating — one second in total, spent once at startup.
const ORPHAN_TERM_POLLS: u32 = 10;
/// See [`ORPHAN_TERM_POLLS`].
const ORPHAN_TERM_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// The executable currently running as `pid`, or `None` if there is no such
/// process (or it is not ours to ask about).
///
/// Same `proc_pidpath` the peer-identity check uses, and here for the same
/// reason: it is the kernel's answer, not the process's own claim, which is
/// what makes it safe to gate a `kill` on.
fn exe_of(pid: i32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt as _;

    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buf` is a live allocation of exactly the length passed.
    let written = unsafe {
        libc::proc_pidpath(
            pid,
            buf.as_mut_ptr().cast::<libc::c_void>(),
            u32::try_from(buf.len()).unwrap_or(u32::MAX),
        )
    };
    if written <= 0 {
        return None;
    }
    // `written` is positive here — the `<= 0` branch above returned.
    let Ok(len) = usize::try_from(written) else {
        return None;
    };
    buf.truncate(len);
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf)))
}

#[cfg(test)]
mod tests {
    use super::{exe_of, profile, sbpl_escape, sweep_orphans};
    use std::path::{Path, PathBuf};

    /// The two rules the profile exists for. A regression that drops either
    /// leaves backends running with network access or with the user's home
    /// writable, and neither failure is visible from the outside.
    #[test]
    fn profile_denies_network_and_writes_outside_the_socket_dir() {
        let p = profile(Path::new("/run/sock"));
        assert!(p.contains("(deny network*)"), "{p}");
        assert!(p.contains("(deny file-write*)"), "{p}");
        assert!(
            p.contains(r#"(allow file-write* (subpath "/run/sock"))"#),
            "the socket dir must be the one writable place: {p}"
        );
        // The socket is the backend's whole interface; denying it would make
        // the sandbox a no-op in the other direction.
        assert!(p.contains("(allow network* (local unix-socket)"), "{p}");
    }

    /// A quote in the socket path would close the `subpath` literal early and
    /// silently change which directory is writable.
    #[test]
    fn sbpl_literals_are_escaped() {
        assert_eq!(sbpl_escape(Path::new(r#"/a"b"#)), r#"/a\"b"#);
        assert_eq!(sbpl_escape(Path::new(r"/a\b")), r"/a\\b");
        let p = profile(Path::new(r#"/tmp/od"d"#));
        assert!(p.contains(r#"(subpath "/tmp/od\"d")"#), "{p}");
    }

    /// The sandbox has to actually hold, not merely be described correctly.
    /// Runs the real `sandbox-exec` with the real profile and checks that a
    /// write outside the socket directory is refused and one inside is not.
    #[test]
    fn sandbox_is_enforced() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Canonicalized because `sandbox-exec` matches the profile's
        // `subpath` against the resolved path, and macOS `TMPDIR` lives
        // under the `/var` -> `/private/var` symlink.
        let sock_dir = dir.path().canonicalize().expect("canonicalize tempdir");
        let outside = sock_dir.join("outside");
        std::fs::create_dir_all(&outside).expect("create the outside dir");
        let inside = sock_dir.join("inside");
        std::fs::create_dir_all(&inside).expect("create the inside dir");

        // Only `inside` is writable, so a write to its sibling must fail even
        // though both are under the same tempdir.
        let p = profile(&inside);

        let run = |script: &str| {
            std::process::Command::new(super::SANDBOX_EXEC)
                .arg("-p")
                .arg(&p)
                .arg("/bin/sh")
                .arg("-c")
                .arg(script)
                .status()
                .expect("run sandbox-exec")
                .success()
        };

        assert!(
            run(&format!("echo ok > {}/f", inside.display())),
            "a write to the socket dir must be allowed"
        );
        assert!(
            !run(&format!("echo no > {}/f", outside.display())),
            "a write outside the socket dir must be denied"
        );
    }

    /// `exe_of` is what stands between the orphan sweep and killing an
    /// unrelated process, so it has to report this process's real binary.
    #[test]
    fn exe_of_reports_the_running_binary() {
        let me = i32::try_from(std::process::id()).expect("pid fits");
        let reported = exe_of(me).expect("this process has an executable");
        let expected = std::env::current_exe().expect("current exe");
        assert_eq!(
            reported.canonicalize().ok(),
            expected.canonicalize().ok(),
            "exe_of disagreed with current_exe"
        );
        // A pid that cannot exist has no executable, rather than some
        // fallback that the sweep would then act on.
        assert_eq!(exe_of(-1), None);
    }

    /// A pid file whose pid has been recycled onto another program must not
    /// get that program killed. Written with *this* test process's pid and a
    /// binary path that is deliberately not ours: the sweep must notice the
    /// mismatch, leave the process alone, and still clear the stale file.
    #[tokio::test]
    async fn sweep_refuses_a_recycled_pid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("some-instance.pid");
        std::fs::write(
            &pid_file,
            format!("{}\n/nonexistent/other-program\n", std::process::id()),
        )
        .expect("write pid file");

        sweep_orphans(dir.path()).await;

        // Still alive — the test is running.
        assert!(!pid_file.exists(), "the stale pid file should be removed");
    }

    /// A malformed pid file is discarded rather than parsed loosely. A
    /// partial parse here would mean signalling a pid read out of garbage.
    ///
    /// `-1` and `0` are in the list because they are the two values that
    /// would not merely signal the wrong process: `killpg(0, …)` hits this
    /// daemon's own process group, and `-1` is the kernel's broadcast.
    #[tokio::test]
    async fn sweep_discards_malformed_pid_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cases = [
            ("bad.pid", "not-a-pid\n"),
            ("short.pid", "12345\n"),
            ("broadcast.pid", "-1\n/some/backend\n"),
            ("selfgroup.pid", "0\n/some/backend\n"),
        ];
        for (name, body) in cases {
            std::fs::write(dir.path().join(name), body).expect("write");
        }

        sweep_orphans(dir.path()).await;

        for (name, _) in cases {
            assert!(
                !dir.path().join(name).exists(),
                "{name} should have been discarded"
            );
        }
    }

    /// The guard itself, exercised directly: a non-positive group is refused
    /// before it reaches `killpg`. If this ever regresses, the test process
    /// signals its own group and the suite dies — which is the point.
    #[test]
    fn signal_group_refuses_a_non_positive_group() {
        super::signal_group(0, libc::SIGTERM, "guard test");
        super::signal_group(-1, libc::SIGTERM, "guard test");
        // Reaching here at all means neither call was delivered.
    }

    /// Files that are not pid files are left untouched — the sockets and logs
    /// of *live* backends share this directory.
    #[tokio::test]
    async fn sweep_ignores_non_pid_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sock: PathBuf = dir.path().join("live.sock");
        std::fs::write(&sock, b"").expect("write");
        let log = dir.path().join("live.log");
        std::fs::write(&log, b"").expect("write");

        sweep_orphans(dir.path()).await;

        assert!(
            sock.exists(),
            "a live backend's socket must survive a sweep"
        );
        assert!(log.exists());
    }
}
