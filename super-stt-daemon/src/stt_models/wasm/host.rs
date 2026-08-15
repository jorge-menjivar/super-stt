// SPDX-License-Identifier: GPL-3.0-only
//! Per-`Store` host state for running a WASM backend component, including the
//! outbound-host allowlist that confines a component's network egress.

use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};

use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::WasiHttpCtx;
use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::types::{HostFutureIncomingResponse, OutgoingRequestConfig};
use wasmtime_wasi_http::p2::{
    HttpResult, WasiHttpCtxView, WasiHttpHooks, WasiHttpView, default_send_request,
};

/// Host state handed to each component invocation.
pub struct Host {
    pub table: ResourceTable,
    pub wasi: WasiCtx,
    pub http: WasiHttpCtx,
    pub hooks: AllowlistHooks,
}

impl WasiView for Host {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for Host {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: &mut self.hooks,
        }
    }
}

/// Enforces the backend's egress rules on every outbound request. A component's
/// only egress is `wasi:http/outgoing-handler`, which the daemon implements
/// here — so a request to a destination neither list below permits never leaves
/// the machine.
pub struct AllowlistHooks {
    /// Hosts pinned by the backend's own (unreviewed) `[network].allowed_hosts`
    /// manifest. These are SSRF-guarded: a manifest entry does **not** authorize
    /// loopback/private/link-local/metadata destinations, because the backend
    /// author is not a trusted operator.
    pub allowed_hosts: Vec<String>,
    /// What the *user* authorized through backend options (e.g. a `base_url` set
    /// in the settings UI): the `host:port` the value names, plus its bare host.
    /// The SSRF guard is relaxed for the authority alone — loopback and private
    /// addresses are reachable there, because pointing a backend at a local
    /// gateway is the point of the option, and the component cannot
    /// self-authorize one (options are user-writable only). The relaxation stops
    /// there: link-local (metadata), unspecified, and broadcast addresses stay
    /// refused, and the bare host is judged like a manifest entry, so a gateway's
    /// other ports stay reachable only while they are public.
    pub user_allowed_hosts: Vec<String>,
    /// Permit egress to loopback addresses (`127.0.0.0/8`, `::1`). Off in
    /// production — the SSRF guard blocks loopback so an untrusted backend
    /// can't reach a service bound to localhost. Tests and local development
    /// against a mock upstream opt in via
    /// [`WasmBackend::permit_loopback_egress`](crate::stt_models::wasm::WasmBackend::permit_loopback_egress).
    /// Only loopback is relaxed; link-local/metadata/private ranges stay blocked.
    pub allow_loopback: bool,
}

impl WasiHttpHooks for AllowlistHooks {
    fn send_request(
        &mut self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        // Enforce egress through the shared allowlist+SSRF check so HTTP and the
        // `ws` host apply byte-for-byte identical rules. This hook used to match
        // the authority string exactly as written, which diverged from
        // `check_host_allowed`'s synthesized `host:port` match: an allowlist
        // entry of `api.example.com:443` passed for `wss://` but not `https://`
        // (whose port is the scheme default and so absent from the authority).
        //
        // NOTE: `check_host_allowed` resolves DNS synchronously and is
        // check-then-connect (TOCTOU); production should use async DNS and pin
        // the resolved address through to connect. The bare-host early return
        // rejects an authority-form request with no host outright.
        let Some(host) = request.uri().host().map(str::to_string) else {
            return Err(
                ErrorCode::InternalError(Some("outbound request has no host".to_string())).into(),
            );
        };
        let port = request.uri().port_u16().unwrap_or_else(|| {
            crate::stt_models::backends::base_url::default_port(request.uri().scheme_str())
        });
        if let Err(msg) = check_host_allowed(
            &self.allowed_hosts,
            &self.user_allowed_hosts,
            &host,
            port,
            self.allow_loopback,
        ) {
            return Err(ErrorCode::InternalError(Some(msg)).into());
        }

        Ok(default_send_request(request, config))
    }
}

/// How far an outbound destination may reach once a list permits it.
#[derive(Clone, Copy)]
enum EgressScope {
    /// Public addresses only — the SSRF guard in full. Applies to every manifest
    /// entry, and to a `user_allowed` entry that names no port and so authorizes
    /// no particular endpoint.
    PublicOnly,
    /// The user named this exact `host:port`: loopback and private addresses are
    /// reachable, the never-routable ones still are not.
    UserAuthorized,
}

