// SPDX-License-Identifier: GPL-3.0-only
//! `/write_method` — text-output strategy (auto, xdotool, clipboard, …).

use super_stt_shared::models::write_method::WriteMethod;

settings_getter!(
    get_write_method -> String, "/settings/write_method", "get_write_method",
    |resp| resp.write_method.unwrap_or_else(|| "auto".to_string())
);
settings_setter!(set_write_method, method: String, "/settings/write_method", "method", "set_write_method");

/// Type a fixed test string with the configured method
/// (HTTP `POST /write_method/test`), returning the backend that actually
/// typed. With `auto` configured that resolved name is the only way to see
/// which rung of the chain is in use.
///
/// `None` means the typing succeeded but the daemon did not name a backend
/// this build understands — the caller must show nothing rather than guess,
/// since the obvious guess (`Auto`) is the one value it can never be.
pub async fn test_write_method()
-> super_stt_shared::daemon::http_client::HttpResult<Option<WriteMethod>> {
    crate::daemon::client::internal::session::with_settings_token(|socket, token| async move {
        let resp = crate::daemon::client::internal::response::require_success(
            super_stt_shared::daemon::http_client::transport::settings_post(
                socket,
                &token,
                "/settings/write_method/test",
                &serde_json::json!({}),
            )
            .await?,
            "test_write_method",
        )?;
        Ok(parse_resolved(resp.resolved_write_method.as_deref()))
    })
    .await
}

/// Parse the `resolved_write_method` field. Anything the daemon omits, leaves
/// empty, or names in a token this build doesn't know becomes `None`; so does
/// `auto`, which the contract forbids there and which would otherwise render
/// as a backend in its own right.
fn parse_resolved(raw: Option<&str>) -> Option<WriteMethod> {
    match raw?.parse().ok()? {
        WriteMethod::Auto => None,
        resolved => Some(resolved),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_resolved;
    use super_stt_shared::models::write_method::WriteMethod;

    #[test]
    fn parses_the_three_concrete_backends() {
        assert_eq!(
            parse_resolved(Some("wayland_protocol")),
            Some(WriteMethod::WaylandProtocol)
        );
        assert_eq!(
            parse_resolved(Some("xdg_desktop_portal")),
            Some(WriteMethod::XdgDesktopPortal)
        );
        assert_eq!(parse_resolved(Some("ydotool")), Some(WriteMethod::Ydotool));
    }

    /// The UI shows this as "Active backend", so a value it cannot vouch for
    /// must produce no row at all. Reporting `Auto` there would claim the
    /// chain resolved to itself — the exact silent-default lie this readout
    /// exists to replace.
    #[test]
    fn unknown_absent_or_auto_resolve_to_nothing() {
        assert_eq!(parse_resolved(None), None);
        assert_eq!(parse_resolved(Some("")), None);
        assert_eq!(parse_resolved(Some("clipboard_paste")), None);
        assert_eq!(parse_resolved(Some("auto")), None);
    }
}
