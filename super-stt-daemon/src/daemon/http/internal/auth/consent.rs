// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::http::state::PeerInfo;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Global cap of one on-screen consent popup at a time. See
/// [`ask_user_for_consent`] — without it a same-uid client could drive hundreds
/// of concurrent exclusive-keyboard dialogs (255 distinct consent keys) and lock
/// the desktop (audit 2 Tier 3 #10).
static CONSENT_POPUP: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Who the daemon believes is calling.
///
/// The two variants are the two transports, and they are not the same kind of
/// claim. [`Self::Native`] is what the kernel says about a peer on the Unix
/// socket; [`Self::Web`] is what a browser says about the page it is running.
/// Keeping them as separate variants rather than one struct with optional
/// fields is what stops a check written for one from silently passing for the
/// other — `is_official_client` reading an absent exe path as "not official"
/// would be correct by accident, and one refactor away from not being.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PeerIdentity {
    /// A process on the Unix socket, identified by `SO_PEERCRED`.
    ///
    /// For an ordinary host process this is just the path the kernel says it
    /// is running — `/proc/<pid>/exe` on Linux, `proc_pidpath` on macOS. A peer
    /// inside a flatpak has its own mount namespace, so that path is resolved
    /// in *its* root and means nothing here: every such peer reads as something
    /// like `/app/bin/<name>`, a string any other sandbox can present just by
    /// naming its binary the same. Identifying a sandboxed caller by its exe
    /// path alone therefore hands one sandbox's grant to every other. The
    /// sandbox's own id is what distinguishes them, so it is carried alongside
    /// and is part of equality.
    ///
    /// The id is only as trustworthy as the sandbox that wrote it, and this is
    /// not a defence against a hostile process running as the user — one of
    /// those can read the session tokens out of the keyring regardless. It is
    /// what lets the daemon name the caller correctly in the consent dialog,
    /// and keep one sandboxed app's grant from silently covering another's.
    Native {
        /// The peer's executable path, as resolved in its own mount namespace
        /// on Linux (where a namespace is possible) — see [`peer_exe_path`].
        exe_path: PathBuf,
        /// `Some(app-id)` when the peer runs inside a flatpak sandbox.
        #[serde(default)]
        flatpak_app_id: Option<String>,
    },
    /// A page on the TCP listener, identified by its `Origin`.
    ///
    /// **This is a weaker claim than [`Self::Native`], and deliberately so.**
    /// The kernel vouches for an exe path; nothing vouches for an origin but
    /// the browser that sent it. A non-browser process can put any string here.
    /// What keeps that from mattering is that the daemon only accepts origins
    /// the user wrote into [`TcpConfig::allowed_origins`](crate::config::TcpConfig::allowed_origins)
    /// — so forging one gets you no further than forging an origin the user
    /// already trusted, on a listener they already turned on.
    ///
    /// The consent dialog says which kind it is asking about, because "allow
    /// this website" and "allow this program" deserve different answers.
    Web {
        /// The full origin as the browser sent it: scheme, host and port.
        origin: String,
    },
}

impl PeerIdentity {
    /// A host process with no sandbox of its own.
    #[cfg(test)]
    pub(crate) fn native(exe_path: impl Into<PathBuf>) -> Self {
        Self::Native {
            exe_path: exe_path.into(),
            flatpak_app_id: None,
        }
    }

    /// A browser page served from `origin`.
    #[cfg(test)]
    pub(crate) fn web(origin: impl Into<String>) -> Self {
        Self::Web {
            origin: origin.into(),
        }
    }

    /// One line naming the caller, for logs and the consent dialog. A
    /// sandboxed peer leads with its app id, since its path is not a path
    /// anyone can go and look at; a web peer is named as a web peer, so a log
    /// line can never be read as naming a binary.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Native {
                exe_path,
                flatpak_app_id: Some(id),
            } => format!("flatpak {id} ({})", exe_path.display()),
            Self::Native { exe_path, .. } => exe_path.display().to_string(),
            Self::Web { origin } => format!("web origin {origin}"),
        }
    }
}

/// Identifies the consent flow uniquely: (`identity`, normalized `scopes`).
/// The user verifies a *binary* (or a sandboxed app), not a self-reported
/// display name, so the deny / dedup key is keyed on the kernel-resolved
/// [`PeerIdentity`] plus the requested scope set (sorted + deduped via
/// [`normalize_scopes`] so request order doesn't matter). `app_name` is
/// shown in the popup but isn't part of the identity.
pub(crate) type ConsentKey = (PeerIdentity, Vec<String>);
pub(crate) type ConsentLock = Arc<tokio::sync::Mutex<()>>;

/// Sort + dedup a requested scope list so the consent key and the
/// granted set are independent of the order the client listed them.
pub(crate) fn normalize_scopes(scopes: &[String]) -> Vec<String> {
    let mut v = scopes.to_vec();
    v.sort();
    v.dedup();
    v
}

/// Per-`(exe_path, scope)` async mutex registry used by the
/// `/auth/request` handler to dedup concurrent first-time consent
/// requests. Without this, two clients that ping the daemon at the same
/// time on a fresh install would each spawn their own consent popup;
/// with it, the second blocks until the first finishes and then
/// short-circuits via the reuse-scan against the now-minted token.
///
/// The map is pruned via [`Self::release`] after the auth flow
/// completes so a malicious client can't drive unbounded memory
/// growth by spamming /auth/request with rotating keys.
#[derive(Clone, Default)]
pub(crate) struct ConsentLocks {
    inner: Arc<Mutex<HashMap<ConsentKey, ConsentLock>>>,
}

