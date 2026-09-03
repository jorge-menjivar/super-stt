// SPDX-License-Identifier: GPL-3.0-only
//! Host implementation of the outgoing `super-stt:realtime/ws` interface.
//!
//! A realtime backend imports `super-stt:realtime/ws` to open a WebSocket to
//! its upstream cloud API. The daemon implements that import here, against the
//! same egress allowlist + SSRF guard the HTTP host uses (see
//! [`check_host_allowed`]) so a realtime backend's only network reach is
//! its declared `allowed_hosts` plus the endpoint the user authorized through
//! its `base_url` option.
//!
//! Only the *outgoing* `ws` interface lives here. The *incoming* `ws-server`
//! interface (which the backend exports and the daemon invokes per consumer
//! session) is wired separately.

use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use wasmtime::Result;
use wasmtime::component::{Linker, Resource};

use crate::stt_models::wasm::host::{Host, check_host_allowed};

// Generate the host-side bindings for the realtime world. Only the `ws`
// interface (a host import) is implemented in this module; the wasi:* deps and
// the `ws-server` export are reused / handled elsewhere.
//
// The wasi:* interfaces are aliased to wasmtime's existing generated bindings
// so the resource and type definitions unify with the linker set up by
// `add_to_linker_async` — generating fresh copies would conflict.
wasmtime::component::bindgen!({
    path: "../docs/protocol/wit",
    world: "realtime-backend",
    imports: { default: async | trappable },
    exports: { default: async },
    with: {
        "wasi:io": wasmtime_wasi::p2::bindings::io,
        "wasi:clocks": wasmtime_wasi::p2::bindings::clocks,
        "wasi:http": wasmtime_wasi_http::p2::bindings::http,
        // Back the guest's `ws-stream` resource handle with our host type so
        // table lookups yield a `WsStreamResource` directly.
        "super-stt:realtime/ws.ws-stream": WsStreamResource,
        // Likewise for the host-owned consumer socket the daemon hands to the
        // guest's `ws-server.handle`.
        "super-stt:realtime/ws.consumer-stream": ConsumerStreamResource,
    },
});

// Re-export the generated wire types so later tasks can name them without
// reaching into the generated module path.
pub use self::super_stt::realtime::ws::{CloseFrame, WsError, WsFrame};

/// A live outgoing WebSocket connection owned by the host. Stored in
/// `Host.table`; the handle is returned to the guest. `None` means the stream
/// has been closed (locally or by the remote) — further sends/recvs fail with
/// [`WsError::Closed`].
pub struct WsStreamResource {
    stream: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    /// One-frame lookahead. `wasi:io/poll` asks "is this readable?" without
    /// taking anything, but a `WebSocketStream` only answers by consuming, so
    /// [`Pollable::ready`] reads one frame and parks it here for the `recv`
    /// that follows. Readiness therefore stays non-destructive from the guest's
    /// side: every frame `ready` observes is still delivered by `recv`.
    pending: Option<std::result::Result<WsFrame, WsError>>,
}

impl WsStreamResource {
    fn new(stream: WebSocketStream<MaybeTlsStream<TcpStream>>) -> Self {
        Self {
            stream: Some(stream),
            pending: None,
        }
    }