/// Confines an outbound connection to the backend's `allowed_hosts` — plus any
/// `user_allowed` endpoints — and runs the SSRF resolver guard. Shared by the
/// HTTP hook and the `ws` host so both transports enforce identical egress
/// rules.
///
/// `host` is the hostname or IP literal as a URI authority carries it (IPv6
/// bracketed); `port` is the resolved port. Entries in either list may be a bare
/// host or a `host:port` authority, matched case-insensitively. A
/// `user_allowed` entry matching the full authority relaxes the guard for that
/// endpoint alone (see [`AllowlistHooks::user_allowed_hosts`]); every other
/// match is guarded in full.
///
/// # Errors
/// Returns a human-readable reason when the host is on neither list, or when the
/// destination resolves to an address its scope does not permit.
pub(crate) fn check_host_allowed(
    allowed: &[String],
    user_allowed: &[String],
    host: &str,
    port: u16,
    allow_loopback: bool,
) -> Result<(), String> {
    let authority = format!("{host}:{port}");
    // The endpoint the *user* named, and the only case that may reach a local or
    // private address. Unlike manifest `allowed_hosts` — written by the untrusted
    // backend author, who therefore cannot self-authorize a localhost target —
    // this list is user intent: the daemon reads it from options set through the
    // settings-scoped API, never from the component. Checked before the manifest
    // so a backend that also lists the host cannot cost the user the endpoint
    // they authorized.
    if user_allowed
        .iter()
        .any(|a| a.eq_ignore_ascii_case(&authority))
    {
        return guard_egress_host(host, port, allow_loopback, EgressScope::UserAuthorized);
    }
    // Everything else is guarded in full. A manifest entry may name a bare host
    // or an authority; a `user_allowed` entry is only reached here in its bare
    // form, since its authority form returned above — so the two lists are
    // matched by the rules they actually have, not by one predicate that would
    // read as if a user authority could land in this scope too.
    let on_manifest = allowed
        .iter()
        .any(|a| a.eq_ignore_ascii_case(host) || a.eq_ignore_ascii_case(&authority));
    let on_user_bare = user_allowed.iter().any(|a| a.eq_ignore_ascii_case(host));
    if on_manifest || on_user_bare {
        return guard_egress_host(host, port, allow_loopback, EgressScope::PublicOnly);
    }
    Err(format!("outbound host not allowed: {host}"))
}

/// Reject an outbound target that points at an address the destination's
/// [`EgressScope`] does not permit. An IP literal is checked directly — the
/// `allowed_hosts` list comes from the backend's own (unreviewed) manifest, so a
/// backend author is not a trusted operator and cannot self-authorize the
/// metadata endpoint via `allowed_hosts = ["169.254.169.254"]`. A hostname is
/// resolved and every resulting address checked.
fn guard_egress_host(
    host: &str,
    port: u16,
    allow_loopback: bool,
    scope: EgressScope,
) -> Result<(), String> {
    if let Some(ip) = ip_literal(host) {
        if is_egress_permitted(&ip, allow_loopback, scope) {
            return Ok(());
        }
        return Err(format!("host {host} is a disallowed address {ip}"));
    }
    check_resolved_addrs(host, port, allow_loopback, scope)
}

/// Parse a host that is an IP literal, accepting the bracketed IPv6 form
/// (`[::1]`) a URI authority carries. `None` for a hostname, which is resolved
/// instead.
fn ip_literal(host: &str) -> Option<IpAddr> {
    host.strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host)
        .parse()
        .ok()
}

/// Resolves `host:port` and rejects if any address is out of scope.
fn check_resolved_addrs(
    host: &str,
    port: u16,
    allow_loopback: bool,
    scope: EgressScope,
) -> Result<(), String> {
    match (host, port).to_socket_addrs() {
        Ok(addrs) => {
            for addr in addrs {
                let ip = addr.ip();
                if !is_egress_permitted(&ip, allow_loopback, scope) {
                    return Err(format!("host {host} resolves to a disallowed address {ip}"));
                }
            }
            Ok(())
        }
        Err(_) => Err(format!("cannot resolve host {host}")),
    }
}

/// Whether a resolved address may be reached under `scope`.
fn is_egress_permitted(ip: &IpAddr, allow_loopback: bool, scope: EgressScope) -> bool {
    // Nothing authorizes these — a gateway a user means to reach never lives on
    // the link-local range, and the metadata endpoint is the address an escaped
    // backend wants most.
    if is_never_routable_ip(ip) {
        return false;
    }
    match scope {
        // Loopback and private are exactly what a local gateway needs.
        EgressScope::UserAuthorized => true,
        // Loopback stays opt-in (tests / local mock upstream); private ranges
        // stay blocked outright.
        EgressScope::PublicOnly => (allow_loopback && ip.is_loopback()) || !is_local_ip(ip),
    }
}

/// Addresses no allowlist authorizes, whoever wrote it: the link-local range
/// (the cloud metadata endpoint `169.254.169.254` among it), the unspecified
/// address, and broadcast.
fn is_never_routable_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_never_routable_v4(*v4),
        IpAddr::V6(v6) => {
            // An IPv4-mapped address (`::ffff:a.b.c.d`) reaches the same host
            // as the bare v4 — e.g. `::ffff:169.254.169.254` is the metadata
            // endpoint — so re-check it through the v4 rules.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_never_routable_v4(mapped);
            }
            v6.is_unspecified() || v6.is_unicast_link_local()
        }
    }
}

