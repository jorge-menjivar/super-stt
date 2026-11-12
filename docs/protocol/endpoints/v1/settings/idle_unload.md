# `/settings/idle_unload`

How long a model may sit unused before the daemon unloads it, in seconds.
Defaults to `0`, which keeps models loaded.

With a value set:

- No stage's selected model is loaded at startup: not transcription's,
  post-processing's or synthesis's.
- A request that needs a stage loads its model first and proceeds once it has
  —
  [`POST /stt/transcribe`](../transcribe.md),
  [`POST /tts/speak`](../tts/speak.md) or
  [`GET /tts/speak/stream`](../tts/speak/stream.md). A transcription loads
  every stage it runs through at once: transcription and, when it is on,
  post-processing. A daemon-mic recording is accepted at once and starts when
  its models have loaded, so the user never waits after speaking.
  `POST /tts/speak`, and a `POST /stt/transcribe` carrying `audio_data`, answer
  after the load, so a client must not time them out sooner.
  `409 model_not_loaded` then means no model is selected or, for those two,
  that the load failed. A post-processor that fails to load costs only the
  cleanup: the transcript passes through unchanged, as it does whenever
  post-processing is down.
- A model unused for that long is unloaded and stays selected. A recording or
  an utterance in progress counts as use of every stage it runs, and the time
  is counted from when it ends. The unload is reported as a `ready` event with
  `model_loaded: false` on [`daemon_status_changed`](../events.md), carrying
  the stage it is about.

One value covers every stage of both features: transcription,
post-processing and synthesis.

An explicit unload
([`DELETE /stt/pipeline/1/model`](../pipeline/model.md),
[`DELETE /tts/pipeline/1/model`](../tts/pipeline/model.md)) is unchanged: it
switches the model off, and nothing loads it again until a client does. That
holds for a model the daemon has already unloaded for being idle: the call
switches it off, so the next request does not load it, and announces it with
the same `ready` event. A call that arrives while a request is loading the
model waits for that load and then unloads it.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /settings/idle_unload`

**Request:**

```http
POST /settings/idle_unload HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
Content-Type: application/json

{
  "after_secs": 300
}
```

| Field        | Type    | Required | Notes                                                                       |
|--------------|---------|----------|-----------------------------------------------------------------------------|
| `after_secs` | integer | yes      | Seconds a model may sit unused before it is unloaded. `0` keeps models loaded. |

The idle time is counted from the change, so a model already unused for longer
is not unloaded the moment a value is set. Setting `0` loads any selected model
that is not in memory, in the background, as a start with `0` would.

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":                 "success",
  "idle_unload_after_secs": 300
}
```

**Errors:**

| HTTP | `message`         | Meaning                                  |
|------|-------------------|------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |

`after_secs` is required and must be a non-negative integer. A malformed body
fails JSON parsing before the request reaches the handler and gets a generic
`400` rejection, not a classified `error_code`.

## `GET /settings/idle_unload`

**Request:**

```http
GET /settings/idle_unload HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":                 "success",
  "idle_unload_after_secs": 0
}
```

**Errors:**

| HTTP | `message`         | Meaning                                  |
|------|-------------------|------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |
