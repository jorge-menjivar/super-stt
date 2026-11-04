// SPDX-License-Identifier: GPL-3.0-only

mod enigo_backend;
#[cfg(target_os = "linux")]
mod xdg_portal_backend;
#[cfg(target_os = "linux")]
mod ydotool_backend;

use anyhow::Result;
use log::debug;
#[cfg(target_os = "linux")]
use log::warn;
use super_stt_shared::models::write_method::WriteMethod;

use enigo_backend::EnigoBackend;
#[cfg(target_os = "linux")]
use xdg_portal_backend::XdgPortalBackend;
#[cfg(target_os = "linux")]
use ydotool_backend::YdotoolBackend;

/// Keyboard simulation backend.
///
/// # Safety
///
/// `Simulator` is `Send + Sync` because it is only ever accessed by one
/// recording session at a time (guarded by `busy`). The `!Send`
/// inner type (`Enigo` with raw xkbcommon pointers) is never used
/// concurrently.
pub enum Simulator {
    /// [`WriteMethod::BuiltIn`] — enigo, which drives
    /// `zwp_virtual_keyboard_manager_v1` on Linux and CoreGraphics on macOS.
    /// The only backend that exists on both.
    BuiltIn(Box<EnigoBackend>),
    #[cfg(target_os = "linux")]
    Ydotool(YdotoolBackend),
    #[cfg(target_os = "linux")]
    XdgPortal(XdgPortalBackend),
    /// Test-only backend that records what *would* have been typed. Every
    /// real backend needs something a test host does not have — a live
    /// compositor, a portal, or the macOS Accessibility grant — so without
    /// this the typing path cannot be asserted on at all.
    #[cfg(test)]
    Capture(std::sync::Arc<std::sync::Mutex<String>>),
}

// SAFETY: see Simulator doc comment — single-writer access enforced by daemon.
unsafe impl Send for Simulator {}
unsafe impl Sync for Simulator {}

impl Simulator {
    /// Create a simulator for the requested write method.
    ///
    /// # Errors
    /// Returns an error when a *specific* method is requested and fails, or —
    /// for `Auto` — when every backend in the chain is unavailable.
    pub async fn new(method: WriteMethod) -> Result<Self> {
        let sim = match method {
            WriteMethod::Auto => Self::auto().await?,
            #[cfg(target_os = "linux")]
            WriteMethod::XdgDesktopPortal => {
                let backend = XdgPortalBackend::new().await?;
                Self::XdgPortal(backend)
            }
            #[cfg(target_os = "linux")]
            WriteMethod::Ydotool => {
                anyhow::ensure!(YdotoolBackend::is_available(), "ydotool is not available");
                Self::Ydotool(YdotoolBackend::new())
            }
            WriteMethod::BuiltIn => Self::BuiltIn(Box::new(EnigoBackend::new()?)),
            // Asking for a Linux session technology on macOS is not a runtime
            // failure to retry but a request that can never be satisfied, so
            // it is named as such rather than reported as "unavailable".
            #[cfg(not(target_os = "linux"))]
            other => anyhow::bail!(
                "write method `{other}` is Linux-only and cannot be used on this platform"
            ),
        };
        Ok(sim)
    }

    /// Auto-detect: built-in → XDG Portal → ydotool.
    ///
    /// The built-in backend leads because it needs nothing installed beyond a
    /// compositor exposing `zwp_virtual_keyboard_manager_v1`, types the user's
    /// actual layout, and costs no D-Bus round-trips or authorization prompt.
    /// The portal follows for sessions that withhold the virtual-keyboard
    /// global, and ydotool last since it needs a running `ydotoold` and types a
    /// hardcoded US-QWERTY map.
    ///
    /// # Errors
    /// Only when every backend is unavailable. The message carries each rung's
    /// reason: a failed recording is the daemon's sole chance to explain why
    /// nothing can type.
    #[cfg(target_os = "linux")]
    async fn auto() -> Result<Self> {
        debug!("Auto-detecting write method...");
        let mut unavailable = Vec::new();

        match EnigoBackend::new() {
            Ok(backend) => return Ok(Self::BuiltIn(Box::new(backend))),
            Err(e) => {
                debug!("Built-in write method unavailable: {e}");
                unavailable.push(format!("built-in ({e})"));
            }
        }

        let portal_available = XdgPortalBackend::is_available().await;
        debug!("XDG Desktop Portal available: {portal_available}");
        if portal_available {
            match XdgPortalBackend::new().await {
                Ok(backend) => return Ok(Self::XdgPortal(backend)),
                Err(e) => {
                    warn!("XDG Portal available but session failed: {e}");
                    unavailable.push(format!("XDG Desktop Portal ({e})"));
                }
            }
        } else {
            unavailable
                .push("XDG Desktop Portal (RemoteDesktop interface not on the session bus)".into());
        }

        let ydotool_available = YdotoolBackend::is_available();
        debug!("ydotool available: {ydotool_available}");
        if ydotool_available {
            return Ok(Self::Ydotool(YdotoolBackend::new()));
        }
        unavailable.push("ydotool (not installed or ydotoold not running)".into());

        anyhow::bail!(
            "no write method available — tried {}",
            unavailable.join(", ")
        )
    }

