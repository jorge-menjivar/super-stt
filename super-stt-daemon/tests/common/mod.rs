// SPDX-License-Identifier: GPL-3.0-only
//! Shared harness for the tests that spawn a real `super-stt-daemon`.
//!
//! Every one of them holds the child in a guard whose `Drop` stops it. How it
//! is stopped is not a detail — see [`shutdown`].

use std::process::Child;
use std::time::{Duration, Instant};

/// How long a daemon gets to finish its graceful shutdown before we stop
/// waiting and `SIGKILL` it, so a wedged child can't hang the suite.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// How often to re-check whether the child has exited while waiting.
const SHUTDOWN_POLL: Duration = Duration::from_millis(25);

/// Stop a spawned daemon the way anything else stops one: `SIGINT`, then wait
/// for it to exit on its own.
///
/// The obvious spelling — [`Child::kill`] — sends `SIGKILL`, which no process
/// can handle. That costs more than an unclean exit. Under `cargo llvm-cov` the
/// daemon is an *instrumented* binary that writes its `.profraw` from an
/// `atexit` hook, and a `SIGKILL`'d process never runs one. The profile is
/// simply never written, so every endpoint the test just exercised is reported
/// as uncovered: the whole `/v1` surface read as 0% while these tests passed
/// against it.
///
/// `SIGINT` instead runs the daemon's Ctrl+C path in `daemon_main::run`, which
/// unloads the model, drains queued session-store writes, and leaves through
/// `std::process::exit` — flushing the profile on the way out. It is also the
/// shutdown the daemon is written for, so these tests now exercise it rather
/// than a path production never takes: an ungracefully killed daemon orphans
/// its `systemd --user` backend units, which is why `daemon_main` has to sweep
/// for them at startup.
pub fn shutdown(child: &mut Child) {
    // Has it already gone? A test that kills its own daemon mid-run (see
    // `widget_smoke`, which drops one on purpose to watch a client notice)
    // reaches this having already reaped it, and the pid may since have been
    // recycled onto an unrelated process. Never signal that.
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }

    let Ok(pid) = i32::try_from(child.id()) else {
        // Not a pid `kill(2)` can name. Nothing to do but the blunt thing.
        let _ = child.kill();
        let _ = child.wait();
        return;
    };

    // The result is dropped on purpose: the only failure worth acting on is
    // "the child is already gone", which the wait loop below handles anyway.
    //
    // SAFETY: `pid` is our own child, and nothing has reaped it yet — the only
    // `wait` calls are below — so the pid cannot have been recycled onto an
    // unrelated process.
    let _ = unsafe { libc::kill(pid, libc::SIGINT) };

    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(SHUTDOWN_POLL),
            // Already reaped, or a pid we can no longer ask about; either way
            // there is nothing left to wait for.
            Err(_) => return,
        }
    }

    let _ = child.kill();
    let _ = child.wait();
}