impl ConsentLocks {
    pub(crate) fn lock_for(&self, key: ConsentKey) -> ConsentLock {
        let mut map = self.inner.lock().unwrap();
        map.entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Drop the registry entry for `key` if no other task is still
    /// holding the `ConsentLock`. Called from the `auth_request`
    /// handler after the consent flow finishes — success or denial.
    /// `strong_count == 2` means exactly the map and our local clone
    /// hold references; anything higher means another in-flight
    /// `auth_request` for the same key is still waiting on the same
    /// mutex and we leave the entry in place for it.
    pub(crate) fn release(&self, key: &ConsentKey, lock: &ConsentLock) {
        let mut map = self.inner.lock().unwrap();
        // Our `lock` reference plus the one inside the map. If
        // anything else is still holding, leave it.
        if Arc::strong_count(lock) <= 2 {
            map.remove(key);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ConsentDecision {
    Allow,
    Deny,
    Dismissed,
    PopupFailed,
}

/// Spawn the `super-stt-consent` helper binary, wait up to 60s for the
/// user's decision. The helper writes one of `allow` / `deny` / `dismissed`
/// to stdout and exits.
/// Read the consent helper's single-line verdict from its stdout.
async fn read_consent_decision(stdout: tokio::process::ChildStdout) -> ConsentDecision {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut reader = BufReader::new(stdout).lines();
    match reader.next_line().await {
        Ok(Some(line)) => match line.trim() {
            "allow" => ConsentDecision::Allow,
            "deny" => ConsentDecision::Deny,
            _ => ConsentDecision::Dismissed,
        },
        _ => ConsentDecision::Dismissed,
    }
}

/// Put the consent question on screen and hand back the running dialog.
///
/// The caller owns the policy around it — the one-popup-at-a-time permit, the
/// 60-second deadline, and the reap — so all this does is start a process that
/// will print one of `allow` / `deny` / `dismissed` to stdout. `None` means no
/// dialog could be shown at all, which the caller reports as
/// [`ConsentDecision::PopupFailed`]: distinct from a denial, because the user
/// was never asked.
///
/// Linux spawns the libcosmic `super-stt-consent` helper installed beside the
/// daemon; see `locate_consent_helper` for why it is only ever looked for
/// there.
#[cfg(target_os = "linux")]
#[expect(
    clippy::unused_async,
    reason = "shares a signature with the macOS arm, which awaits writing its script to osascript"
)]
async fn spawn_consent_dialog(
    app_name: &str,
    scopes: &[String],
    identity: &PeerIdentity,
) -> Option<tokio::process::Child> {
    // `locate_consent_helper` already logs a specific reason on every
    // failure path (missing / un-canonicalizable / failed metadata check),
    // so we don't emit a second, redundant warning here.
    let helper = locate_consent_helper()?;

    let mut cmd = tokio::process::Command::new(&helper);
    cmd.env("STT_AUTH_APP_NAME", app_name)
        .env("STT_AUTH_SCOPES", scopes.join(" "));
    match identity {
        PeerIdentity::Native {
            exe_path,
            flatpak_app_id,
        } => {
            cmd.env("STT_AUTH_EXE_PATH", exe_path.to_string_lossy().as_ref())
                // Set only for a sandboxed peer, so the dialog can name the app
                // the user actually installed instead of a path inside its
                // sandbox.
                .envs(
                    flatpak_app_id
                        .as_ref()
                        .map(|id| ("STT_AUTH_FLATPAK_APP_ID", id.clone())),
                );
        }
        // A web peer sets the origin variable *instead of* the exe path, never
        // alongside it. The helper decides which dialog to show by which one it
        // was given, so sending both would leave the user reading a sentence
        // about a binary when a website is what is asking.
        PeerIdentity::Web { origin } => {
            cmd.env("STT_AUTH_WEB_ORIGIN", origin);
        }
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        // The timeout in the caller only reaps the helper while *that* request
        // is still running. A client that exits mid-consent cancels it, which
        // drops the future and this `Child` with it — and a dropped
        // `tokio::process::Child` leaves the process alone unless asked not
        // to. Without this the dialog is orphaned for the life of the
        // session, holding an exclusive-keyboard layer surface and ~40 MB
        // that nothing is left to kill.
        .kill_on_drop(true);

    match cmd.spawn() {
        Ok(c) => Some(c),
        Err(e) => {
            log::warn!("failed to spawn super-stt-consent: {e}");
            None
        }
    }
}

/// The `AppleScript` behind the macOS consent dialog.
///
/// It builds no strings: both the message and the title arrive as `argv`
/// items, so nothing a caller can put in an app name is ever parsed as
/// `AppleScript`. The two-step — compose in Rust, display in `AppleScript` — is
/// the whole reason this is `osascript -` with arguments rather than
/// `osascript -e` with the text interpolated in.
///
/// `giving up after 55` sits just inside the caller's 60-second deadline so
/// an abandoned dialog reports itself as dismissed and exits, instead of
/// being killed with the question still on screen.
///
/// There is deliberately no `cancel button`: naming one would make Escape and
/// a Deny click raise the same `-128`, and the daemon would lose the
/// difference between "the user refused" (sticky) and "the user walked away"
/// (not sticky).
#[cfg(target_os = "macos")]
const CONSENT_APPLESCRIPT: &str = r#"on run argv
	set dialogText to item 1 of argv
	set dialogTitle to item 2 of argv
	try
		set answer to display dialog dialogText with title dialogTitle buttons {"Deny", "Allow"} default button "Deny" with icon caution giving up after 55
	on error number -128
		return "dismissed"
	end try
	if gave up of answer then return "dismissed"
	if button returned of answer is "Allow" then return "allow"
	return "deny"
end run
"#;

/// Absolute path to the system `AppleScript` interpreter.
///
/// Absolute, never `osascript` off `PATH`, for the reason
/// the Linux helper lookup spells out: anyone who can prepend a writable
/// directory to the daemon's `PATH` could otherwise answer the consent
/// question on the user's behalf. `/usr/bin` is on the signed system volume,
/// which is read-only and cryptographically sealed.
#[cfg(target_os = "macos")]
const OSASCRIPT: &str = "/usr/bin/osascript";

/// See the Linux [`spawn_consent_dialog`].
///
/// macOS has no `super-stt-consent` binary to spawn — the helper is a
/// libcosmic application and libcosmic does not build here — so the question
/// goes up through `osascript` instead. The user-visible sentences come from
/// [`super_stt_shared::consent`], which is also what the Linux helper renders,
/// so the two platforms describe a grant identically.
#[cfg(target_os = "macos")]
async fn spawn_consent_dialog(
    app_name: &str,
    scopes: &[String],
    identity: &PeerIdentity,
) -> Option<tokio::process::Child> {
    use tokio::io::AsyncWriteExt as _;

    let message = consent_dialog_text(app_name, scopes, identity);

    let mut child = match tokio::process::Command::new(OSASCRIPT)
        // `-` reads the script from stdin; everything after it is `argv`.
        .arg("-")
        .arg(&message)
        .arg("Allow access to Super STT?")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        // See the Linux arm: a cancelled request drops the `Child`, and a
        // dropped child is not killed unless asked.
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log::warn!("failed to spawn {OSASCRIPT} for the consent dialog: {e}");
            return None;
        }
    };

    // Hand over the script and close the pipe — `osascript -` reads stdin to
    // EOF before it will run anything, so the dialog does not appear until
    // this drop happens.
    let Some(mut stdin) = child.stdin.take() else {
        log::warn!("osascript child had no stdin pipe; cannot deliver the consent script");
        return None;
    };
    if let Err(e) = stdin.write_all(CONSENT_APPLESCRIPT.as_bytes()).await {
        log::warn!("failed to write the consent script to osascript: {e}");
        return None;
    }
    drop(stdin);

    Some(child)
}

/// Compose what the macOS dialog says.
///
/// Mirrors the structure of the libcosmic helper's dialog: who is asking, how
/// they were identified, and the union of what the requested scopes grant.
///
/// `app_name` is the one part of this the *caller* chose, and the dialog is a
/// single text field rather than a set of labelled widgets — so an app name
/// carrying newlines could otherwise forge the `Executable:` line beneath it
/// and take credit for a binary the user trusts. [`sanitize_display_name`]
/// is what stops that, and is the reason this is assembled here rather than
/// inline at the call site.
#[cfg(target_os = "macos")]
fn consent_dialog_text(app_name: &str, scopes: &[String], identity: &PeerIdentity) -> String {
    use std::fmt::Write as _;

    let mut text = String::new();
    match identity {
        PeerIdentity::Native {
            exe_path,
            flatpak_app_id: _,
        } => {
            let name = sanitize_display_name(app_name);
            let name = if name.is_empty() {
                "An application".to_string()
            } else {
                name
            };
            let _ = writeln!(text, "{name} wants access to Super STT.");
            let _ = writeln!(text);
            // Not sanitized, and does not need to be: this is the path the
            // kernel reported for the calling process, not anything the
            // caller wrote.
            let _ = writeln!(text, "Executable:  {}", exe_path.display());
        }
        PeerIdentity::Web { origin } => {
            // The origin has already been matched against the user's
            // allowlist by the origin gate, so it is one of a small set of
            // strings the user typed themselves.
            let _ = writeln!(text, "{origin} wants access to Super STT.");
            let _ = writeln!(text);
            let _ = writeln!(
                text,
                "Your browser reports which website this is. That is a weaker check than \
                 Super STT does for installed programs."
            );
        }
    }

    let _ = writeln!(text);
    let _ = writeln!(text, "This will allow it to:");
    for line in super_stt_shared::consent::permissions_for_scopes(scopes) {
        let _ = writeln!(text, "  •  {line}");
    }
    text
}

/// Longest app name the dialog will show, in characters.
///
/// Long enough for any real product name; short enough that a name cannot
/// push the `Executable:` line and the permission list off the bottom of the
/// dialog, which would leave the user approving a question they cannot see
/// the whole of.
#[cfg(target_os = "macos")]
const MAX_DISPLAY_NAME: usize = 64;

/// Flatten a caller-supplied app name to one line of printable text.
///
/// Control characters — newlines above all — become spaces rather than being
/// dropped, so `"Foo\nExecutable:  /usr/bin/trusted"` reads as one visibly odd
/// name instead of silently becoming two convincing lines. Runs of whitespace
/// collapse for the same reason: spaces are as good as newlines for pushing
/// text around once the font is proportional.
#[cfg(target_os = "macos")]
fn sanitize_display_name(name: &str) -> String {
    let flattened: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut out = String::new();
    for word in flattened.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.chars().count() >= MAX_DISPLAY_NAME {
            break;
        }
    }
    if out.chars().count() > MAX_DISPLAY_NAME {
        out = out.chars().take(MAX_DISPLAY_NAME - 1).collect();
        out.push('…');
    }
    out
}