    /// Auto-detect on macOS, where the chain has one rung.
    ///
    /// Not a degenerate case of the Linux chain. The two fallbacks there
    /// exist because a Wayland compositor may withhold the virtual-keyboard
    /// global, leaving a working session with no way to type; CoreGraphics
    /// event posting has no such variation between machines. What gates it
    /// instead is a permission — the daemon must hold Accessibility — and a
    /// second backend would not help, because every way of synthesizing a
    /// key event on macOS is behind the same grant.
    ///
    /// So there is nothing to fall back *to*, and the failure is reported
    /// rather than walked around. enigo checks the grant itself
    /// (`AXIsProcessTrusted`) and reports it distinctly, so the message a
    /// user sees already names the checkbox — see
    /// `enigo_backend::describe_connection_failure`.
    ///
    /// # Errors
    /// When enigo cannot open a keyboard connection: no Accessibility grant,
    /// or no window server to talk to at all (an ssh session, or a
    /// `LaunchDaemon` in the system context rather than a `LaunchAgent` in the
    /// user's).
    // `allow`, not `expect`: clippy 1.98 files these under `unused_async_trait_impl`
    // and later releases under `unused_async`, so either `expect` goes unfulfilled
    // on some toolchain.
    #[cfg(target_os = "macos")]
    #[allow(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "shares a signature with the Linux arm, which awaits the portal probe"
    )]
    async fn auto() -> Result<Self> {
        debug!("Auto-detecting write method...");
        match EnigoBackend::new() {
            Ok(backend) => Ok(Self::BuiltIn(Box::new(backend))),
            // Not prefixed with "no write method available — tried …" the way
            // the Linux arm is: there is one method here, and its own error
            // already says what to do about it.
            Err(e) => Err(e),
        }
    }

    /// Whether this backend may be held across recordings.
    ///
    /// Everything except the built-in backend is cached. Rebuilding the
    /// portal session costs
    /// three D-Bus round-trips before capture can start and may prompt the
    /// user for authorization each time, so paying it per recording is not an
    /// option. enigo is the exception: Wayland compositors recycle idle
    /// connections, leaving a stale `Con` that fails silently on the next
    /// recording, and recreating it is cheap.
    #[must_use]
    pub fn is_cacheable(&self) -> bool {
        !matches!(self, Self::BuiltIn(_))
    }

    /// The concrete method this simulator drives.
    ///
    /// Never `Auto` in a shipped build: `auto()` resolves the chain at
    /// construction, and this is the only way a client can learn which rung it
    /// landed on (`POST /write_method/test`). The test-only capture backend
    /// types through no real method and so reports the unresolved `Auto`.
    #[must_use]
    pub fn resolved_method(&self) -> WriteMethod {
        match self {
            #[cfg(target_os = "linux")]
            Self::XdgPortal(_) => WriteMethod::XdgDesktopPortal,
            #[cfg(target_os = "linux")]
            Self::Ydotool(_) => WriteMethod::Ydotool,
            Self::BuiltIn(_) => WriteMethod::BuiltIn,
            #[cfg(test)]
            Self::Capture(_) => WriteMethod::Auto,
        }
    }

    /// Human-readable name for logging.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Self::XdgPortal(_) => "XDG Desktop Portal",
            #[cfg(target_os = "linux")]
            Self::Ydotool(_) => "ydotool",
            Self::BuiltIn(_) => "built-in",
            #[cfg(test)]
            Self::Capture(_) => "capture (test)",
        }
    }

    /// Type text using the active backend. Async so the portal backend awaits
    /// its D-Bus calls directly and the blocking backends yield the worker
    /// (audit Tier 3 #35).
    ///
    /// # Errors
    /// Returns an error if the backend fails to simulate key input.
    ///
    /// # Panics
    /// The test-only capture backend panics if its buffer mutex is poisoned.
    // `allow`, not `expect`: clippy 1.98 files these under `unused_async_trait_impl`
    // and later releases under `unused_async`, so either `expect` goes unfulfilled
    // on some toolchain.
    #[cfg_attr(
        target_os = "macos",
        allow(
            clippy::unused_async,
            clippy::unused_async_trait_impl,
            reason = "the awaiting arm is the XDG portal backend, which is Linux-only"
        )
    )]
    pub async fn type_text(&mut self, text: &str) -> Result<()> {
        match self {
            // The enigo/ydotool backends are synchronous and `!Send`; run them
            // under `block_in_place` so their handle never crosses an await and
            // the runtime spins up a replacement worker rather than stalling. The
            // portal backend is genuinely async — await it directly.
            Self::BuiltIn(b) => tokio::task::block_in_place(|| b.type_text(text)),
            #[cfg(target_os = "linux")]
            Self::Ydotool(_) => tokio::task::block_in_place(|| YdotoolBackend::type_text(text)),
            #[cfg(target_os = "linux")]
            Self::XdgPortal(b) => b.type_text(text).await,
            #[cfg(test)]
            Self::Capture(buf) => {
                buf.lock().expect("capture buffer poisoned").push_str(text);
                Ok(())
            }
        }
    }

    /// Backspace N characters using the active backend.
    ///
    /// # Errors
    /// Returns an error if the backend fails to simulate key input.
    ///
    /// # Panics
    /// The test-only capture backend panics if its buffer mutex is poisoned.
    // `allow`, not `expect`: clippy 1.98 files these under `unused_async_trait_impl`
    // and later releases under `unused_async`, so either `expect` goes unfulfilled
    // on some toolchain.
    #[cfg_attr(
        target_os = "macos",
        allow(
            clippy::unused_async,
            clippy::unused_async_trait_impl,
            reason = "the awaiting arm is the XDG portal backend, which is Linux-only"
        )
    )]
    pub async fn backspace_n(&mut self, n: usize) -> Result<()> {
        match self {
            Self::BuiltIn(b) => tokio::task::block_in_place(|| b.backspace_n(n)),
            #[cfg(target_os = "linux")]
            Self::Ydotool(_) => tokio::task::block_in_place(|| YdotoolBackend::backspace_n(n)),
            #[cfg(target_os = "linux")]
            Self::XdgPortal(b) => b.backspace_n(n).await,
            #[cfg(test)]
            Self::Capture(buf) => {
                let mut guard = buf.lock().expect("capture buffer poisoned");
                // Truncate by chars, not bytes — a real backspace removes one
                // grapheme, and truncating mid-UTF-8 would panic.
                let keep = guard.chars().count().saturating_sub(n);
                *guard = guard.chars().take(keep).collect();
                Ok(())
            }
        }
    }
}

