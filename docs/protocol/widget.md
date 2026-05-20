# Widget Protocol

> Scope: **widget** (subscribe-only — no endpoints beyond auth and
> `GET /events`, no configuration access).

The widget protocol is for visualizers, panel applets, and overlays:
anything that displays *what the daemon is doing* without driving the
recording itself. A widget can show:

- Whether a recording is in progress.
- The audio waveform / frequency bands while recording.
- Live or final transcription text (optional opt-in).

A widget cannot:

- Start or stop recording (use [client](./client.md) for that).
- Read or modify any daemon configuration value (use [settings](./settings.md)).
- Issue any other endpoint. The only verbs available to a widget are
  the authentication handshake and `GET /events`.

> The COSMIC applet shipped with this repo is a widget-scope client.

## Why widget is its own scope

The widget receives a continuous broadcast firehose: dozens of audio
frames per second during recording, plus state events. Two
consequences:

1. **A leak is loud.** Even a few seconds of stolen audio frames is a
   privacy event. The daemon detects binary replacement during the
   long-lived SSE connection by re-checking `/proc/<peer_pid>/exe`
   against the keyring entry — see
   [auth.md](./auth.md#widget-anti-replacement).
2. **It must be high-throughput.** Audio frames stream at tens of kB/sec
   during recording. The daemon uses a bounded per-subscriber queue
   that drops oldest frames on overflow rather than backpressuring the
   capture pipeline.

## Single transport

All widget traffic — auth, subscription, audio fan-out, state events —
flows over **one** Unix-socket HTTP connection per stream. There is
no separate UDP listener. See [transport.md](./transport.md) for the
full HTTP shape.

```mermaid
flowchart LR
    daemon[(Daemon)]
    sock(("Unix socket<br/>$XDG_RUNTIME_DIR/stt/super-stt.sock"))
    widget["Widget app"]

    daemon -- "JSON responses (one-shot HTTP)<br/>+ SSE event stream (long-lived HTTP)" --> sock
    sock --> widget
    widget -- "POST /auth/request once<br/>+ GET /events for the lifetime of the widget" --> sock
```

## Endpoints

A widget calls only two endpoints:

| Endpoint              | Method | Purpose                                                     |
|-----------------------|--------|-------------------------------------------------------------|
| `/auth/request`       | POST   | One-time consent (scope = `widget`) — see [auth.md](./auth.md) |
| `/events`             | GET (SSE) | Subscribe to recording state, audio frames, and (optionally) transcription text |

Every other settings-scope or client-scope endpoint returns
`403 scope_denied` for a widget token.

## Topics available to the widget scope

| Topic                  | Carries                                                        |
|------------------------|----------------------------------------------------------------|
| `recording_started`    | `{ client_id, timestamp, write_mode }`                         |
| `recording_stopped`    | `{ client_id, timestamp, transcription_success, error }`       |
| `recording_state`      | `{ is_recording: bool }` — coarse state ping                   |
| `audio_samples`        | `{ sample_rate, channels, samples_b64 }` — base64-encoded f32 PCM |
| `frequency_bands`      | `{ bands_b64, sample_rate, total_energy }` — base64-encoded f32 bands for visualization |
| `partial_stt`          | `{ text, confidence }` — live transcription text (opt-in)      |
| `final_stt`            | `{ text, confidence }` — final transcription text (opt-in)     |

Audio sample and frequency-band payloads use base64 inside the SSE
JSON `data` field. At ~30 KB/s of f32 PCM the encoding overhead
(~33 %) is a rounding error on a local socket. If a widget needs the
absolute most efficient binary path, the daemon may later expose a
`Content-Type: application/octet-stream` chunked variant on a
different endpoint — but most visualizers don't need it.

## Subscribing

```mermaid
sequenceDiagram
    autonumber
    participant W as "Widget"
    participant D as "Daemon"

    W->>D: POST /auth/request<br/>{ app_name, scope: "widget" }
    D-->>W: 200 { session_token, expires_at }

    W->>D: GET /events?topics=recording_started,<br/>      recording_stopped,<br/>      audio_samples,<br/>      frequency_bands<br/>Authorization: Bearer <token>

    Note right of D: Daemon registers a subscriber<br/>internally and assigns subscriber_id.<br/>Verifies /proc/{pid}/exe matches keyring.

    D-->>W: 200 OK<br/>Content-Type: text/event-stream
    D-->>W: event: subscribed<br/>data: { client_id, subscribed_to }

    Note over W,D: persistent connection,<br/>any-client events delivered

    loop until disconnect
        Note right of D: Another app's recording triggers events —<br/>daemon publishes them on the stream.
        D-->>W: event: recording_started<br/>data: { client_id, timestamp, write_mode }
        loop while recording
            D-->>W: event: audio_samples<br/>data: { sample_rate, channels, samples_b64 }
            D-->>W: event: frequency_bands<br/>data: { bands_b64, total_energy }
        end
        D-->>W: event: recording_stopped<br/>data: { client_id, timestamp, transcription_success }
    end
```

The widget chooses its topic set at subscription time. A simple
visualizer might subscribe only to `frequency_bands` +
`recording_started` / `recording_stopped`. A richer overlay could add
`partial_stt` / `final_stt` to show transcription text.

## Anti-replacement re-check

The daemon periodically re-reads `/proc/<peer_pid>/exe` while the SSE
stream is open and compares it to the keyring entry. If the binary
changed, the daemon sends `event: revoked\ndata: { reason:
"exe_changed" }\n\n` and closes the connection. The widget must
re-issue `/auth/request` (which triggers a fresh user popup) before
re-subscribing.

```mermaid
sequenceDiagram
    autonumber
    participant W as "Widget"
    participant D as "Daemon"
    participant K as "Daemon keyring<br/>(internal)"

    Note over W,D: SSE stream open

    Note over D: Periodic re-check<br/>(at recording boundaries + every 30s)
    D->>K: lookup widget session
    D->>D: read /proc/{peer_pid}/exe
    alt exe_path matches
        D-->>W: continue streaming events
    else exe_path mismatch (binary moved or replaced)
        D->>K: delete session entry
        D-->>W: event: revoked<br/>data: { reason: "exe_changed" }
        D-->>W: closes SSE stream
        Note over W: must POST /auth/request again<br/>(fresh popup)
    end
```

Because the SSE stream runs over the same Unix-socket HTTP connection
as the request that opened it, `SO_PEERCRED` continues to identify
the peer PID throughout the connection's lifetime. There's no need
for an HMAC challenge — the kernel-verified peer credential is the
trust anchor.

For TCP-bound widget clients (browser-based visualizers), peer
credentials aren't available, so the `Origin` header (browser-
enforced) is the trust anchor. Origin checks happen at request
acceptance and on each periodic re-check.