pub(crate) async fn ask_user_for_consent(
    app_name: &str,
    scopes: &[String],
    identity: &PeerIdentity,
) -> ConsentDecision {
    // Serialize popups globally: at most one consent dialog on screen at a time
    // (audit 2 Tier 3 #10). `/auth/request` is unauthenticated and outside the
    // rate limiter, and the 8 scopes yield 255 distinct `(exe, scopes)` consent
    // keys — each bypassing the per-key dedup — so without this cap a same-uid
    // process could stack hundreds of concurrent exclusive-keyboard overlays and
    // lock the desktop. Excess requests wait for the permit rather than opening
    // in parallel. Acquired before the spawn and held while the dialog is on
    // screen; released before the untimed reap below so a wedged helper can't
    // wedge all consent.
    let Ok(popup_permit) = CONSENT_POPUP.acquire().await else {
        return ConsentDecision::PopupFailed; // semaphore closed (never in practice)
    };

    let Some(mut child) = spawn_consent_dialog(app_name, scopes, identity).await else {
        drop(popup_permit);
        return ConsentDecision::PopupFailed;
    };

    let Some(stdout) = child.stdout.take() else {
        return ConsentDecision::PopupFailed;
    };

    let result = tokio::time::timeout(Duration::from_mins(1), read_consent_decision(stdout)).await;
    let _ = child.start_kill();
    // The dialog is being torn down, so release the global one-popup permit
    // *before* the reap. `child.wait()` is untimed; holding the sole global
    // permit across it would let a helper that somehow doesn't reap promptly
    // (a pathological uninterruptible-sleep) wedge all consent daemon-wide.
    // Releasing first keeps the popup cap intact while the reap still completes.
    drop(popup_permit);
    let _ = child.wait().await;

    result.unwrap_or(ConsentDecision::Dismissed)
}

