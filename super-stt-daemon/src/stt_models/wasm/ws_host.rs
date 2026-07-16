// SPDX-License-Identifier: GPL-3.0-only
//! Host implementation of the outgoing `super-stt:realtime/ws` interface.
//!
//! A realtime backend imports `super-stt:realtime/ws` to open a WebSocket to
//! its upstream cloud API. The daemon implements that import here, against the
//! same egress allowlist + SSRF guard the HTTP host uses (see
//! [`host::check_host_allowed`]) so a realtime backend's only network reach is
//! its declared `allowed_hosts`.
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
}

impl WsStreamResource {
    fn new(stream: WebSocketStream<MaybeTlsStream<TcpStream>>) -> Self {
        Self {
            stream: Some(stream),
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
}

impl ConsumerStreamResource {
    /// Wrap a live transport for handoff to the guest.
    #[must_use]
    pub fn new(t: ConsumerStreamTransport) -> Self {
        Self { transport: Some(t) }
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
        let port = uri.port_u16().unwrap_or(match uri.scheme_str() {
            Some("ws") => 80,
            _ => 443,
        });

        // Same egress allowlist + SSRF guard the HTTP host enforces.
        if let Err(msg) = check_host_allowed(
            &self.hooks.allowed_hosts,
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
        let Some(stream) = res.stream.as_mut() else {
            return Ok(Err(WsError::Closed));
        };
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(text))) => {
                    return Ok(Ok(WsFrame::Text(text.as_str().to_string())));
                }
                Some(Ok(Message::Binary(data))) => {
                    return Ok(Ok(WsFrame::Binary(data.into())));
                }
                Some(Ok(Message::Close(frame))) => {
                    // Emit the close once, then mark the stream closed so the
                    // next recv returns `WsError::Closed`.
                    res.stream = None;
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
                    return Ok(Ok(WsFrame::Close(close)));
                }
                // Ping/pong are handled by tungstenite's auto-pong; skip them
                // and read the next data frame.
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                Some(Err(e)) => {
                    res.stream = None;
                    return Ok(Err(WsError::RecvFailed(format!("recv failed: {e}"))));
                }
                None => {
                    res.stream = None;
                    return Ok(Err(WsError::Closed));
                }
            }
        }
    }

    async fn subscribe(
        &mut self,
        _self_: Resource<WsStreamResource>,
    ) -> Result<Resource<wasmtime_wasi::p2::bindings::io::poll::Pollable>> {
        // Wiring a real wasi:io/poll Pollable into the tungstenite stream is
        // non-trivial (it needs a host-side waker the poll loop can await).
        // The first realtime backend polls `recv` sequentially instead, so
        // this stays unimplemented for now.
        wasmtime::bail!("ws-stream::subscribe is not yet implemented")
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
        let Some(transport) = res.transport.as_mut() else {
            return Ok(Err(WsError::Closed));
        };
        if let Some(frame) = transport.incoming.recv().await {
            Ok(Ok(frame))
        } else {
            // The consumer (axum) hung up: mark the session closed so the next
            // recv also returns `WsError::Closed`.
            res.transport = None;
            Ok(Err(WsError::Closed))
        }
    }

    async fn subscribe(
        &mut self,
        _self_: Resource<ConsumerStreamResource>,
    ) -> Result<Resource<wasmtime_wasi::p2::bindings::io::poll::Pollable>> {
        // Same documented limitation as `ws-stream::subscribe`: a real
        // wasi:io/poll Pollable needs a host-side waker; the first realtime
        // backend polls `recv` sequentially instead.
        wasmtime::bail!("consumer-stream::subscribe is not yet implemented")
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
