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

    /// Human-readable name for logging.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::XdgPortal(_) => "XDG Desktop Portal",
            Self::Ydotool(_) => "ydotool",
            Self::WaylandProtocol(_) => "Wayland protocol",
        }
    }

    /// Type text using the active backend.
    ///
    /// # Errors
    /// Returns an error if the backend fails to simulate key input.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        match self {
            Self::WaylandProtocol(b) => b.type_text(text),
            Self::Ydotool(_) => YdotoolBackend::type_text(text),
            Self::XdgPortal(b) => b.type_text(text),
        }
    }

    /// Backspace N characters using the active backend.
    ///
    /// # Errors
    /// Returns an error if the backend fails to simulate key input.
    pub fn backspace_n(&mut self, n: usize) -> Result<()> {
        match self {
            Self::WaylandProtocol(b) => {
                b.backspace_n(n);
                Ok(())
            }
            Self::Ydotool(_) => YdotoolBackend::backspace_n(n),
            Self::XdgPortal(b) => b.backspace_n(n),
        }
    }
}