    /// The next frame the protocol exposes, skipping tungstenite's ping/pong.
    ///
    /// Cancel-safe, which `poll` requires: it races several `ready` futures and
    /// drops the losers. Dropping this future at its only await point cannot
    /// lose a frame — `StreamExt::next` leaves partially-read bytes in
    /// tungstenite's own buffer, and a frame that *has* been decoded is stored
    /// by the caller with no await in between.
    async fn next_frame(&mut self) -> std::result::Result<WsFrame, WsError> {
        let Some(stream) = self.stream.as_mut() else {
            return Err(WsError::Closed);
        };
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(text))) => {
                    return Ok(WsFrame::Text(text.as_str().to_string()));
                }
                Some(Ok(Message::Binary(data))) => return Ok(WsFrame::Binary(data.into())),
                Some(Ok(Message::Close(frame))) => {
                    // Emit the close once, then mark the stream closed so the
                    // next recv returns `WsError::Closed`.
                    self.stream = None;
                    let close = frame.map_or(
                        CloseFrame {
                            code: 1005,
                            reason: String::new(),
                        },
                        |f| CloseFrame {
                            code: f.code.into(),
                            reason: f.reason.as_str().to_string(),
                        },
                    );
                    return Ok(WsFrame::Close(close));
                }
                // Ping/pong are handled by tungstenite's auto-pong; skip them
                // and read the next data frame.
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                Some(Err(e)) => {
                    self.stream = None;
                    return Err(WsError::RecvFailed(format!("recv failed: {e}")));
                }
                None => {
                    self.stream = None;
                    return Err(WsError::Closed);
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::Pollable for WsStreamResource {
    async fn ready(&mut self) {
        if self.pending.is_none() {
            self.pending = Some(self.next_frame().await);
        }
    }
}

/// Capacity of the consumer→guest `incoming` channel. Bounds per-session memory:
/// a client streaming audio faster than the guest drains it applies TCP
/// backpressure rather than growing an unbounded buffer (audit 2 Tier 1 #7).
pub const CONSUMER_INCOMING_CAPACITY: usize = 256;

/// The host-side bridge between the consumer-facing axum WebSocket and the
/// guest's realtime session. The axum endpoint task owns the opposite ends of
/// these channels: it forwards frames it reads off the consumer socket into
/// `incoming`, and writes frames the guest emits on `outgoing` back to the
/// consumer.
pub struct ConsumerStreamTransport {
    /// Frames arriving FROM the consumer (axum) TO the guest. Bounded
    /// ([`CONSUMER_INCOMING_CAPACITY`]) so a fast producer can't exhaust memory.
    pub incoming: tokio::sync::mpsc::Receiver<WsFrame>,
    /// Frames the guest sends OUT to the consumer (axum forwards them).
    pub outgoing: tokio::sync::mpsc::UnboundedSender<WsFrame>,
}

/// A live consumer WebSocket the daemon owns and hands to the guest's
/// `ws-server.handle`. Stored in `Host.table`; the handle is passed into the
/// guest. `None` means the session has been closed — further sends/recvs fail
/// with [`WsError::Closed`].
pub struct ConsumerStreamResource {
    transport: Option<ConsumerStreamTransport>,
    /// One-frame lookahead, for the same reason as
    /// [`WsStreamResource::pending`]: readiness must not consume.
    pending: Option<std::result::Result<WsFrame, WsError>>,
}

impl ConsumerStreamResource {
    /// The next frame from the consumer, or [`WsError::Closed`] once the axum
    /// side has hung up.
    ///
    /// Cancel-safe: `tokio::sync::mpsc::Receiver::recv` is documented not to
    /// lose a message when its future is dropped, which is what lets `poll`
    /// race this against the upstream's readiness.
    async fn next_frame(&mut self) -> std::result::Result<WsFrame, WsError> {
        let Some(transport) = self.transport.as_mut() else {
            return Err(WsError::Closed);
        };
        if let Some(frame) = transport.incoming.recv().await {
            Ok(frame)
        } else {
            // The consumer (axum) hung up: mark the session closed so the next
            // recv also returns `WsError::Closed`.
            self.transport = None;
            Err(WsError::Closed)
        }
    }
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::Pollable for ConsumerStreamResource {
    async fn ready(&mut self) {
        if self.pending.is_none() {
            self.pending = Some(self.next_frame().await);
        }
    }
}

impl ConsumerStreamResource {
    /// Wrap a live transport for handoff to the guest.
    #[must_use]
    pub fn new(t: ConsumerStreamTransport) -> Self {
        Self {
            transport: Some(t),
            pending: None,
        }
    }
}

/// WebSocket handshake headers the host owns; a guest-supplied header with one
/// of these names is dropped so it cannot corrupt the upgrade request.
const RESERVED_HEADERS: &[&str] = &[
    "host",
    "connection",
    "upgrade",
    "sec-websocket-key",
    "sec-websocket-version",
];

impl self::super_stt::realtime::ws::Host for Host {
    async fn connect(
        &mut self,
        url: String,
        headers: Vec<(String, Vec<u8>)>,
    ) -> Result<Result<Resource<WsStreamResource>, WsError>> {
        let uri: Uri = match url.parse() {
            Ok(u) => u,
            Err(e) => return Ok(Err(WsError::InvalidUrl(format!("invalid url: {e}")))),
        };

        match uri.scheme_str() {
            Some("ws" | "wss") => {}
            other => {
                return Ok(Err(WsError::InvalidUrl(format!(
                    "scheme must be ws or wss, got {}",
                    other.unwrap_or("<none>")
                ))));
            }
        }

        let Some(host) = uri.host() else {
            return Ok(Err(WsError::InvalidUrl("url has no host".to_string())));
        };
        let port = uri.port_u16().unwrap_or_else(|| {
            crate::stt_models::backends::base_url::default_port(uri.scheme_str())
        });

        // Same egress allowlist + SSRF guard the HTTP host enforces, including
        // the relaxation for the user-authorized `base_url` endpoint.
        if let Err(msg) = check_host_allowed(
            &self.hooks.allowed_hosts,
            &self.hooks.user_allowed_hosts,
            host,
            port,
            self.hooks.allow_loopback,
        ) {
            return Ok(Err(WsError::HostNotAllowed(msg)));
        }

        // Build the upgrade request: `into_client_request` fills the reserved
        // handshake headers (Host/Connection/Upgrade/Sec-WebSocket-*). Forward
        // the caller's headers, skipping those reserved names so we never
        // duplicate them (tungstenite rejects duplicates).
        let mut request = match uri.into_client_request() {
            Ok(r) => r,
            Err(e) => return Ok(Err(WsError::InvalidUrl(format!("invalid url: {e}")))),
        };
        for (name, value) in headers {
            if RESERVED_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                continue;
            }
            let header_name = match name.parse::<tokio_tungstenite::tungstenite::http::HeaderName>()
            {
                Ok(n) => n,
                Err(e) => {
                    return Ok(Err(WsError::ConnectFailed(format!(
                        "invalid header name {name}: {e}"
                    ))));
                }
            };
            let header_value =
                match tokio_tungstenite::tungstenite::http::HeaderValue::from_bytes(&value) {
                    Ok(v) => v,
                    Err(e) => {
                        return Ok(Err(WsError::ConnectFailed(format!(
                            "invalid header value for {name}: {e}"
                        ))));
                    }
                };
            request.headers_mut().append(header_name, header_value);
        }

        match connect_async(request).await {
            Ok((stream, _response)) => {
                let resource = self.table.push(WsStreamResource::new(stream))?;
                Ok(Ok(resource))
            }
            Err(e) => Ok(Err(WsError::ConnectFailed(format!("connect failed: {e}")))),
        }
    }
}

// Signatures here are dictated by wasmtime's generated trait, so the
// `async` cannot be dropped from the members that never await (the
// unimplemented ones, and `drop`). `unknown_lints` keeps the attribute
// harmless on toolchains predating the lint.
#[allow(unknown_lints, clippy::unused_async_trait_impl)]
impl self::super_stt::realtime::ws::HostWsStream for Host {
    async fn send_text(
        &mut self,
        self_: Resource<WsStreamResource>,
        text: String,
    ) -> Result<Result<(), WsError>> {
        let res = self.table.get_mut(&self_)?;
        let Some(stream) = res.stream.as_mut() else {
            return Ok(Err(WsError::Closed));
        };
        match stream.send(Message::Text(text.into())).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(WsError::SendFailed(format!("send failed: {e}")))),
        }
    }

    async fn send_binary(
        &mut self,
        self_: Resource<WsStreamResource>,
        data: Vec<u8>,
    ) -> Result<Result<(), WsError>> {
        let res = self.table.get_mut(&self_)?;
        let Some(stream) = res.stream.as_mut() else {
            return Ok(Err(WsError::Closed));
        };
        match stream.send(Message::Binary(data.into())).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(WsError::SendFailed(format!("send failed: {e}")))),
        }
    }

    async fn recv(
        &mut self,
        self_: Resource<WsStreamResource>,
    ) -> Result<Result<WsFrame, WsError>> {
        let res = self.table.get_mut(&self_)?;
        // A frame `subscribe`/`ready` already pulled off the socket is handed
        // over before touching the stream again.
        if let Some(frame) = res.pending.take() {
            return Ok(frame);
        }
        Ok(res.next_frame().await)
    }

    async fn subscribe(
        &mut self,
        self_: Resource<WsStreamResource>,
    ) -> Result<Resource<wasmtime_wasi::p2::bindings::io::poll::Pollable>> {
        // Readiness is backed by the one-frame lookahead: the pollable resolves
        // when a frame has been pulled off the socket, and `recv` then hands
        // that same frame over. Lets a guest wait on its consumer and its
        // upstream at once instead of running half-duplex.
        wasmtime_wasi::p2::subscribe(&mut self.table, self_)
    }

    async fn close(&mut self, self_: Resource<WsStreamResource>) -> Result<Result<(), WsError>> {
        let res = self.table.get_mut(&self_)?;
        if let Some(mut stream) = res.stream.take() {
            let _ = stream.close(None).await;
        }
        Ok(Ok(()))
    }

    async fn drop(&mut self, rep: Resource<WsStreamResource>) -> Result<()> {
        let _ = self.table.delete(rep)?;
        Ok(())
    }
}

