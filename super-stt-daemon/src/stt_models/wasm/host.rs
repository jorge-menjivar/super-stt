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

/// Enforces the backend's `allowed_hosts` on every outbound request. A
/// component's only egress is `wasi:http/outgoing-handler`, which the daemon
/// implements here — so a request to a host outside the allowlist never
/// leaves the machine.
pub struct AllowlistHooks {
    pub allowed_hosts: Vec<String>,
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
        let port =
            request
                .uri()
                .port_u16()
                .unwrap_or(if request.uri().scheme_str() == Some("http") {
                    80
                } else {
                    443
                });
        if let Err(msg) = check_host_allowed(&self.allowed_hosts, &host, port, self.allow_loopback)
        {
            return Err(ErrorCode::InternalError(Some(msg)).into());
        }

        Ok(default_send_request(request, config))
    }
}

/// Confines an outbound connection to the backend's `allowed_hosts` and runs
/// the SSRF resolver guard. Shared by the HTTP hook and the `ws` host so both
/// transports enforce identical egress rules.
///
/// `host` is the bare hostname or IP literal; `port` is the resolved port.
/// `allowed` entries may be either a bare host or a `host:port` authority.
///
/// # Errors
/// Returns a human-readable reason when the host is not on the allowlist or a
/// hostname resolves to a loopback/private/link-local/unspecified address.
pub(crate) fn check_host_allowed(
    allowed: &[String],
    host: &str,
    port: u16,
    allow_loopback: bool,
) -> Result<(), String> {
    let authority = format!("{host}:{port}");
    let on_allowlist = allowed
        .iter()
        .any(|a| a.as_str() == host || a.as_str() == authority);
    if !on_allowlist {
        return Err(format!("outbound host not allowed: {host}"));
    }

    guard_egress_host(host, port, allow_loopback)
}

/// Reject an outbound target that points at an address a sandboxed backend must
/// never reach. An IP literal is checked directly against the disallow-list —
/// the `allowed_hosts` list comes from the backend's own (unreviewed) manifest,
/// so a backend author is not a trusted operator and cannot self-authorize the
/// metadata endpoint via `allowed_hosts = ["169.254.169.254"]`. A hostname is
/// resolved and every resulting address checked.
fn guard_egress_host(host: &str, port: u16, allow_loopback: bool) -> Result<(), String> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        // Loopback is opt-in (tests / local mock upstream); everything else on
        // the disallow-list — including the metadata endpoint — stays blocked.
        if allow_loopback && ip.is_loopback() {
            return Ok(());
        }
        if is_disallowed_ip(&ip) {
            return Err(format!("host {host} is a disallowed address {ip}"));
        }
        return Ok(());
    }
    check_resolved_addrs(host, port, allow_loopback)
}

/// Resolves `host:port` and rejects if any address is disallowed.
fn check_resolved_addrs(host: &str, port: u16, allow_loopback: bool) -> Result<(), String> {
    match (host, port).to_socket_addrs() {
        Ok(addrs) => {
            for addr in addrs {
                let ip = addr.ip();
                if allow_loopback && ip.is_loopback() {
                    continue;
                }
                if is_disallowed_ip(&ip) {
                    return Err(format!("host {host} resolves to a disallowed address {ip}"));
                }
            }
            Ok(())
        }
        Err(_) => Err(format!("cannot resolve host {host}")),
    }
}

/// Addresses a sandboxed backend must never reach.
pub(crate) fn is_disallowed_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_disallowed_v4(*v4),
        IpAddr::V6(v6) => {
            // An IPv4-mapped address (`::ffff:a.b.c.d`) reaches the same host
            // as the bare v4 — e.g. `::ffff:169.254.169.254` is the metadata
            // endpoint — so re-check it through the v4 rules.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_disallowed_v4(mapped);
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
        }
    }
}

fn is_disallowed_v4(v4: Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
}

#[cfg(test)]
mod tests {
    use super::{check_host_allowed, is_disallowed_ip};
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn rejects_v6_internal_ranges_and_mapped_metadata() {
        assert!(is_disallowed_ip(&ip("fc00::1")), "unique-local");
        assert!(is_disallowed_ip(&ip("fd12:3456::1")), "unique-local");
        assert!(is_disallowed_ip(&ip("fe80::1")), "link-local");
        assert!(is_disallowed_ip(&ip("::1")), "loopback");
        assert!(
            is_disallowed_ip(&ip("::ffff:169.254.169.254")),
            "mapped metadata"
        );
        assert!(is_disallowed_ip(&ip("::ffff:127.0.0.1")), "mapped loopback");
    }

    #[test]
    fn allows_public_addresses() {
        assert!(!is_disallowed_ip(&ip("93.184.216.34")));
        assert!(!is_disallowed_ip(&ip("2606:2800:220:1:248:1893:25c8:1946")));
    }

    #[test]
    fn ip_literal_metadata_on_allowlist_is_still_rejected() {
        // A backend cannot self-authorize the metadata endpoint by listing it.
        let allow = vec!["169.254.169.254".to_string()];
        assert!(check_host_allowed(&allow, "169.254.169.254", 80, false).is_err());
    }

    #[test]
    fn public_ip_literal_on_allowlist_is_permitted() {
        let allow = vec!["93.184.216.34".to_string()];
        assert!(check_host_allowed(&allow, "93.184.216.34", 443, false).is_ok());
    }

    #[test]
    fn host_not_on_allowlist_is_rejected() {
        let allow = vec!["api.example.com".to_string()];
        assert!(check_host_allowed(&allow, "169.254.169.254", 80, false).is_err());
    }

    #[test]
    fn host_port_authority_matches_when_port_is_scheme_default() {
        // Divergence regression (Tier 1 #10): a `host:port` allowlist entry must
        // match a request whose port is the scheme default and therefore absent
        // from the written authority. Both the HTTP hook (`send_request`) and the
        // `ws` host now route through `check_host_allowed`, so `["h:443"]` behaves
        // identically for `https://h/` and `wss://h/`. Loopback avoids real DNS.
        let allow = vec!["127.0.0.1:443".to_string()];
        assert!(check_host_allowed(&allow, "127.0.0.1", 443, true).is_ok());
    }

    #[test]
    fn loopback_blocked_by_default_opt_in_permits_only_loopback() {
        let allow = vec!["127.0.0.1:8088".to_string()];
        // Default: loopback egress is refused even when allowlisted (SSRF).
        assert!(check_host_allowed(&allow, "127.0.0.1", 8088, false).is_err());
        // Opt-in (tests / local mock upstream): loopback is permitted.
        assert!(check_host_allowed(&allow, "127.0.0.1", 8088, true).is_ok());
        // The opt-in relaxes loopback ONLY — metadata stays blocked.
        let meta = vec!["169.254.169.254".to_string()];
        assert!(check_host_allowed(&meta, "169.254.169.254", 80, true).is_err());
    }
}
