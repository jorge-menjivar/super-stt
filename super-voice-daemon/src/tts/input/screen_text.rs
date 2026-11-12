// SPDX-License-Identifier: GPL-3.0-only
//! The text the read method names: the highlighted text or the clipboard.
//!
//! On Wayland the highlighted text is the primary selection, and the clipboard
//! is the regular selection. The daemon has no window, and a client without
//! keyboard focus cannot read either through the core protocols. The
//! data-control protocol (`ext_data_control_v1`, or wlroots'
//! `zwlr_data_control_v1`) is for exactly this kind of client, and COSMIC offers
//! both. The daemon's unit waits for `WAYLAND_DISPLAY`, so the session is there
//! to connect to.

use super_voice_shared::models::read_method::ReadMethod;

/// Read the text `method` names, or `None` when there is none: nothing
/// selected or copied, or only something that is not text, like an image.
///
/// Reads at most `max_bytes`; the caller decides what more than that means.
/// Blocks until the app that owns the text has written it, so call it off the
/// async runtime.
///
/// # Errors
///
/// When there is no Wayland session, the compositor offers no data-control
/// protocol, or the transfer fails.
#[cfg(target_os = "linux")]
pub fn read(method: ReadMethod, max_bytes: u64) -> Result<Option<String>, String> {
    use std::io::Read;
    use wl_clipboard_rs::paste::{self, ClipboardType, MimeType, Seat};

    let clipboard = match method {
        ReadMethod::Selection => ClipboardType::Primary,
        ReadMethod::Clipboard => ClipboardType::Regular,
    };
    let pipe = match paste::get_contents(clipboard, Seat::Unspecified, MimeType::Text) {
        Ok((pipe, _)) => pipe,
        Err(paste::Error::ClipboardEmpty | paste::Error::NoMimeType) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    pipe.take(max_bytes)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok((!text.trim().is_empty()).then_some(text))
}

/// Only Wayland has the selection and clipboard this reads.
///
/// # Errors
///
/// Always.
#[cfg(not(target_os = "linux"))]
pub fn read(_method: ReadMethod, _max_bytes: u64) -> Result<Option<String>, String> {
    Err("reading the screen's text is not supported on this platform".to_string())
}