// Same as above: the trait is generated, so these signatures are fixed.
#[allow(unknown_lints, clippy::unused_async_trait_impl)]
impl self::super_stt::realtime::ws::HostConsumerStream for Host {
    async fn send_text(
        &mut self,
        self_: Resource<ConsumerStreamResource>,
        text: String,
    ) -> Result<Result<(), WsError>> {
        let res = self.table.get_mut(&self_)?;
        let Some(transport) = res.transport.as_mut() else {
            return Ok(Err(WsError::Closed));
        };
        match transport.outgoing.send(WsFrame::Text(text)) {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(WsError::SendFailed(format!("consumer gone: {e}")))),
        }
    }

    async fn send_binary(
        &mut self,
        self_: Resource<ConsumerStreamResource>,
        data: Vec<u8>,
    ) -> Result<Result<(), WsError>> {
        let res = self.table.get_mut(&self_)?;
        let Some(transport) = res.transport.as_mut() else {
            return Ok(Err(WsError::Closed));
        };
        match transport.outgoing.send(WsFrame::Binary(data)) {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(WsError::SendFailed(format!("consumer gone: {e}")))),
        }
    }

    async fn recv(
        &mut self,
        self_: Resource<ConsumerStreamResource>,
    ) -> Result<Result<WsFrame, WsError>> {
        let res = self.table.get_mut(&self_)?;
        // A frame `subscribe`/`ready` already took off the channel is handed
        // over before awaiting another.
        if let Some(frame) = res.pending.take() {
            return Ok(frame);
        }
        Ok(res.next_frame().await)
    }

    async fn subscribe(
        &mut self,
        self_: Resource<ConsumerStreamResource>,
    ) -> Result<Resource<wasmtime_wasi::p2::bindings::io::poll::Pollable>> {
        // Backed by the same one-frame lookahead as `ws-stream::subscribe`.
        wasmtime_wasi::p2::subscribe(&mut self.table, self_)
    }

    async fn close(
        &mut self,
        self_: Resource<ConsumerStreamResource>,
    ) -> Result<Result<(), WsError>> {
        let res = self.table.get_mut(&self_)?;
        // Dropping the transport closes both channel halves, signalling the
        // axum side that the guest is done.
        res.transport = None;
        Ok(Ok(()))
    }

    async fn drop(&mut self, rep: Resource<ConsumerStreamResource>) -> Result<()> {
        let _ = self.table.delete(rep)?;
        Ok(())
    }
}

/// Register only the `super-stt:realtime/ws` host import on the linker.
///
/// Wired into the component linker for websocket-capable backends; kept here so
/// the host implementation and its registration live together.
///
/// # Errors
/// Returns an error if the import is already defined on the linker.
pub fn add_to_linker(linker: &mut Linker<Host>) -> Result<()> {
    self::super_stt::realtime::ws::add_to_linker::<Host, wasmtime::component::HasSelf<Host>>(
        linker,
        |h| h,
    )?;
    Ok(())
}