/// Basenames of the first-party client binaries that skip the consent
/// popup when co-located with the daemon binary. See
/// [`is_official_client`] for the full trust check.
const OFFICIAL_CLIENT_NAMES: [&str; 3] =
    ["super-stt-app", "super-stt-cli", "super-stt-cosmic-applet"];

/// First-party trust check: does `exe_path` denote one of our own
/// client binaries, installed alongside the daemon binary itself — or, in
/// the macOS app bundle, in the bundle's helper directory (see
/// [`bundle_helpers_dir`])?
///
/// Mirrors the consent-helper security model — co-location
/// with the daemon binary plus the same ownership/permission
/// verification. Writing to the daemon's install directory is already
/// sufficient to replace the daemon, so trusting exact-named sibling
/// binaries adds no new attack surface. Returns a plain bool: failure
/// is the common case (every third-party client) and is deliberately
/// not logged here — the caller logs the rare success.
pub(crate) fn is_official_client(identity: &PeerIdentity) -> bool {
    let PeerIdentity::Native {
        exe_path,
        flatpak_app_id,
    } = identity
    else {
        // A web peer is never first-party. The whole check below is about a
        // binary on this filesystem, and a page has none — there is nothing
        // to canonicalize and no ownership to verify, so the only safe answer
        // is the consent popup. A site calling itself `super-stt-app` must not
        // get within reach of the short-circuit.
        return false;
    };
    // A sandboxed peer is never first-party, whatever its path says. The
    // check below canonicalizes the path against *our* filesystem, and a
    // sandbox is free to put its own binary at /usr/local/bin/super-stt-app;
    // that path would then resolve to the real host binary, pass every test
    // here, and auto-approve a stranger with no popup at all.
    if flatpak_app_id.is_some() {
        return false;
    }
    let Ok(daemon_exe) = std::env::current_exe() else {
        return false;
    };
    let Some(daemon_dir) = daemon_exe.parent() else {
        return false;
    };
    is_official_client_in(daemon_dir, exe_path)
}

/// Testable core of [`is_official_client`] with the daemon's own
/// directory injected. Fail-closed on every non-verifiable branch: a
/// replaced-on-disk exe (`/proc/<pid>/exe` → "… (deleted)") or a
/// symlink resolving outside `daemon_dir` fails canonicalization or
/// the parent check and falls through to the normal consent flow.
fn is_official_client_in(daemon_dir: &Path, exe_path: &Path) -> bool {
    let (Ok(resolved), Ok(daemon_dir)) = (exe_path.canonicalize(), daemon_dir.canonicalize())
    else {
        return false;
    };
    let Some(name) = resolved.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if !OFFICIAL_CLIENT_NAMES.contains(&name) {
        return false;
    }
    let parent = resolved.parent();
    if parent != Some(daemon_dir.as_path()) && parent != bundle_helpers_dir(&daemon_dir).as_deref()
    {
        return false;
    }
    verify_helper_metadata(&resolved).is_ok()
}

