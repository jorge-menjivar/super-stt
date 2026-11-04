# playback_events scope

> Scope: **playback_events** (subscribe, read-only, to speech playback events on
> [`GET /events`](../endpoints/v1/events.md): when speech starts and stops, and
> how far through the current utterance playback has got).

This scope reveals *when* the machine is speaking and how far through it is — it
does **not** reveal what is being said. A panel applet or overlay uses it to show
a speaking indicator without ever seeing the text or the audio.

It is deliberately separate from [`audio_visualization`](./audio_visualization.md):
an applet that draws a waveform holds that scope, and knowing which utterance is
playing and how long it runs is a different disclosure that costs a second
grant. It does not grant the ability to *produce* speech (see
[`speak`](./speak.md)) or daemon/model status (see
[`daemon_status`](./daemon_status.md)). Compose the scopes you need in one
[`POST /auth/request`](../endpoints/v1/auth/request.md); see [auth.md](../auth.md).

## Topics

| Topic             | Carries                                                                 |
|-------------------|--------------------------------------------------------------------------|
| `speaking_state`  | `{ is_speaking: bool, utterance_id?: string }` — coarse on/off, plus the utterance when there is one |
| `speech_progress` | `{ utterance_id: string, spoken_ms: u64, queued_ms: u64 }`               |

`utterance_id` is absent from `speaking_state` when speech has stopped.

`spoken_ms` and `queued_ms` are positions within the utterance derived from the
output device's format, not wall-clock time, so they stay correct while the
stream is idle. `spoken_ms > 0` is the first evidence that audio actually
reached the device: an utterance is accepted before any audio exists for it, and
with a remote backend that gap can be seconds long.

The daemon speaks for whoever asked. An utterance another app started raises
these events too — the events describe the *device*, not your requests. Match on
`utterance_id` against what [`POST /speak`](../endpoints/v1/speak.md) returned if
you need to tell your own utterance apart.

Full payload semantics, the closing frames (`event: shutdown`, `event: revoked`),
and the SSE framing rules live on [`/events`](../endpoints/v1/events.md). A
subscription that requests a topic outside the token's scopes fails the whole
stream with `403 scope_denied` before it opens.

## Errors

| HTTP | `message`             | Meaning                                                                                                          |
|------|-----------------------|------------------------------------------------------------------------------------------------------------------|
| 401  | `invalid_session`     | Token expired, unknown, or binary identity changed; re-issue [`/auth/request`](../endpoints/v1/auth/request.md). |
| 403  | `scope_denied`        | Requested a topic this token's scopes don't grant.                                                              |
| 503  | `connection_rejected` | Server refused the connection.                                                                                  |
