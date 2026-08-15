// SPDX-License-Identifier: GPL-3.0-only
//! The `base_url` option — the convention for a backend's configurable
//! endpoint, and the one option whose value widens the sandbox.
//!
//! A configured value authorizes egress the SSRF guard would otherwise refuse,
//! so the daemon reads it from the user's config only: a `default` a
//! `backend.toml` declares for it is refused at publication, and dropped with a
//! warning if the backend was installed some other way. Deriving the endpoint
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
/// the rest of the host. A value with no scheme is an authority, read under the
/// scheme [`inferred_scheme`] gives its host. Returns `None` when no host can be
/// read, which authorizes nothing.
///
/// The port a scheme implies when a URI carries none.
///
/// Both transports and this module's derivation must answer that question the
/// same way: the entry the daemon authorizes is compared byte-for-byte against
/// the authority the transport builds, so a scheme the two rank differently
/// would make them disagree about the very endpoint the user named.
#[cfg(feature = "wasm-backends")]
pub(crate) fn default_port(scheme: Option<&str>) -> u16 {
    // Matched case-insensitively: `Uri` canonicalizes `http` and `https` but
    // leaves any other scheme's case as written, so `WS://` would otherwise be
    // ranked with the 443 schemes.
    match scheme {
        Some(s) if s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("ws") => 80,
        _ => 443,
    }
}

/// Rewrite a configured value into the canonical form the backend is handed:
/// `scheme://host[:port][/path]`.
///
/// The daemon acts on this value twice — it authorizes an endpoint and it tells
/// the component which one to dial — and a backend that re-derived the endpoint
/// from raw text would be writing a second URL parser whose disagreements with
/// this one surface as a refused request. Normalizing once, here, is what lets
/// a backend split the value at the first `/` and stop.
///
/// The scheme is lowercased and supplied when absent, userinfo is stripped, a
/// trailing slash is removed, and any query or fragment is dropped. Two things
/// are deliberately left alone: the port is emitted only when the value carried
/// one, since a synthesized `:443` would travel to the upstream in the `Host`
/// header for no gain — [`authority`] already pins the port the egress entry
/// names — and the path is preserved verbatim, because it does not affect
/// egress and only the backend knows which path its API serves.
///
/// Returns `None` for a value no host can be read from, which is a
/// misconfiguration the caller reports rather than silently discards.
#[cfg(feature = "wasm-backends")]
pub(crate) fn normalize(value: &str) -> Option<String> {
    let uri = parse(value)?;
    let host = uri.host().filter(|h| !h.is_empty())?;
    let scheme = uri.scheme_str().unwrap_or("https").to_ascii_lowercase();
    let port = uri.port_u16().map(|p| format!(":{p}")).unwrap_or_default();
    // `path()` is `/` when the value carried none, and already excludes the
    // query and fragment.
    let path = uri.path().trim_end_matches('/');
    Some(format!("{scheme}://{host}{port}{path}"))
}

/// The scheme a value carrying none is read as.
///
/// A local endpoint is nearly always a plaintext one — the gateways people run
/// on a private address speak `http`, and reading such a value as `https` fails
/// every time. A public endpoint is the opposite. So the daemon decides from
/// the host, using the same classifier the egress guard applies, which is what
/// keeps "local" from meaning two things in one codebase.
///
/// This decides only a value that names no scheme, and decides it before
/// anything connects; a value that says `https` stays `https` however it fails.
/// Nothing here retries a failed TLS connection over plaintext — that would let
/// anyone able to break the handshake move the user's audio and credentials
/// into the clear, while still looking like success.
///
/// A name other than `localhost` cannot be classified without resolving it, and
/// is read as `https`. The two mistakes are not equally bad: `https` against a
/// plaintext endpoint costs a failed connection, loud and recoverable, while
/// `http` against a TLS one discloses whatever the request carries. Where the
/// choice is uncertain, take the loud failure.
#[cfg(feature = "wasm-backends")]
fn inferred_scheme(host: &str) -> &'static str {
    let local = host.eq_ignore_ascii_case("localhost")
        || crate::stt_models::wasm::host::ip_literal(host)
            .is_some_and(|ip| crate::stt_models::wasm::host::is_local_ip(&ip));
    if local { "http" } else { "https" }
}