/// `Contents/Helpers` of the app bundle the daemon runs from, when
/// `daemon_dir` is a bundle's `Contents/MacOS`.
///
/// The macOS bundle keeps a copy of the CLI there for the shortcut listener.
/// Beside the daemon in `Contents/MacOS`, the listener's event loop would
/// register it with macOS as the app itself, and opening Super STT would
/// activate the listener instead of launching the settings app. The same
/// install writes both directories, and anyone able to write to either can
/// already replace the daemon, so trusting this one adds nothing that
/// co-location did not. The name and metadata checks apply unchanged.
fn bundle_helpers_dir(daemon_dir: &Path) -> Option<PathBuf> {
    let contents = daemon_dir.parent()?;
    let bundle = contents.parent()?;
    let is_bundle = daemon_dir.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app";
    is_bundle.then(|| contents.join("Helpers"))
}

/// Find the consent helper.
///
/// **Security model.** The helper is only ever looked for **alongside the
/// daemon binary itself**. We deliberately do NOT fall back to `PATH`
/// because doing so would let any attacker who can prepend a writable
/// directory to the daemon's `PATH` (a classic privilege-escalation
/// vector) substitute their own helper. Forcing co-location bounds the
/// attack surface to "whoever can write to the directory holding the
/// daemon binary" — which is the same threshold required to replace the
/// daemon itself, so we don't make consent any easier to subvert than
/// the daemon's own integrity.
///
/// On top of that, before returning the path:
/// - We `canonicalize()` it, so symlink-swap shenanigans don't help.
/// - We verify the resolved file is owned by root or the daemon's
///   effective uid (catches "another local user dropped a helper they
///   own into the install dir").
/// - We verify it isn't world-writable.
#[cfg(target_os = "linux")]
pub(crate) fn locate_consent_helper() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join("super-stt-consent");
    if !candidate.exists() {
        log::warn!(
            "super-stt-consent not found alongside daemon binary at {}; \
             auth_request will be denied with popup_failed",
            candidate.display()
        );
        return None;
    }

    let resolved = match candidate.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            log::warn!(
                "failed to canonicalize consent helper path {}: {e}",
                candidate.display()
            );
            return None;
        }
    };

    if let Err(reason) = verify_helper_metadata(&resolved) {
        log::warn!(
            "consent helper at {} rejected: {reason}",
            resolved.display()
        );
        return None;
    }

    Some(resolved)
}

/// Verify the helper's file metadata is consistent with "trusted binary
/// installed by the user". Returns Err with a static reason on
/// rejection.
#[cfg(unix)]
fn verify_helper_metadata(path: &Path) -> Result<(), &'static str> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).map_err(|_| "cannot stat helper")?;
    let our_uid = unsafe { libc::geteuid() };
    check_helper_metadata(metadata.uid(), our_uid, metadata.mode())
}

/// Testable core of [`verify_helper_metadata`]. Trust binaries owned by
/// the daemon's own uid (source/dev installs) or by root (the packaged
/// /usr/local/bin // /usr/bin install) — whoever controls root already
/// controls the daemon binary itself, so root ownership adds no new
/// attack surface. Anything else is another local user's drop-in.
#[cfg(unix)]
fn check_helper_metadata(owner_uid: u32, our_uid: u32, mode: u32) -> Result<(), &'static str> {
    if owner_uid != our_uid && owner_uid != 0 {
        return Err("helper not owned by root or the daemon's effective uid");
    }
    // Reject world-writable helpers — anyone could swap them out.
    if mode & 0o002 != 0 {
        return Err("helper is world-writable");
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_helper_metadata(_: &Path) -> Result<(), &'static str> {
    Ok(())
}

/// Which sandbox, if any, the peer is inside.
///
/// `/proc/<pid>/root` is the peer's root directory. When it is the same
/// directory as ours, the peer shares our view of the filesystem and its exe
/// path means what it says. When it differs, the peer has been pivoted
/// somewhere else and the path has to be read in *that* root, so we ask the
/// sandbox to name itself.
///
/// `Err(())` means the peer is in a root of its own that could not be
/// identified. Callers fail closed on it: an unidentifiable sandbox must not
/// be handed the identity its exe path would otherwise imply, which is
/// whatever host binary happens to sit at the same path.
///
/// Note that a namespace is not by itself a flatpak — a container would land
/// here too, and be refused for the same reason.
#[cfg(target_os = "linux")]
fn peer_sandbox_app_id(pid: u32, context: &str) -> Result<Option<String>, ()> {
    use std::os::unix::fs::MetadataExt as _;

    let root = format!("/proc/{pid}/root");
    let (Ok(peer_root), Ok(our_root)) = (std::fs::metadata(&root), std::fs::metadata("/")) else {
        log::warn!("{context}: cannot stat {root}; refusing to identify peer pid {pid}");
        return Err(());
    };
    if (peer_root.dev(), peer_root.ino()) == (our_root.dev(), our_root.ino()) {
        return Ok(None);
    }

    let info_path = format!("{root}/.flatpak-info");
    let Ok(info) = std::fs::read_to_string(&info_path) else {
        log::warn!(
            "{context}: peer pid {pid} runs in a mount namespace of its own but {info_path} is unreadable; cannot identify it"
        );
        return Err(());
    };
    let Some(app_id) = super_stt_shared::sandbox::app_id_from_info(&info) else {
        log::warn!("{context}: {info_path} names no application; cannot identify peer pid {pid}");
        return Err(());
    };
    Ok(Some(app_id))
}

