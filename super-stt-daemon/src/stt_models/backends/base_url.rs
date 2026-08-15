// SPDX-License-Identifier: GPL-3.0-only
//! The `base_url` option — the convention for a backend's configurable
//! endpoint, and the one option whose value widens the sandbox.
//!
//! A configured value authorizes egress the SSRF guard would otherwise refuse,
//! so the daemon reads it from the user's config only; a `backend.toml`
//! declaring a `default` for it is rejected at parse. Deriving the endpoint
//! lives here rather than at either call site so the egress list the transport
//! enforces and the host the catalog discloses can never disagree about what a
//! given value means. See `docs/protocol/backend/config.md`.

/// Name of the option that carries a backend's configurable endpoint, from the
/// crate the daemon, the indexer, and the catalog synthesis all share.
pub(crate) const OPTION_NAME: &str = super_stt_registry_types::manifest::BASE_URL_OPTION;

/// Extract the host and port a base URL points at. Parsed with the same [`Uri`]
/// the transports match against, so what this produces and the authority
/// [`check_host_allowed`](crate::stt_models::wasm::host::check_host_allowed)
/// sees agree on userinfo, case, and the bracketed IPv6 form.
///
/// The port is explicit or the scheme's default, never absent: it is what
/// distinguishes the endpoint the user chose — the one that may be local — from
/// the rest of the host. A value with no scheme is an authority, read as
/// `https`. Returns `None` when no host can be read, which authorizes nothing.
///
/// The port a scheme implies when a URI carries none.
///
/// Both transports and this module's derivation must answer that question the
/// same way: the entry the daemon authorizes is compared byte-for-byte against
/// the authority the transport builds, so a scheme the two rank differently
/// would make them disagree about the very endpoint the user named.
#[cfg(feature = "wasm-backends")]
pub(crate) fn default_port(scheme: Option<&str>) -> u16 {
    match scheme {
        Some("http" | "ws") => 80,
        _ => 443,
    }
}

/// Egress derivation is meaningful only for the wasm transport, which is the
/// one that grants a component network at all; discovery uses [`OPTION_NAME`]
/// regardless of build.
///
/// [`Uri`]: hyper::Uri
#[cfg(feature = "wasm-backends")]
pub(crate) fn authority(value: &str) -> Option<(String, u16)> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Give a scheme-less value one, so `Uri` reads it as an authority rather
    // than as `scheme:path`.
    let uri: hyper::Uri = if trimmed.contains("://") {
        trimmed.parse().ok()?
    } else {
        format!("https://{trimmed}").parse().ok()?
    };
    let host = uri.host()?;
    if host.is_empty() {
        return None;
    }
    let port = uri
        .port_u16()
        .unwrap_or_else(|| default_port(uri.scheme_str()));
    Some((host.to_string(), port))
}

/// The egress entries a configured value contributes: the endpoint the user
/// named, which has the SSRF guard relaxed, followed by its bare host, which
/// does not — that entry keeps a public gateway reachable on its other ports
/// without opening any further local one.
#[cfg(feature = "wasm-backends")]
pub(crate) fn egress_entries(value: &str) -> Vec<String> {
    authority(value).map_or_else(Vec::new, |(host, port)| {
        vec![format!("{host}:{port}"), host]
    })
}

#[cfg(all(test, feature = "wasm-backends"))]
mod tests {
    use super::{authority, default_port, egress_entries};

    #[test]
    fn derives_the_authority_port() {
        // The relaxation covers one endpoint, so a value with no explicit port
        // is pinned to its scheme's default rather than to every port on the
        // host.
        assert_eq!(
            authority("https://api.openai.com"),
            Some(("api.openai.com".to_string(), 443))
        );
        assert_eq!(
            authority("https://api.openai.com/"),
            Some(("api.openai.com".to_string(), 443))
        );
        assert_eq!(
            authority("http://gw.example.com"),
            Some(("gw.example.com".to_string(), 80))
        );
        assert_eq!(
            authority("http://gw.example.com:8080"),
            Some(("gw.example.com".to_string(), 8080))
        );
        // A realtime endpoint carries the ws schemes.
        assert_eq!(
            authority("wss://gw.example.com"),
            Some(("gw.example.com".to_string(), 443))
        );
        assert_eq!(
            authority("ws://gw.example.com"),
            Some(("gw.example.com".to_string(), 80))
        );
        // A value with no scheme is an authority, read as https.
        assert_eq!(
            authority("gw.example.com"),
            Some(("gw.example.com".to_string(), 443))
        );
        assert_eq!(
            authority("gw.example.com:8080"),
            Some(("gw.example.com".to_string(), 8080))
        );
        // Any path after the authority is dropped — the backends assume origin
        // form.
        assert_eq!(
            authority("https://gw.example.com/v1/audio"),
            Some(("gw.example.com".to_string(), 443))
        );
    }

    /// The first egress entry is the endpoint the user named; it is compared
    /// against the authority the request URI carries, so both sides must agree
    /// on the forms a user can paste.
    fn endpoint(value: &str) -> Option<String> {
        egress_entries(value).first().cloned()
    }

    #[test]
    fn matches_what_the_transport_sees() {
        // The entry is compared against the authority the request URI carries,
        // so both sides must agree on the forms a user can paste: an uppercase
        // scheme, userinfo, surrounding whitespace, a query, and the bracketed
        // IPv6 literal form.
        assert_eq!(
            endpoint("HTTP://gw.example.com:8080"),
            Some("gw.example.com:8080".to_string())
        );
        assert_eq!(
            endpoint("https://user:pass@gw.example.com"),
            Some("gw.example.com:443".to_string())
        );
        assert_eq!(
            endpoint("  https://gw.example.com:8443  "),
            Some("gw.example.com:8443".to_string())
        );
        assert_eq!(
            endpoint("https://gw.example.com/v1?key=value"),
            Some("gw.example.com:443".to_string())
        );
        assert_eq!(
            endpoint("http://[::1]:8080"),
            Some("[::1]:8080".to_string())
        );
        assert_eq!(endpoint("http://[::1]"), Some("[::1]:80".to_string()));
    }

    /// The transports derive the enforced authority's port with this same
    /// function; a scheme ranked differently there would make the authorized
    /// entry and the enforced one disagree.
    #[test]
    fn default_port_follows_the_scheme() {
        assert_eq!(default_port(Some("http")), 80);
        assert_eq!(default_port(Some("ws")), 80);
        assert_eq!(default_port(Some("https")), 443);
        assert_eq!(default_port(Some("wss")), 443);
        assert_eq!(default_port(None), 443);
    }

    #[test]
    fn rejects_unparseable() {
        for value in ["", "   ", "https://", "/", "https:///path", "http://:8080"] {
            assert_eq!(authority(value), None, "{value:?}");
            assert!(egress_entries(value).is_empty(), "{value:?}");
        }
    }

    #[test]
    fn egress_entries_are_the_endpoint_then_the_bare_host() {
        assert_eq!(
            egress_entries("http://192.168.1.50"),
            vec!["192.168.1.50:80".to_string(), "192.168.1.50".to_string()]
        );
    }
}