/// Read a configured value as a [`Uri`], giving a scheme-less one the scheme
/// its host implies so it parses as an authority rather than as `scheme:path`.
///
/// The inference lives here rather than in [`normalize`] so that every reader
/// of a scheme-less value agrees on it: [`authority`] derives the port from the
/// scheme, so a value inferred `http` in one place and `https` in another would
/// authorize `:443` while the component dialed `:80`.
///
/// [`Uri`]: hyper::Uri
#[cfg(feature = "wasm-backends")]
fn parse(value: &str) -> Option<hyper::Uri> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains("://") {
        return trimmed.parse().ok();
    }
    // Read the host under a placeholder scheme, then re-read under the one that
    // host implies. `Uri` needs *a* scheme to treat this as an authority at all.
    let probe: hyper::Uri = format!("https://{trimmed}").parse().ok()?;
    let scheme = inferred_scheme(probe.host()?);
    format!("{scheme}://{trimmed}").parse().ok()
}

/// Egress derivation is meaningful only for the wasm transport, which is the
/// one that grants a component network at all; discovery uses [`OPTION_NAME`]
/// regardless of build.
///
/// [`Uri`]: hyper::Uri
#[cfg(feature = "wasm-backends")]
pub(crate) fn authority(value: &str) -> Option<(String, u16)> {
    let uri = parse(value)?;
    let host = uri.host().filter(|h| !h.is_empty())?;
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
    use super::{authority, default_port, egress_entries, normalize};

    /// The form a backend is handed. Everything it may stop parsing for is
    /// asserted here, since a backend reading the value has only this
    /// guarantee to lean on.
    #[test]
    fn canonicalizes_what_the_backend_is_handed() {
        let cases = [
            // Already canonical — unchanged.
            ("https://api.openai.com", "https://api.openai.com"),
            ("https://api.openai.com/v1", "https://api.openai.com/v1"),
            ("http://localhost:11434/v1", "http://localhost:11434/v1"),
            // Scheme lowercased, and supplied when absent.
            ("HTTPS://api.openai.com", "https://api.openai.com"),
            ("WSS://gw.example.com/rt", "wss://gw.example.com/rt"),
            ("gw.example.com:8080", "https://gw.example.com:8080"),
            // Userinfo never reaches the component.
            (
                "https://user:pass@gw.example.com/v1",
                "https://gw.example.com/v1",
            ),
            // One trailing slash, so a backend can append a suffix blindly.
            ("https://api.openai.com/", "https://api.openai.com"),
            ("https://gw.example.com/v1/", "https://gw.example.com/v1"),
            // A query or fragment cannot compose with a path suffix.
            (
                "https://gw.example.com/v1?key=v#frag",
                "https://gw.example.com/v1",
            ),
            // Surrounding whitespace from a paste.
            ("  https://gw.example.com/v1  ", "https://gw.example.com/v1"),
            // The bracketed IPv6 form survives as the component must send it.
            ("http://[::1]:8080/v1", "http://[::1]:8080/v1"),
            // A path is preserved verbatim: only the backend knows its API's shape.
            (
                "https://api.groq.com/openai/v1",
                "https://api.groq.com/openai/v1",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(normalize(input).as_deref(), Some(want), "{input:?}");
        }
    }

    /// A value naming no scheme is read as `http` when its host is one the
    /// daemon can see is local, and `https` otherwise. This is the case that
    /// sent a user chasing a `write_failed`: a private gateway read as `https`
    /// opens TLS against a plaintext listener, and the connection dies wherever
    /// the body write happens to notice.
    #[test]
    fn a_scheme_less_value_is_read_by_its_host() {
        for (input, want) in [
            // Local: plaintext is what is actually listening there.
            ("192.168.0.179:8080/v1", "http://192.168.0.179:8080/v1"),
            ("10.0.0.5:8080", "http://10.0.0.5:8080"),
            ("172.16.3.9", "http://172.16.3.9"),
            ("127.0.0.1:11434/v1", "http://127.0.0.1:11434/v1"),
            ("localhost:4000/v1", "http://localhost:4000/v1"),
            ("LOCALHOST:4000", "http://LOCALHOST:4000"),
            ("[::1]:8080", "http://[::1]:8080"),
            ("[fd00::1]:8080", "http://[fd00::1]:8080"),
            // Public, and any name the daemon cannot classify without
            // resolving it: https, because that mistake is the recoverable one.
            ("api.openai.com/v1", "https://api.openai.com/v1"),
            ("gw.internal:8443", "https://gw.internal:8443"),
            ("140.82.121.4", "https://140.82.121.4"),
        ] {
            assert_eq!(normalize(input).as_deref(), Some(want), "{input:?}");
        }
    }

    /// Inference must not split the two readers. `authority` derives the port
    /// from the scheme, so a value read as `http` here and `https` there would
    /// authorize `:443` while the component dialed `:80`, and every request
    /// would be refused.
    #[test]
    fn the_inferred_scheme_decides_the_authorized_port() {
        assert_eq!(
            authority("192.168.0.179"),
            Some(("192.168.0.179".to_string(), 80))
        );
        assert_eq!(
            authority("api.openai.com"),
            Some(("api.openai.com".to_string(), 443))
        );
        // An explicit scheme is never second-guessed, local host or not.
        assert_eq!(
            authority("https://192.168.0.179"),
            Some(("192.168.0.179".to_string(), 443))
        );
        assert_eq!(
            normalize("https://192.168.0.179").as_deref(),
            Some("https://192.168.0.179")
        );
    }

    /// A port the user did not write is not invented. `authority` pins the
    /// egress entry's port regardless, and a synthesized one would reach the
    /// upstream in the `Host` header.
    #[test]
    fn keeps_the_port_the_user_wrote_and_no_other() {
        assert_eq!(
            normalize("https://gw.example.com").as_deref(),
            Some("https://gw.example.com")
        );
        assert_eq!(
            normalize("https://gw.example.com:443").as_deref(),
            Some("https://gw.example.com:443")
        );
        assert_eq!(
            authority("https://gw.example.com"),
            authority("https://gw.example.com:443")
        );
    }

    /// Re-normalizing is a no-op. The value is stored raw and canonicalized on
    /// every load, so a value that changed on each pass would make the entry
    /// authorized and the endpoint dialed drift apart across reloads.
    #[test]
    fn normalizing_is_idempotent() {
        for value in [
            "HTTPS://user:pass@gw.example.com:8443/v1/?k=v",
            "gw.example.com",
            "http://[::1]:8080/",
            "https://api.groq.com/openai/v1",
        ] {
            let once = normalize(value).expect("parses");
            assert_eq!(
                normalize(&once).as_deref(),
                Some(once.as_str()),
                "{value:?}"
            );
        }
    }

    /// Normalization must not move the endpoint: the entry the guard enforces
    /// is derived from the same value the component is told to dial, so the two
    /// have to agree before and after.
    #[test]
    fn normalizing_preserves_the_authorized_endpoint() {
        for value in [
            "HTTPS://user:pass@gw.example.com:8443/v1?k=v",
            "gw.example.com",
            "WS://gw.example.com/rt",
            "http://[::1]:8080/",
            "http://192.168.1.50/v1",
        ] {
            let canonical = normalize(value).expect("parses");
            assert_eq!(authority(value), authority(&canonical), "{value:?}");
            assert_eq!(
                egress_entries(value),
                egress_entries(&canonical),
                "{value:?}"
            );
        }
    }

    /// The caller reports these rather than dropping them: falling back to the
    /// backend's built-in endpoint would send the user's audio and credentials
    /// to the vendor they configured their way out of.
    #[test]
    fn normalize_rejects_what_yields_no_host() {
        for value in ["", "   ", "https://", "/", "https:///path", "http://:8080"] {
            assert_eq!(normalize(value), None, "{value:?}");
        }
    }

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
        // A value with no scheme is an authority, read under the scheme its
        // host implies — a name resolves to https.
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