fn is_never_routable_v4(v4: Ipv4Addr) -> bool {
    v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast()
}

/// Loopback and private addresses: off-limits to a manifest-declared
/// destination, reachable for the endpoint the user named.
fn is_local_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_local_v4(*v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_local_v4(mapped);
            }
            v6.is_loopback() || v6.is_unique_local()
        }
    }
}

fn is_local_v4(v4: Ipv4Addr) -> bool {
    v4.is_loopback() || v4.is_private()
}

#[cfg(test)]
mod tests {
    use super::{check_host_allowed, is_local_ip, is_never_routable_ip};
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn classifies_v6_internal_ranges_and_mapped_metadata() {
        // Never routable: refused under either scope.
        assert!(is_never_routable_ip(&ip("fe80::1")), "link-local");
        assert!(is_never_routable_ip(&ip("::")), "unspecified");
        assert!(
            is_never_routable_ip(&ip("::ffff:169.254.169.254")),
            "mapped metadata"
        );
        // Local: refused for a manifest destination, reached only by the endpoint
        // the user named.
        assert!(is_local_ip(&ip("fc00::1")), "unique-local");
        assert!(is_local_ip(&ip("fd12:3456::1")), "unique-local");
        assert!(is_local_ip(&ip("::1")), "loopback");
        assert!(is_local_ip(&ip("::ffff:127.0.0.1")), "mapped loopback");
    }

    #[test]
    fn allows_public_addresses() {
        for addr in ["93.184.216.34", "2606:2800:220:1:248:1893:25c8:1946"] {
            assert!(!is_never_routable_ip(&ip(addr)), "{addr}");
            assert!(!is_local_ip(&ip(addr)), "{addr}");
        }
    }

    #[test]
    fn ip_literal_metadata_on_allowlist_is_still_rejected() {
        // A backend cannot self-authorize the metadata endpoint by listing it.
        let allow = vec!["169.254.169.254".to_string()];
        assert!(check_host_allowed(&allow, &[], "169.254.169.254", 80, false).is_err());
    }

    #[test]
    fn public_ip_literal_on_allowlist_is_permitted() {
        let allow = vec!["93.184.216.34".to_string()];
        assert!(check_host_allowed(&allow, &[], "93.184.216.34", 443, false).is_ok());
    }

    #[test]
    fn host_not_on_allowlist_is_rejected() {
        let allow = vec!["api.example.com".to_string()];
        assert!(check_host_allowed(&allow, &[], "169.254.169.254", 80, false).is_err());
    }

    #[test]
    fn host_port_authority_matches_when_port_is_scheme_default() {
        // Divergence regression (Tier 1 #10): a `host:port` allowlist entry must
        // match a request whose port is the scheme default and therefore absent
        // from the written authority. Both the HTTP hook (`send_request`) and the
        // `ws` host now route through `check_host_allowed`, so `["h:443"]` behaves
        // identically for `https://h/` and `wss://h/`. Loopback avoids real DNS.
        let allow = vec!["127.0.0.1:443".to_string()];
        assert!(check_host_allowed(&allow, &[], "127.0.0.1", 443, true).is_ok());
    }

    #[test]
    fn loopback_blocked_by_default_opt_in_permits_only_loopback() {
        let allow = vec!["127.0.0.1:8088".to_string()];
        // Default: loopback egress is refused even when allowlisted (SSRF).
        assert!(check_host_allowed(&allow, &[], "127.0.0.1", 8088, false).is_err());
        // Opt-in (tests / local mock upstream): loopback is permitted.
        assert!(check_host_allowed(&allow, &[], "127.0.0.1", 8088, true).is_ok());
        // The opt-in relaxes loopback ONLY — metadata stays blocked.
        let meta = vec!["169.254.169.254".to_string()];
        assert!(check_host_allowed(&meta, &[], "169.254.169.254", 80, true).is_err());
    }

    #[test]
    fn user_allowed_authority_reaches_loopback_without_the_opt_in() {
        // The endpoint the user named in a backend option (e.g. a base_url set in
        // the settings UI) reaches a local gateway with no loopback opt-in.
        let user = vec!["127.0.0.1:8088".to_string()];
        assert!(check_host_allowed(&[], &user, "127.0.0.1", 8088, false).is_ok());
    }

