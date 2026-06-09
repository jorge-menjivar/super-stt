# audio_visualization scope

> Scope: **audio_visualization** (subscribe, read-only, to pre-computed
> frequency-band data on [`GET /events`](../endpoints/v1/events.md) for drawing
> an audio visualizer).

The daemon runs the FFT and broadcasts ready-to-render frequency bands; this
scope grants those bands and nothing else. Raw PCM is deliberately **not**
exposed on the wire — a visualizer gets the bar heights it needs without
receiving reconstructable audio. The COSMIC applet shipped with this repo is a
client of this scope (plus [`recording_events`](./recording_events.md)).

## Topics

| Topic             | Carries                                                                                  |
|-------------------|------------------------------------------------------------------------------------------|
| `frequency_bands` | `{ bands_b64, sample_rate, total_energy }` — base64-encoded f32 visualization bands       |

Full payload semantics and the SSE framing rules live on
[`/events`](../endpoints/v1/events.md). A subscription that requests a topic
outside the token's scopes fails the whole stream with `403 scope_denied` before
it opens.

## Errors

| HTTP | `message`             | Meaning                                                                                                          |
|------|-----------------------|----------------------------------------------------------------------------------------------------------------|
| 401  | `invalid_session`     | Token expired, unknown, or binary identity changed; re-issue [`/auth/request`](../endpoints/v1/auth/request.md). |
| 403  | `scope_denied`        | Requested a topic this token's scopes don't grant.                                                              |
| 503  | `connection_rejected` | Server refused the connection.                                                                                  |
