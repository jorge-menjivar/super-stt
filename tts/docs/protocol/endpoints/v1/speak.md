# `POST /speak`

Synthesize `text` with the active model and play it on the daemon's output
device.

The request is `text` and nothing else; see [the removed
fields](#the-removed-fields) if you are porting a client that sent more.

The request returns as soon as the utterance is **queued**, not when it finishes
playing — the daemon keeps speaking after the connection closes. To follow an
utterance to its end, subscribe to
[`speaking_state` and `speech_progress`](./events.md) with the
[`playback_events`](../../scopes/playback_events.md) scope; those outlive any
one request, which is what a status widget actually needs.

**One utterance at a time, newest wins.** A `/speak` while something is already
playing interrupts it — that is what "speak this instead" means. Taking the
model away does the same: unloading it, switching it, reloading it, moving it
to another device or changing its backend stops whatever it was saying rather
than waiting for it (see [`/pipeline/{stage}/model`](./pipeline/model.md)).

To stream text in as it is generated — an LLM reply spoken as it arrives —
use [`GET /speak/stream`](./speak/stream.md) instead. This endpoint takes
complete text.

## Auth

- **Required scope:** `speak`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `speak` scope get `403 scope_denied`.

## Request

```http
POST /speak HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
Content-Type: application/json

{
  "text": "The kettle is boiling."
}
```

| Field  | Type   | Required | Meaning                                                        |
|--------|--------|----------|-----------------------------------------------------------------|
| `text` | string | yes      | What to speak. Empty or whitespace-only is `400 invalid_value`. |

**That is the whole request.** How an utterance is spoken — which voice, which
language, how fast, in what manner — is the user's configuration, read per
utterance by the daemon:

| To change          | Use                                                                             |
|--------------------|----------------------------------------------------------------------------------|
| The voice          | [`POST /pipeline/{stage}/model/{model}/voice`](./pipeline/voice.md) (`settings`) |
| The language       | [`POST /settings/language`](./settings/language.md) (`settings`)                 |

An app that can make the machine talk does not thereby get a say in which voice
it talks in. `speak` buys the speakers; choosing what comes out of them is a
separate grant the user makes separately.

### The removed fields

`voice`, `language`, `speed` and `instructions` used to be accepted here. A
request still carrying one is **refused**, not ignored — a client given a
different voice than it asked for has no way to find out:

```http
HTTP/1.1 400 Bad Request
Content-Type: application/json

{
  "status": "error",
  "error_code": "invalid_value",
  "message": "voice is no longer accepted on /speak; how an utterance is spoken is configuration — set a voice with POST /pipeline/{stage}/model/{model}/voice and a language with POST /settings/language"
}
```

A field present but `null` is not naming one: that is how a client spells "use
what is configured", which is what every request means now.

`speed` and `instructions` have no setting to move to yet. Rate and delivery are
not adjustable over the protocol until one exists.

The daemon normalizes `text` before synthesis — markup that would otherwise be
read out literally (emphasis markers, code fences, link targets, table pipes) is
stripped, and long text is split on sentence boundaries so playback can start
before the whole thing is synthesized. A line break that ends a line — a
heading, a list item, a paragraph — is a boundary too, period or no period, so
each is spoken as its own utterance rather than run into the next; a line break
that merely wraps a sentence is a space. Numbers, dates, and currency are passed
through untouched: the model reads them, because a wrong number-to-words is
worse than none.

`text` is capped at **50 000 characters**. A model may declare a lower per-request
cap in its manifest (`max_input_chars`), which the daemon applies on top.

## Response (202)

```http
HTTP/1.1 202 Accepted
Content-Type: application/json

{
  "status": "success",
  "utterance_id": "utt_9f2c1e40"
}
```

`202`, not `200`: the audio has been accepted, and the device is still playing
it when you read this.

`utterance_id` is what correlates this call with the playback events, and what
[`/speak/stop`](./speak/stop.md) reports having cancelled. It is returned from
the first version of this endpoint even though the current policy is
one-at-a-time — adding an id later would break every client written against it.

## Errors

| HTTP | `error_code`        | Meaning                                                                                     |
|------|---------------------|---------------------------------------------------------------------------------------------|
| 400  | `invalid_value`     | `text` missing, empty, or over the cap; a `voice` the model does not declare or whose shape it did not opt into (see [`voice_kinds`](../../backend/config.md#voices)); or a malformed field. |
| 401  | `invalid_session`   | Token unknown / expired / `exe_changed` — re-auth and retry.                                |
| 403  | `scope_denied`      | Token lacks the `speak` scope.                                                              |
| 409  | `model_not_loaded`  | No model is loaded. Load one via [`POST /pipeline/1/model`](./pipeline/model.md#post-pipelinestagemodel) and retry.        |
| 429  | `rate_limited`      | Too many requests; back off.                                                                |
| 500  | —                   | The backend failed, or the output device could not be opened. `message` carries the reason.  |

A failure the caller did not cause — no model, a dead audio device, a backend
that refused — also raises a desktop notification, because a `speak` is as
likely to come from a keyboard shortcut as from an app with a UI to show the
error in. See [`/settings/notification_method`](./settings/notification_method.md).