#[cfg(test)]
impl Simulator {
    /// A simulator that accumulates typed text instead of driving a keyboard.
    /// Returns the simulator and a handle to the accumulated text.
    pub(crate) fn capture() -> (Self, std::sync::Arc<std::sync::Mutex<String>>) {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        (Self::Capture(std::sync::Arc::clone(&buf)), buf)
    }
}

#[cfg(test)]
mod tests {
    use super::Simulator;

    /// The capture backend has to behave like a real one — text accumulates and
    /// backspace removes trailing *characters* (not bytes) — or tests written
    /// against it will not reflect what lands in a user's window.
    #[tokio::test]
    async fn capture_backend_accumulates_text_and_honors_backspace() {
        let (mut sim, buf) = Simulator::capture();

        sim.type_text("hello").await.expect("type");
        sim.type_text(" wörld").await.expect("type");
        assert_eq!(*buf.lock().unwrap(), "hello wörld");

        // Multi-byte char must be removed whole.
        sim.backspace_n(4).await.expect("backspace");
        assert_eq!(*buf.lock().unwrap(), "hello w");

        assert_eq!(sim.name(), "capture (test)");
    }

    /// Caching is the default; only enigo opts out. A regression that inverts
    /// this rebuilds the portal session before every recording, costing three
    /// D-Bus round-trips and possibly an authorization prompt. enigo itself
    /// needs a live compositor to construct, so this pins the side of the rule
    /// that is reachable in a test.
    #[test]
    fn backends_are_cached_by_default() {
        let (sim, _buf) = Simulator::capture();
        assert!(sim.is_cacheable());
    }
}

/// Ask macOS, once at daemon startup, whether the daemon may type — and let
/// the system's Accessibility prompt appear if it may not.
///
/// Without Accessibility the daemon transcribes perfectly and types nothing,
/// which is the worst shape a failure can take: everything looks like it
/// worked. The grant cannot be requested programmatically — only the user can
/// give it, in System Settings — so the most the daemon can do is ask at a
/// moment when asking is not disruptive, and say clearly what is wrong when
/// the answer is no.
///
/// Startup is that moment. The alternative, prompting from the recording
/// path, puts a modal dialog on screen in the instant between the user
/// finishing a sentence and expecting it to appear, stealing focus from the
/// window they were dictating into. So this is the one place that prompts,
/// and [`EnigoBackend::new`] never does.
///
/// Advisory only: a `false` here does not stop the daemon. Transcription over
/// the HTTP protocol works without ever typing anything, and a client that
/// only reads transcripts has no use for the grant.
#[cfg(target_os = "macos")]
pub fn probe_accessibility_permission() {
    match EnigoBackend::probe_accessibility_permission() {
        Ok(()) => debug!("Accessibility permission granted; write mode can type"),
        Err(e) => log::warn!(
            "Write mode will not be able to type: {e} \
             (transcription itself is unaffected)"
        ),
    }
}