/// See the Linux [`peer_sandbox_app_id`]. Always `Ok(None)` on macOS.
///
/// Not a stub that gives something up. The Linux version exists because a
/// flatpak peer is pivoted into a root of its own, which makes its exe path
/// a claim about a filesystem this daemon cannot see. macOS has no such
/// pivot: the App Sandbox confines what a process may *open*, but leaves it
/// in the one system root, so `proc_pidpath` returns a path that means here
/// what it means there. There is no second namespace for an identity to be
/// ambiguous across, so there is nothing to disambiguate — and no
/// unidentifiable-sandbox case to fail closed on.
#[cfg(target_os = "macos")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "signature is shared with the Linux arm, which genuinely fails"
)]
fn peer_sandbox_app_id(_pid: u32, _context: &str) -> Result<Option<String>, ()> {
    Ok(None)
}

/// The path of the binary running as `pid`, as the kernel reports it.
///
/// `/proc/<pid>/exe` is a kernel-maintained symlink to the executable the
/// process is running, which is what makes it an identity the daemon can
/// trust rather than something the peer told it.
///
/// `None` (logged with its reason) when the link cannot be read: Yama
/// `ptrace_scope`, systemd `ProtectProc=`, or the peer having exited and its
/// pid been recycled. Callers fail closed.
#[cfg(target_os = "linux")]
fn peer_exe_path(pid: u32, context: &str) -> Option<PathBuf> {
    let path = format!("/proc/{pid}/exe");
    match std::fs::read_link(&path) {
        Ok(p) => Some(p),
        Err(e) => {
            log::warn!("{context}: read_link({path}) failed: {e}; cannot identify peer pid {pid}");
            None
        }
    }
}

