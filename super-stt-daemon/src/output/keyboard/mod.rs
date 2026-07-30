// SPDX-License-Identifier: GPL-3.0-only

mod enigo_backend;
mod xdg_portal_backend;
mod ydotool_backend;

use anyhow::Result;
use log::{debug, warn};
use super_stt_shared::models::write_method::WriteMethod;

use enigo_backend::EnigoBackend;
use xdg_portal_backend::XdgPortalBackend;
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
    WaylandProtocol(Box<EnigoBackend>),
    Ydotool(YdotoolBackend),
    XdgPortal(XdgPortalBackend),
    /// Test-only backend that records what *would* have been typed. The three
    /// real backends each need a live compositor or portal, so without this the
    /// typing path cannot be asserted on at all.
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
    /// Returns an error only when a *specific* method is requested and fails.
    /// `Auto` always falls back to Wayland protocol.
    pub async fn new(method: WriteMethod) -> Result<Self> {
        let sim = match method {
            WriteMethod::Auto => Self::auto().await?,
            WriteMethod::XdgDesktopPortal => {
                let backend = XdgPortalBackend::new().await?;
                Self::XdgPortal(backend)
            }
            WriteMethod::Ydotool => {
                anyhow::ensure!(YdotoolBackend::is_available(), "ydotool is not available");
                Self::Ydotool(YdotoolBackend::new())
            }
            WriteMethod::WaylandProtocol => Self::WaylandProtocol(Box::new(EnigoBackend::new()?)),
        };
        Ok(sim)
    }

    /// Auto-detect: XDG Portal → ydotool → Wayland protocol.
    async fn auto() -> Result<Self> {
        debug!("Auto-detecting write method...");

        let portal_available = XdgPortalBackend::is_available().await;
        debug!("XDG Desktop Portal available: {portal_available}");
        if portal_available {
            match XdgPortalBackend::new().await {
                Ok(backend) => return Ok(Self::XdgPortal(backend)),
                Err(e) => warn!("XDG Portal available but session failed: {e}"),
            }
        }

        let ydotool_available = YdotoolBackend::is_available();
        debug!("ydotool available: {ydotool_available}");
        if ydotool_available {
            return Ok(Self::Ydotool(YdotoolBackend::new()));
        }

        debug!("Falling back to Wayland protocol");
        Ok(Self::WaylandProtocol(Box::new(EnigoBackend::new()?)))
    }

    /// Whether this backend may be held across recordings.
    ///
    /// Everything except enigo is cached. Rebuilding the portal session costs
    /// three D-Bus round-trips before capture can start and may prompt the
    /// user for authorization each time, so paying it per recording is not an
    /// option. enigo is the exception: Wayland compositors recycle idle
    /// connections, leaving a stale `Con` that fails silently on the next
    /// recording, and recreating it is cheap.
    #[must_use]
    pub fn is_cacheable(&self) -> bool {
        !matches!(self, Self::WaylandProtocol(_))
    }

    /// Human-readable name for logging.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::XdgPortal(_) => "XDG Desktop Portal",
            Self::Ydotool(_) => "ydotool",
            Self::WaylandProtocol(_) => "Wayland protocol",
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
    pub async fn type_text(&mut self, text: &str) -> Result<()> {
        match self {
            // The enigo/ydotool backends are synchronous and `!Send`; run them
            // under `block_in_place` so their handle never crosses an await and
            // the runtime spins up a replacement worker rather than stalling. The
            // portal backend is genuinely async — await it directly.
            Self::WaylandProtocol(b) => tokio::task::block_in_place(|| b.type_text(text)),
            Self::Ydotool(_) => tokio::task::block_in_place(|| YdotoolBackend::type_text(text)),
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
    pub async fn backspace_n(&mut self, n: usize) -> Result<()> {
        match self {
            Self::WaylandProtocol(b) => {
                tokio::task::block_in_place(|| b.backspace_n(n));
                Ok(())
            }
            Self::Ydotool(_) => tokio::task::block_in_place(|| YdotoolBackend::backspace_n(n)),
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
