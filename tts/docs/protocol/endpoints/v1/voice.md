# `/voice`

The cloned-voice library: the recordings a voice is cloned from, and the ids
that name them on [`POST /speak`](./speak.md).

A voice is stored **once, independently of any model**. `clone_ref_seconds` is
a property of the model — one takes thirty seconds of reference audio and the
next takes ten — so the library keeps the whole recording and the daemon trims
it per model when it hands it over. Switching models never asks the user to
record again.

The daemon pushes a clip to the loaded backend the **first time** a model is
asked to speak in that voice, over
[`POST /v1/voices`](../../backend/contract.md#post-v1voices) on the backend
contract, and remembers that it did. Long text is split into one synthesis
request per sentence, so pushing the clip with each request would re-upload and
re-derive it a dozen times for one paragraph.

Five paths, following the surface's convention that a collection is the `/list`
sub-resource of its singular noun:

| Path | Method | Does |
|---|---|---|
| [`/voice/list`](#get-voicelist)        | `GET`    | Every stored voice, plus what the loaded model can do with them |
| [`/voice`](#post-voice)                | `POST`   | Add one — the body is the WAV itself |
| [`/voice/{id}`](#get-voiceid)          | `GET`    | One voice's metadata |
| [`/voice/{id}`](#patch-voiceid)        | `PATCH`  | Rename it |
| [`/voice/{id}`](#delete-voiceid)       | `DELETE` | Forget it and its recording |
| [`/voice/{id}/audio`](#get-voiceidaudio) | `GET`  | The stored clip, to play back |

These are **cloned** voices. A model's **preset** voices are declared in its
manifest and listed with the model on
[`GET /backend/list`](./backends.md); **described** voices are free text on the
utterance. Which of the three shapes a model accepts is its `voice_kinds`, and
the daemon refuses a shape the model did not opt into — see
[`config.md`](../../backend/config.md#voices).

## Auth

- **Required scope:** `voices` — see [scopes/voices.md](../../scopes/voices.md).
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `voices` scope get `403 scope_denied`.

Speaking in a cloned voice needs `speak`, not `voices`.

## What a stored clip looks like

Every upload is converted to one canonical shape: **mono, 16-bit PCM, 24 kHz**.
Multi-channel input is averaged down rather than having one channel picked, and
anything at another rate is resampled. Backends therefore receive one
predictable format and need no resampler of their own, and 24 kHz is the rate
the speaker encoders in scope expect.

| Bound                | Value       | Why                                                              |
|----------------------|-------------|------------------------------------------------------------------|
| Container            | WAV (PCM)   | 8/16/24/32-bit integer and 32-bit float. See [Formats](#formats). |
| Longest clip         | 120 s       | The archive bound; a model's `clone_ref_seconds` is applied on top. |
| Largest upload       | 48 MiB      | Above 120 s of 48 kHz stereo 24-bit, so the duration error is the one users hit. |
| Longest label        | 128 chars   |                                                                  |
| Longest transcript   | 4000 chars  |                                                                  |

### Formats

WAV only. Decoding mp3/m4a/ogg would put a media parser inside a daemon that
runs for the whole login session, and the path that matters most — recording a
sample — produces PCM already. A client holding a compressed file converts it
before upload.

## `GET /voice/list`

Every stored voice, newest first.

```http
GET /voice/list HTTP/1.1
Authorization: Bearer tts_…64hex…
```

```json
{
  "status": "success",
  "voices": [
    {
      "id":               "2f8a2d0e-9c31-4e77-b0aa-1c6b2f0a51d4",
      "voice_id":         "voice:2f8a2d0e-9c31-4e77-b0aa-1c6b2f0a51d4",
      "label":            "Ada",
      "transcript":       "The quick brown fox jumps over the lazy dog.",
      "created_at":       "2026-09-05T09:14:22.481Z",
      "duration_seconds": 18.4,
      "sample_rate":      24000,
      "channels":         1
    }
  ],
  "model": {
    "name":              "qwen3-tts-1.7b-base",
    "source":            "github.com/super-libre/super-tts-qwen",
    "clones":            true,
    "clone_ref_seconds": 30.0,
    "needs_transcript":  false
  }
}
```

`voice_id` is the string to pass as `voice` on `POST /speak`; `id` is the same
value without its prefix, for building paths under `/voice`. `transcript` is
omitted entirely when the voice was stored without one.

`model` describes what the **currently loaded** model can do with these voices,
and is `null` when nothing is loaded. It is the answer to the three questions a
voices UI has: whether to offer cloning at all (`clones`), how much of a
recording will actually be used (`clone_ref_seconds`), and whether to ask the
user what the clip says (`needs_transcript`). The library itself is
model-independent — these fields change when the user switches models, the
stored voices do not.

A daemon that has never cloned a voice answers `200` with an empty list — the
library directory does not exist until something is written to it.

## `POST /voice`

Add a voice. The **body is the WAV file itself**, not JSON: a recording is
megabytes of PCM, and base64 in a JSON envelope would inflate it by a third and
push it through a parser meant for small objects. Metadata rides in the query
string, which keeps adding a voice to one request.

```http
POST /voice?label=Ada&transcript=The%20quick%20brown%20fox HTTP/1.1
Authorization: Bearer tts_…64hex…
Content-Type: audio/wav
Content-Length: 1587244

RIFF….
```

| Parameter    | Required | Meaning                                                                                     |
|--------------|----------|---------------------------------------------------------------------------------------------|
| `label`      | yes      | Display name. Trimmed; empty is `400 invalid_value`.                                          |
| `transcript` | no       | What the clip says. Needed only by models that declare `clone_needs_transcript` — in-context cloning conditions on the words as well as the audio. Supply it when you have it: it costs nothing and a model that needs it cannot use a voice stored without it. |

**Response (201):**

```json
{ "status": "success", "voice": { "id": "…", "voice_id": "voice:…", "…": "…" } }
```

`201`, not `200`: a new resource exists at `/voice/{id}`.

## `GET /voice/{id}`

One voice's metadata, in the same shape as a list entry. `404 not_found` if
`id` names nothing — including when it is not a uuid at all, so a malformed id
tells a caller nothing about the filesystem.

`{id}` is accepted with or without the `voice:` prefix.

## `PATCH /voice/{id}`

Rename.

```json
{ "label": "Ada Lovelace" }
```

Only the label. The transcript describes the stored audio, and a backend caches
what it derived from the pair — changing it would leave every registration
already made disagreeing with the library. To change what the clip says, add a
new voice.

## `DELETE /voice/{id}`

Forget the voice and its recording.

```json
{ "status": "success", "deleted": "voice:2f8a2d0e-…" }
```

If the model loaded at the time holds the voice, the daemon also asks the
backend to release it. That release is best-effort: the registration lives in
the backend's memory and dies with the instance, so a failure costs a little
memory until the next model switch, never correctness. The voice is deleted
either way.

## `GET /voice/{id}/audio`

The stored clip, so a client can let the user hear what a voice was built from.

```http
HTTP/1.1 200 OK
Content-Type: audio/wav
```

Mono 16-bit PCM at 24 kHz — the canonical form, not the bytes that were
uploaded.

## Speaking in a cloned voice

```http
POST /speak
{ "text": "The kettle is boiling.", "voice": "voice:2f8a2d0e-…" }
```

The daemon checks, in order, that the loaded model accepts `cloned` ids at all
(`voice_kinds` in its manifest), that the library holds the voice, and — when
the model declares `clone_needs_transcript` — that the voice was stored with
one. Each failure is `400 invalid_value` naming what is missing. A model that
does not clone never sees a `voice:` id.

The first utterance in a given voice pays for the registration; later ones do
not. Deleting a voice mid-session does not stop an utterance already in flight.

## Errors

| HTTP | `error_code`                | Meaning                                                                     |
|------|-----------------------------|------------------------------------------------------------------------------|
| 400  | `invalid_value`             | Missing or empty `label`, an over-long label or transcript, or an empty body. |
| 400  | `unsupported_audio`         | The body is not a WAV file this daemon can read.                            |
| 400  | `clip_too_long`             | The recording is longer than 120 s.                                          |
| 403  | `scope_denied`              | The token does not hold `voices`.                                            |
| 404  | `not_found`                 | No voice with that id.                                                       |
| 413  | —                           | The upload exceeded 48 MiB and was rejected before it was read.              |
| 503  | `voice_library_unavailable` | The library could not be read or written.                                    |