/// See the Linux [`peer_exe_path`]. macOS has no `/proc`, so the same fact
/// comes from `proc_pidpath`, which the kernel answers from the process's own
/// `p_textvp` — the vnode it was executed from. Same provenance as the Linux
/// symlink: the peer does not get a say in it.
///
/// `None` when `proc_pidpath` fails, which is the peer having exited (ESRCH)
/// or this daemon lacking the privilege to ask about it (EPERM — another
/// user's process, which the `SO_PEERCRED` uid check upstream already
/// refuses).
///
/// **One guarantee is weaker here than on Linux.** When a binary is replaced
/// on disk while running, Linux renders the link as `/path/to/exe (deleted)`,
/// so the daemon sees that the file behind a minted token is no longer the
/// one it approved. `proc_pidpath` reports only the path, and a path whose
/// file was swapped still resolves. A token stays bound to the path across
/// such a swap rather than being invalidated by it. The swap still requires
/// write access to the install directory — the same access needed to replace
/// the daemon itself — so it does not open a new door, but it does mean the
/// exe-change revocation is a Linux-only belt on top of that braces.
#[cfg(target_os = "macos")]
fn peer_exe_path(pid: u32, context: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt as _;

    let Ok(pid) = i32::try_from(pid) else {
        log::warn!("{context}: peer pid {pid} does not fit in a pid_t; cannot identify it");
        return None;
    };
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buf` is a live allocation of exactly the length passed, and
    // `proc_pidpath` writes at most that many bytes into it.
    let written = unsafe {
        libc::proc_pidpath(
            pid,
            buf.as_mut_ptr().cast::<libc::c_void>(),
            u32::try_from(buf.len()).unwrap_or(u32::MAX),
        )
    };
    if written <= 0 {
        let err = std::io::Error::last_os_error();
        log::warn!("{context}: proc_pidpath({pid}) failed: {err}; cannot identify peer pid {pid}");
        return None;
    }
    // `proc_pidpath` returns the byte length written, terminator excluded.
    // `written` is positive here — the `<= 0` branch above returned.
    let Ok(len) = usize::try_from(written) else {
        return None;
    };
    buf.truncate(len);
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf)))
}

/// Resolve who is calling from the [`PeerInfo`] the accept loop attached.
/// Returns `None` when the peer can't be identified — a missing
/// `PeerInfo`/pid (`SO_PEERCRED` unsupported, peer process gone), a
/// kernel-denied executable lookup (Yama `ptrace_scope`, systemd
/// `ProtectProc=`, pid recycling — see [`peer_exe_path`]), or a sandbox that
/// would not name itself.
///
/// `context` names the caller in the log line, since both ends of a session's
/// life resolve the peer here: `auth_request` at mint time, and the
/// per-request authorization check on every call after it.
///
/// A peer that arrived over TCP has no kernel-attested identity at all, so it
/// is resolved from its `Origin` instead — see [`PeerInfo::web_origin`]. That
/// field is only ever set by the origin gate, which has already checked the
/// value against the user's allowlist; nothing here re-derives it from a
/// header, so there is exactly one place an origin can enter the system.
///
/// The caller **must fail closed** on `None`: the consent model verifies a
/// *binary*, so an unidentifiable peer must not be prompted for (a
/// `<unknown>`-labelled dialog is meaningless to approve) nor minted a token
/// bound to a bogus identity that the `/events` exe-watch would then spuriously
/// revoke (audit 2 Tier 3 #9). Each failure is logged with its specific reason.
pub(crate) fn resolve_peer_identity(
    peer: Option<&PeerInfo>,
    context: &str,
) -> Option<PeerIdentity> {
    let Some(peer) = peer else {
        log::warn!(
            "{context}: no PeerInfo extension attached — cannot identify the requesting binary"
        );
        return None;
    };
    if let Some(origin) = &peer.web_origin {
        return Some(PeerIdentity::Web {
            origin: origin.clone(),
        });
    }
    let Some(pid) = peer.pid else {
        log::warn!(
            "{context}: PeerInfo had no pid (SO_PEERCRED returned no credentials); cannot resolve exe"
        );
        return None;
    };
    let exe_path = peer_exe_path(pid, context)?;
    let flatpak_app_id = peer_sandbox_app_id(pid, context).ok()?;

    Some(PeerIdentity::Native {
        exe_path,
        flatpak_app_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The macOS dialog is one text field, so the app name — the one part of
    /// it the *caller* chooses — is the only place a forged line could come
    /// from. These pin the flattening that prevents it.
    #[cfg(target_os = "macos")]
    mod display_name {
        use super::super::PeerIdentity;
        use super::super::{MAX_DISPLAY_NAME, consent_dialog_text, sanitize_display_name};
        use std::path::PathBuf;

        /// The attack this function exists for: an app name carrying a
        /// newline and a plausible `Executable:` line, which in a plain text
        /// field would read as the daemon's own attestation about the
        /// caller's binary.
        #[test]
        fn a_newline_cannot_forge_a_second_line() {
            let forged = sanitize_display_name("Evil\nExecutable:  /usr/local/bin/super-stt-app");
            assert!(!forged.contains('\n'), "{forged:?} still spans two lines");
            assert_eq!(
                forged, "Evil Executable: /usr/local/bin/super-stt-app",
                "the text should survive, visibly, on one line"
            );
        }

        /// Every control character, not just `\n`. A carriage return alone
        /// repositions the cursor in some renderers, and a vertical tab is a
        /// line break in others.
        #[test]
        fn every_control_character_is_flattened() {
            for c in ['\n', '\r', '\t', '\u{000b}', '\u{000c}', '\u{0085}'] {
                let out = sanitize_display_name(&format!("a{c}b"));
                assert_eq!(out, "a b", "control character {c:?} survived");
            }
        }

        /// A name long enough to push the rest of the dialog off screen is
        /// cut, and marked as cut.
        #[test]
        fn an_over_long_name_is_truncated() {
            let out = sanitize_display_name(&"x".repeat(MAX_DISPLAY_NAME * 3));
            assert!(out.chars().count() <= MAX_DISPLAY_NAME, "{out:?}");
            assert!(out.ends_with('…'), "truncation should be visible: {out:?}");
        }

        /// An empty or blank name yields an empty string rather than
        /// whitespace, so the caller's "An application" fallback triggers.
        #[test]
        fn a_blank_name_is_empty() {
            assert_eq!(sanitize_display_name(""), "");
            assert_eq!(sanitize_display_name("   \n\t "), "");
        }

        /// End to end: the composed dialog must name the kernel-reported
        /// executable exactly once, however hard the app name tries to add
        /// another.
        #[test]
        fn the_dialog_carries_one_executable_line() {
            let identity = PeerIdentity::Native {
                exe_path: PathBuf::from("/usr/bin/curl"),
                flatpak_app_id: None,
            };
            let text = consent_dialog_text(
                "Evil\nExecutable:  /usr/local/bin/super-stt-app",
                &["status".to_string()],
                &identity,
            );
            assert_eq!(
                text.lines()
                    .filter(|l| l.starts_with("Executable:"))
                    .count(),
                1,
                "exactly one line may claim to be the executable:\n{text}"
            );
            assert!(text.contains("Executable:  /usr/bin/curl"), "{text}");
            // And the scope's real description is present, from the shared
            // table the Linux helper renders from.
            assert!(
                text.contains("Read which speech-to-text model and device are currently active"),
                "{text}"
            );
        }

        /// A blank name falls back to a neutral label rather than leaving the
        /// sentence starting with "wants access".
        #[test]
        fn a_blank_name_becomes_a_neutral_label() {
            let identity = PeerIdentity::Native {
                exe_path: PathBuf::from("/usr/bin/curl"),
                flatpak_app_id: None,
            };
            let text = consent_dialog_text("  ", &["status".to_string()], &identity);
            assert!(
                text.starts_with("An application wants access to Super STT."),
                "{text}"
            );
        }
    }

    /// `check_helper_metadata`: the ownership/permission gate shared by
    /// the consent-helper lookup and the official-client trust check.
    mod helper_metadata {
        use super::super::check_helper_metadata;

        #[test]
        fn owned_by_daemon_uid_is_trusted() {
            assert!(check_helper_metadata(1000, 1000, 0o755).is_ok());
        }

        #[test]
        fn root_owned_is_trusted() {
            // The packaged install (/usr/local/bin, /usr/bin) is
            // root-owned; root could already replace the daemon binary
            // itself, so this adds no new attack surface.
            assert!(check_helper_metadata(0, 1000, 0o755).is_ok());
        }

        #[test]
        fn other_local_user_is_rejected() {
            assert!(check_helper_metadata(1001, 1000, 0o755).is_err());
        }

        #[test]
        fn world_writable_is_rejected_even_when_root_owned() {
            assert!(check_helper_metadata(0, 1000, 0o757).is_err());
        }
    }

    #[test]
    fn normalize_sorts_and_dedups() {
        let got = normalize_scopes(&[
            "transcribe".to_string(),
            "status".to_string(),
            "transcribe".to_string(),
        ]);
        assert_eq!(got, vec!["status".to_string(), "transcribe".to_string()]);
    }

    #[test]
    fn normalize_is_order_independent() {
        let a = normalize_scopes(&["settings".to_string(), "status".to_string()]);
        let b = normalize_scopes(&["status".to_string(), "settings".to_string()]);
        assert_eq!(a, b, "request order must not change the consent key");
    }

    /// First-party trust check (`is_official_client_in`): exact-name
    /// allowlist + co-location with the daemon dir + metadata
    /// verification, fail-closed on every non-verifiable branch.
    mod official_client {
        use super::super::is_official_client_in;
        use std::os::unix::fs::PermissionsExt;
        use std::path::{Path, PathBuf};

        fn write_executable(dir: &Path, name: &str, mode: u32) -> PathBuf {
            let path = dir.join(name);
            std::fs::write(&path, b"\x7fELF").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            path
        }

        #[test]
        fn official_name_co_located_is_trusted() {
            let dir = tempfile::tempdir().unwrap();
            let app = write_executable(dir.path(), "super-stt-app", 0o755);
            assert!(is_official_client_in(dir.path(), &app));
        }

        #[test]
        fn unlisted_name_co_located_is_rejected() {
            let dir = tempfile::tempdir().unwrap();
            let other = write_executable(dir.path(), "super-stt-extra", 0o755);
            assert!(
                !is_official_client_in(dir.path(), &other),
                "co-location alone must not confer trust"
            );
        }

        #[test]
        fn official_name_in_foreign_dir_is_rejected() {
            let daemon_dir = tempfile::tempdir().unwrap();
            let foreign = tempfile::tempdir().unwrap();
            let app = write_executable(foreign.path(), "super-stt-app", 0o755);
            assert!(
                !is_official_client_in(daemon_dir.path(), &app),
                "an official name outside the daemon dir must not be trusted"
            );
        }

        #[test]
        fn world_writable_official_binary_is_rejected() {
            let dir = tempfile::tempdir().unwrap();
            let app = write_executable(dir.path(), "super-stt-cli", 0o757);
            assert!(
                !is_official_client_in(dir.path(), &app),
                "a world-writable binary could be swapped by anyone"
            );
        }

        #[test]
        fn missing_exe_is_rejected() {
            let dir = tempfile::tempdir().unwrap();
            assert!(
                !is_official_client_in(dir.path(), &dir.path().join("super-stt-app")),
                "a nonexistent (e.g. replaced-on-disk) exe must fail closed"
            );
        }

        /// A daemon in `<name>.app/Contents/MacOS`, and that bundle's
        /// `Contents/Helpers`, under a fresh temp dir.
        fn app_bundle(root: &Path, name: &str) -> (PathBuf, PathBuf) {
            let contents = root.join(name).join("Contents");
            let (macos, helpers) = (contents.join("MacOS"), contents.join("Helpers"));
            std::fs::create_dir_all(&macos).unwrap();
            std::fs::create_dir_all(&helpers).unwrap();
            (macos, helpers)
        }

        #[test]
        fn official_name_in_the_daemons_bundle_helpers_is_trusted() {
            let root = tempfile::tempdir().unwrap();
            let (macos, helpers) = app_bundle(root.path(), "Super STT.app");
            let cli = write_executable(&helpers, "super-stt-cli", 0o755);
            assert!(is_official_client_in(&macos, &cli));
        }

        #[test]
        fn helpers_of_a_directory_that_is_not_an_app_bundle_are_rejected() {
            let root = tempfile::tempdir().unwrap();
            let (macos, helpers) = app_bundle(root.path(), "Super STT");
            let cli = write_executable(&helpers, "super-stt-cli", 0o755);
            assert!(
                !is_official_client_in(&macos, &cli),
                "only an app bundle's own Contents/Helpers is trusted"
            );
        }

        #[test]
        fn helpers_of_another_app_bundle_are_rejected() {
            let root = tempfile::tempdir().unwrap();
            let (macos, _) = app_bundle(root.path(), "Super STT.app");
            let (_, other_helpers) = app_bundle(root.path(), "Other.app");
            let cli = write_executable(&other_helpers, "super-stt-cli", 0o755);
            assert!(!is_official_client_in(&macos, &cli));
        }

        #[test]
        fn world_writable_binary_in_bundle_helpers_is_rejected() {
            let root = tempfile::tempdir().unwrap();
            let (macos, helpers) = app_bundle(root.path(), "Super STT.app");
            let cli = write_executable(&helpers, "super-stt-cli", 0o757);
            assert!(!is_official_client_in(&macos, &cli));
        }

        #[test]
        fn symlink_resolving_outside_daemon_dir_is_rejected() {
            let daemon_dir = tempfile::tempdir().unwrap();
            let foreign = tempfile::tempdir().unwrap();
            let target = write_executable(foreign.path(), "super-stt-cli", 0o755);
            let link = daemon_dir.path().join("super-stt-cli");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(
                !is_official_client_in(daemon_dir.path(), &link),
                "canonicalization must unmask a symlink escaping the daemon dir"
            );
        }
    }
}
