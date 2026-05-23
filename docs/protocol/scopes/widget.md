# Widget scope

> Scope: **widget** (subscribe-only — `GET /events` plus the
> [auth handshake](../auth.md). No configuration access, no recording
> control.)

The widget scope is for visualizers, panel applets, and overlays:
anything that displays *what the daemon is doing* without driving
the recording itself. A widget can show:

- Whether a recording is in progress.
- The audio waveform / frequency bands while recording.
- Live or final transcription text (optional opt-in).

A widget cannot:

- Start or stop recording (use [client](./client.md) for that).
- Read or modify any daemon configuration value (use [settings](./settings.md)).
- Issue any other endpoint. The only verbs a widget calls are
  [`POST /auth/request`](../endpoints/v1/auth/request.md) (once),
  [`GET /events`](../endpoints/v1/events.md) (long-lived), and the
  scope-agnostic [`GET /ping`](../endpoints/v1/ping.md) /
  [`GET /auth/status`](../endpoints/v1/auth/status.md) probes.

> The COSMIC applet shipped with this repo is a widget-scope client.

## Why widget is its own scope

A widget receives a continuous broadcast firehose during recording.
Two consequences:

1. **A leak is loud.** Even a few seconds of stolen audio frames is
   a privacy event. A long-lived widget subscription is checked for
   binary replacement; on mismatch the stream ends with `event:
   revoked` (reason `exe_changed`) and the widget must re-auth —
   see [auth.md § widget anti-replacement](../auth.md#widget-anti-replacement).
2. **It must be high-throughput.** Audio frames stream at tens of
   kB/sec during recording. A slow consumer loses oldest queued
   frames rather than stalling the stream — see
   [transport.md § slow consumers](../transport.md#slow-consumers).

## Endpoint reference

| Endpoint                                                        | Method     | Notes                                                                          |
|-----------------------------------------------------------------|------------|--------------------------------------------------------------------------------|
| [`/auth/request`](../endpoints/v1/auth/request.md)              | POST       | One-time consent with `scope: "widget"`. See [auth.md](../auth.md).            |
| [`/auth/status`](../endpoints/v1/auth/status.md)                | GET        | Probe whether the held token is still valid                                    |
| [`/ping`](../endpoints/v1/ping.md)                              | GET        | Liveness probe                                                                  |
| [`/events`](../endpoints/v1/events.md)                          | GET (SSE)  | Subscribe to widget-scope topics                                                |

A widget token requesting anything else gets `403 scope_denied`.

## Topics

Widget tokens can request the following SSE topics; settings-only
topics (`daemon_status_changed`, `download_progress`) return `403
scope_denied` before the stream opens.

| Topic                  | Carries                                                                                  |
|------------------------|------------------------------------------------------------------------------------------|
| `recording_started`    | `{ client_id, timestamp, write_mode }`                                                    |
| `recording_stopped`    | `{ client_id, timestamp, transcription_success, error }`                                  |
| `recording_state`      | `{ is_recording: bool }` — coarse on/off ping                                             |
| `audio_samples`        | `{ sample_rate, channels, samples_b64 }` — base64-encoded f32 PCM                         |
| `frequency_bands`      | `{ bands_b64, sample_rate, total_energy }` — base64-encoded f32 visualization bands       |
| `partial_stt`          | `{ text, confidence }` — live transcription preview                                       |
| `final_stt`            | `{ text, confidence }` — final transcription text                                         |

Full payload semantics, the closing frames (`event: shutdown`,
`event: revoked`), and the SSE framing rules live on
[`/events`](../endpoints/v1/events.md).

## Errors

| HTTP | `message`             | Meaning                                                                                                      |
|------|-----------------------|--------------------------------------------------------------------------------------------------------------|
| 401  | `invalid_session`     | Token expired, unknown, or binary identity changed; re-issue [`/auth/request`](../endpoints/v1/auth/request.md). |
| 403  | `scope_denied`        | Tried a non-widget endpoint, or a settings-only topic.                                                       |
| 503  | `connection_rejected` | Server refused the connection.                                                                                |
