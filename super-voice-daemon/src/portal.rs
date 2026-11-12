// SPDX-License-Identifier: GPL-3.0-only
//! What every XDG desktop portal call shares: the portal's bus name and
//! object, and the `org.freedesktop.portal.Request` round trip a method
//! answers through. The `RemoteDesktop` write method
//! (`output::keyboard::xdg_portal_backend`) and the `GlobalShortcuts`
//! shortcuts (`shortcuts::portal`) both call through it.

use anyhow::{Context, Result};
use futures::StreamExt;
use log::{debug, warn};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// The portal frontend's bus name.
pub(crate) const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
/// The one object every portal interface is on.
pub(crate) const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";

/// A `handle_token` no other request from this process is using.
pub(crate) fn next_handle_token() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("superstt_r{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// The object path the portal gives a request that `unique_name` made with
/// `handle_token`, per the `org.freedesktop.portal.Request` naming scheme.
fn request_path(unique_name: &str, handle_token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("{PORTAL_PATH}/request/{sender}/{handle_token}")
}

/// Start listening for `Response` on one request path.
async fn subscribe_response(
    conn: &zbus::Connection,
    path: String,
) -> Result<zbus::proxy::SignalStream<'static>> {
    let proxy = zbus::Proxy::new(conn, PORTAL_BUS, path, REQUEST_IFACE).await?;
    Ok(proxy.receive_signal("Response").await?)
}

/// Call a portal method and wait for its Response signal.
///
/// Subscribes before calling. The portal can answer before the call has even
/// returned: on COSMIC, `Response` arrived 0.2 ms after the reply carrying the
/// request path, and a subscription made after that reply landed 0.7 ms too
/// late. The answer was lost and every step timed out, so the dialog at
/// `Start` almost never came up. The portal documents this race and its fix:
/// the caller sends `handle_token` in the method's options, which makes the
/// request path predictable before the call.
pub(crate) async fn portal_call(
    conn: &zbus::Connection,
    portal: &zbus::Proxy<'_>,
    method: &str,
    handle_token: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
    timeout_secs: u64,
) -> Result<HashMap<String, OwnedValue>> {
    let (code, results) =
        portal_request(conn, portal, method, handle_token, body, timeout_secs).await?;
    if code != 0 {
        return Err(anyhow::anyhow!(
            "Portal {method} failed (response code {code})"
        ));
    }
    Ok(results)
}

/// How a request ended, as its `Response` says: the user said no in the
/// desktop's dialog.
pub(crate) const RESPONSE_CANCELLED: u32 = 1;

/// How a request ended, as its `Response` says: another way than the user
/// saying yes or no. GNOME answers its dialog's Cancel this way.
pub(crate) const RESPONSE_ENDED: u32 = 2;

/// [`portal_call`], answering the `Response`'s code with its results rather
/// than refusing any but `0`, for a caller that tells a cancelled dialog
/// ([`RESPONSE_CANCELLED`]) from a failure.
pub(crate) async fn portal_request(
    conn: &zbus::Connection,
    portal: &zbus::Proxy<'_>,
    method: &str,
    handle_token: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
    timeout_secs: u64,
) -> Result<(u32, HashMap<String, OwnedValue>)> {
    let unique_name = conn
        .unique_name()
        .context("Session bus connection has no unique name")?;
    let expected = request_path(unique_name.as_str(), handle_token);
    let mut signals = subscribe_response(conn, expected.clone()).await?;

    let request_path: OwnedObjectPath = portal
        .call(method, body)
        .await
        .context(format!("Portal {method} call failed"))?;

    debug!("Portal {method}: request path = {request_path}");

    if request_path.as_str() != expected {
        // A portal older than 0.9 ignores `handle_token` and picks its own
        // path. Listening there now reopens the race, but it is all such a
        // portal allows.
        warn!("Portal {method}: request path {request_path} is not the predicted {expected}");
        signals = subscribe_response(conn, request_path.to_string()).await?;
    }

    let signal = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), signals.next())
        .await
        .context("Timeout waiting for portal Response")?
        .context("Signal stream ended without Response")?;

    let body = signal.body();
    let (code, results): (u32, HashMap<String, OwnedValue>) = body
        .deserialize()
        .context("Failed to deserialize portal Response")?;

    debug!("Portal {method}: response code = {code}");
    Ok((code, results))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The portal names a request after its sender and `handle_token`. A path
    /// predicted wrong is a subscription on the wrong object, which is the race
    /// again: every step would wait out its timeout.
    #[test]
    fn request_path_follows_the_portal_naming_scheme() {
        assert_eq!(
            request_path(":1.676039", "superstt_r0"),
            "/org/freedesktop/portal/desktop/request/1_676039/superstt_r0"
        );
    }

    #[test]
    fn handle_tokens_are_not_reused() {
        let first = next_handle_token();
        let second = next_handle_token();
        assert_ne!(first, second);
        // The portal accepts only [A-Za-z0-9_] in a token.
        assert!(
            first.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "{first}"
        );
    }
}
