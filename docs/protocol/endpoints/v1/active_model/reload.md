# `POST /active_model/reload`

Re-instantiate the currently-loaded model **in place** (same identity) so a
changed backend secret or option takes effect without picking a different model.
It is a no-op when no model is loaded, and is rejected while a daemon-mic
recording is active. A real-time (WebSocket) session holds the `model` read lock,
so a reload requested during one serializes behind it rather than being rejected.

Unlike [`POST /active_model`](../active_model.md#post-active_model) (which starts
a possibly-long switch and returns `202`), reload is **synchronous** — the
response is sent after the model has been re-instantiated.

The active model state itself is read and written via
[`/active_model`](../active_model.md).

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /active_model/reload`

**Request:**

```http
POST /active_model/reload HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

No request body.

**Response (200) — reloaded:**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":  "success",
  "message": "Successfully switched to model: whisper-tiny"
}
```

On success the daemon re-instantiates the same `(model, source)` on the
current device preference and broadcasts `model_switched` then `ready` on
[`/events?topics=daemon_status_changed`](../events.md) — identical to a completed
switch, so subscribers converge on the same state.

**Response (200) — nothing loaded:**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":  "success",
  "message": "No active model to reload"
}
```

**Errors:** `recording_in_progress` carries its identifier in `error_code`; the
auth failures carry theirs in `message`, and the 500 is uncoded.

| HTTP | Identifier              | Carried in   | Meaning                                              |
|------|-------------------------|--------------|------------------------------------------------------|
| 401  | `invalid_session`       | `message`    | Token unknown / expired / `exe_changed`              |
| 403  | `scope_denied`          | `message`    | Token lacks the `settings` scope                     |
| 409  | `recording_in_progress` | `error_code` | A daemon-mic recording is active — stop it and retry |
| 500  | *(uncoded)*             | `message`    | Re-instantiation failed (`Model reload failed: …`); the previous model is unloaded |
