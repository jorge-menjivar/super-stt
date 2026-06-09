# global_transcriptions scope

> Scope: **global_transcriptions** (subscribe, read-only, to the live and final
> transcription text of **every** app on [`GET /events`](../endpoints/v1/events.md),
> not just your own).

This is the highest-sensitivity event scope. Its topics carry the full text the
user dictates through *any* client — including passwords, messages, and anything
else spoken into another app. Request it only when reading other apps'
transcriptions is the actual purpose of the client, and expect the consent popup
to call this out plainly.

Reading back your *own* transcription results does **not** require this scope —
that is part of [`transcribe`](./transcribe.md), returned inline on the
`/transcribe` response.

## Topics

| Topic         | Carries                                              |
|---------------|------------------------------------------------------|
| `partial_stt` | `{ text, confidence }` — live transcription preview  |
| `final_stt`   | `{ text, confidence }` — final transcription text    |

Full payload semantics and the SSE framing rules live on
[`/events`](../endpoints/v1/events.md). A subscription that requests a topic
outside the token's scopes fails the whole stream with `403 scope_denied` before
it opens.

Because this scope rides a long-lived stream carrying transcription text, the
subscription is checked for binary replacement: on mismatch the stream ends with
`event: revoked` (reason `exe_changed`) and the client must re-auth — see
[auth.md § anti-replacement](../auth.md#anti-replacement).

## Errors

| HTTP | `message`             | Meaning                                                                                                          |
|------|-----------------------|----------------------------------------------------------------------------------------------------------------|
| 401  | `invalid_session`     | Token expired, unknown, or binary identity changed; re-issue [`/auth/request`](../endpoints/v1/auth/request.md). |
| 403  | `scope_denied`        | Requested a topic this token's scopes don't grant.                                                              |
| 503  | `connection_rejected` | Server refused the connection.                                                                                  |
