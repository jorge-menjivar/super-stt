# recording_events scope

> Scope: **recording_events** (subscribe, read-only, to recording lifecycle
> events on [`GET /events`](../endpoints/v1/events.md): when a recording starts,
> stops, transcription begins/ends, and a coarse on/off state ping).

This is the lowest-sensitivity event scope: it reveals *that* a recording is
happening, not *what* is being said or heard. A panel applet or overlay uses it
to show a recording indicator without ever touching audio or transcription text.

It does not grant audio data (see [`audio_visualization`](./audio_visualization.md)),
transcription text (see [`global_transcriptions`](./global_transcriptions.md)),
or daemon/model status (see [`daemon_status`](./daemon_status.md)). Compose the
scopes you need in one [`POST /auth/request`](../endpoints/v1/auth/request.md);
see [auth.md](../auth.md).

## Topics

| Topic                  | Carries                                                          |
|------------------------|------------------------------------------------------------------|
| `recording_started`    | `{ client_id, timestamp, write_mode }`                           |
| `recording_stopped`    | `{ client_id, timestamp }` — emitted when mic capture ends        |
| `recording_state`      | `{ is_recording: bool }` — coarse on/off ping                    |
| `transcribing_started` | `{ client_id, timestamp }` — model decode begins                 |
| `transcribing_stopped` | `{ client_id, timestamp, transcription_success, error? }` — decode + typing finished |

Full payload semantics, the closing frames (`event: shutdown`, `event: revoked`),
and the SSE framing rules live on [`/events`](../endpoints/v1/events.md). A
subscription that requests a topic outside the token's scopes fails the whole
stream with `403 scope_denied` before it opens.

## Errors

| HTTP | `message`             | Meaning                                                                                                          |
|------|-----------------------|----------------------------------------------------------------------------------------------------------------|
| 401  | `invalid_session`     | Token expired, unknown, or binary identity changed; re-issue [`/auth/request`](../endpoints/v1/auth/request.md). |
| 403  | `scope_denied`        | Requested a topic this token's scopes don't grant.                                                              |
| 503  | `connection_rejected` | Server refused the connection.                                                                                  |
