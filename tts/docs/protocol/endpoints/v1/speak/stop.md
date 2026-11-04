# `POST /speak/stop`

Stop the utterance now playing and discard whatever audio is queued behind it.

**Idempotent, and never an error.** Stopping when nothing is speaking succeeds:
a client cancelling on a keypress should not have to win a race to get a clean
result, and should never have to distinguish "I stopped it" from "it had already
finished" to avoid showing the user an error.

Unlike closing a connection, this works regardless of who started the utterance.
[`POST /speak`](../speak.md) returns as soon as the audio is queued, so by the
time a user reaches for a stop button there is no connection left to close —
which is the whole reason this endpoint exists.

## Auth

- **Required scope:** `speak`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `speak` scope get `403 scope_denied`.

## Request

```http
POST /speak/stop HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
```

No request body.

## Response (200)

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "success",
  "utterance_id": "utt_9f2c1e40"
}
```

| Field          | Present when                                                        |
|----------------|---------------------------------------------------------------------|
| `utterance_id` | Something was speaking; this is the utterance that was cancelled.   |
| *(absent)*     | Nothing was speaking. Still `success`.                              |

Use the presence of `utterance_id` — not the status — to tell the two apart.

A stop is immediate: in-flight synthesis is cancelled, the queue is dropped, and
the audio ring is flushed. The daemon then publishes `speaking_state`
with `is_speaking: false` to
[`/events`](../events.md) subscribers.

## Errors

| HTTP | `message`         | Meaning                                                     |
|------|-------------------|-------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed` — re-auth and retry |
| 403  | `scope_denied`    | Token lacks the `speak` scope                               |
| 429  | `rate_limited`    | Too many requests; back off                                 |