    #[test]
    fn user_allowed_authority_does_not_widen_to_other_ports() {
        // The authorization covers one `host:port`. Every other port on that host
        // is only as reachable as the manifest allowlist makes it — here, not at
        // all, and local addresses stay refused even when it does list them.
        let user = vec!["127.0.0.1:8088".to_string()];
        assert!(check_host_allowed(&[], &user, "127.0.0.1", 22, false).is_err());
        let allow = vec!["127.0.0.1".to_string()];
        assert!(check_host_allowed(&allow, &user, "127.0.0.1", 22, false).is_err());
    }

    #[test]
    fn user_allowed_bare_host_keeps_public_ports_not_local_ones() {
        // Both entries a `base_url` produces: the endpoint, then its bare host.
        // The bare host names no particular endpoint, so it buys no local reach —
        // a gateway keeps its other ports only while they are public.
        let public = vec!["93.184.216.34:443".to_string(), "93.184.216.34".to_string()];
        assert!(check_host_allowed(&[], &public, "93.184.216.34", 8443, false).is_ok());
        let local = vec!["127.0.0.1:8088".to_string(), "127.0.0.1".to_string()];
        assert!(check_host_allowed(&[], &local, "127.0.0.1", 8088, false).is_ok());
        assert!(check_host_allowed(&[], &local, "127.0.0.1", 22, false).is_err());
    }

    #[test]
    fn user_allowed_metadata_is_refused_even_when_user_set() {
        // The relaxation lifts the loopback and private blocks only. No gateway a
        // user means to reach lives on the link-local range, so a value pointing
        // at the metadata endpoint is refused however it got there.
        let user = vec!["169.254.169.254:80".to_string()];
        assert!(check_host_allowed(&[], &user, "169.254.169.254", 80, false).is_err());
        let mapped = vec!["[::ffff:169.254.169.254]:80".to_string()];
        assert!(check_host_allowed(&[], &mapped, "[::ffff:169.254.169.254]", 80, false).is_err());
    }

    #[test]
    fn user_allowed_unspecified_and_broadcast_are_refused() {
        let unspecified = vec!["0.0.0.0:8080".to_string()];
        assert!(check_host_allowed(&[], &unspecified, "0.0.0.0", 8080, false).is_err());
        let broadcast = vec!["255.255.255.255:8080".to_string()];
        assert!(check_host_allowed(&[], &broadcast, "255.255.255.255", 8080, false).is_err());
    }

    #[test]
    fn user_allowed_private_authority_is_permitted() {
        let user = vec!["10.0.0.5:8443".to_string()];
        assert!(check_host_allowed(&[], &user, "10.0.0.5", 8443, false).is_ok());
        // IPv6 arrives bracketed from a URI authority, on both sides of the match.
        let user6 = vec!["[fd12:3456::1]:8443".to_string()];
        assert!(check_host_allowed(&[], &user6, "[fd12:3456::1]", 8443, false).is_ok());
    }

    #[test]
    fn user_allowed_authority_matches_case_insensitively() {
        // Hostnames are case-insensitive; exact-authority matching must not turn
        // a difference in case into a refusal. A hostname entry reaches the
        // resolver, so what shows the match is that the destination is judged on
        // the addresses it resolves to rather than refused as unlisted.
        let user = vec!["GW.Example.com:443".to_string()];
        let res = check_host_allowed(&[], &user, "gw.example.com", 443, false);
        assert!(
            !matches!(&res, Err(msg) if msg.contains("not allowed")),
            "case-mismatched entry was treated as unlisted: {res:?}"
        );
    }

    #[test]
    fn user_authority_survives_the_same_host_on_the_manifest_list() {
        // A backend that lists `localhost` must not cost the user the endpoint
        // they authorized: the user's exact authority is honored first.
        let allow = vec!["127.0.0.1".to_string()];
        let user = vec!["127.0.0.1:8080".to_string()];
        assert!(check_host_allowed(&allow, &user, "127.0.0.1", 8080, false).is_ok());
    }

    #[test]
    fn user_allowed_does_not_loosen_other_hosts() {
        // Only what the user named is relaxed; a different disallowed target is
        // still refused even while a user endpoint is present.
        let user = vec!["10.0.0.5:8443".to_string()];
        assert!(check_host_allowed(&[], &user, "169.254.169.254", 80, false).is_err());
        assert!(check_host_allowed(&[], &user, "10.0.0.6", 8443, false).is_err());
    }

    #[test]
    fn manifest_host_still_ssrf_guarded_alongside_user_host() {
        // The manifest allowlist stays fully SSRF-guarded even when a user
        // endpoint is present: loopback/private via the manifest is refused,
        // while the user's own endpoint passes.
        let allow = vec!["10.0.0.9".to_string()];
        let user = vec!["10.0.0.5:8443".to_string()];
        assert!(check_host_allowed(&allow, &user, "10.0.0.9", 443, false).is_err());
        assert!(check_host_allowed(&allow, &user, "10.0.0.5", 8443, false).is_ok());
    }
}