## Errors specific to widgets

| HTTP | `message`                  | Meaning                                                            |
|------|----------------------------|--------------------------------------------------------------------|
| 403  | `scope_denied`             | Widget tried a mutation or non-allowed endpoint — only `/events` and `/auth/*` are allowed |
| 401  | `invalid_session`          | Token expired or `/proc/<pid>/exe` changed; re-issue `/auth/request` |
| 503  | `connection_rejected`      | Resource manager refused the connection                             |

If the daemon detects that a streaming widget has fallen too far
behind, it drops the oldest queued events for that subscriber but
keeps the connection open. The widget will see fresh events once it
catches up; old events are gone.

## A typical widget session

```mermaid
sequenceDiagram
    autonumber
    participant W as "Widget"
    participant D as "Daemon"

    Note over W,D: 1. Authenticate (one time)
    W->>D: POST /auth/request<br/>{ app_name, scope: "widget" }
    D-->>W: 200 { session_token, expires_at }
    W->>W: persist token in keyring

    Note over W,D: 2. Open the long-lived event stream
    W->>D: GET /events?topics=recording_started,<br/>      recording_stopped,<br/>      audio_samples,<br/>      frequency_bands
    D-->>W: 200 SSE stream

    Note over W,D: 3. While the daemon is running
    loop forever
        D-->>W: event: recording_started
        loop during one recording
            D-->>W: event: audio_samples / frequency_bands
        end
        D-->>W: event: recording_stopped
    end

    Note over W,D: 4. Token rotation (every 30 days)
    Note over W: On any 401 invalid_session,<br/>POST /auth/request and re-subscribe.
```

For a working example, look at `super-stt-cosmic-applet`. It opens a
single `GET /events` SSE subscription with `topics=recording_state,
frequency_bands,audio_samples` and routes each event into its
visualization state machine.
