# `GET /events`

Long-lived Server-Sent Events subscription. One connection delivers
every event in the requested topic set until either side closes
the stream. The byte-level SSE mechanics — comment keep-alives,
slow-consumer behavior, revoked / shutdown frames — live in
[`transport.md`](../../transport.md).

## Auth

- **Required scope:** any valid token, then **per topic**. Each requested topic
  is gated by the scope that grants it (see the Topics tables below):
  `recording_events`, `audio_visualization`, `global_transcriptions`, or
  `daemon_status`.
- `Authorization: Bearer <session_token>` is required.
- Requesting a topic the token's scopes don't grant fails the **whole**
  subscription with `403 scope_denied` before the stream opens — partial
  subscriptions are not supported.

## `GET /events`

**Request:**

```http
GET /events?topics=recording_state,frequency_bands HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
Accept: text/event-stream
```

| Query param | Required | Notes                                                                            |
|-------------|----------|----------------------------------------------------------------------------------|
| `topics`    | yes      | Comma-separated list of topic names. Repeating `?topics=` is also accepted.      |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: text/event-stream
Cache-Control: no-store

event: subscribed
data: {"client_id":"sub_…","subscribed_to":["recording_state","frequency_bands"]}

event: recording_state
data: {"is_recording":true}

event: frequency_bands
data: {"bands_b64":"…","sample_rate":16000.0,"total_energy":0.0042}

…
```

The very first frame is always `event: subscribed`, carrying the
assigned subscriber id and the resolved topic list. Subsequent
frames have `event:` set to the topic name and `data:` set to the
topic-specific JSON payload (no outer wrapper).

The stream may end with `event: shutdown` (the daemon is going
away) or `event: revoked` (the session is no longer accepted —
reasons include `expired`, `exe_changed`, etc.). After a `revoked`
frame the client must re-issue
[`POST /auth/request`](./auth/request.md) before reopening.

## Topics

Each topic lists the scope a token must hold to subscribe to it. A token
requesting a topic outside its granted scopes gets `403 scope_denied` for the
whole subscription.

### Recording state

| Topic                  | Scope              | Payload                                                                                   |
|------------------------|--------------------|-------------------------------------------------------------------------------------------|
| `recording_started`    | `recording_events` | `{ client_id, timestamp, write_mode }` — emitted when mic capture begins, after the start cue |
| `recording_stopped`    | `recording_events` | `{ client_id, timestamp }` — emitted when mic capture ends, before transcription           |
| `recording_state`      | `recording_events` | `{ is_recording: bool }` — `true` at capture start, `false` when the mic releases (mic-capture phase only; not the whole cycle) |
| `transcribing_started` | `recording_events` | `{ client_id, timestamp }` — model decode of the captured audio has begun                 |
| `transcribing_stopped` | `recording_events` | `{ client_id, timestamp, transcription_success, error? }` — decode + typing finished       |

For one daemon-mic capture the lifecycle events fire in this order:
`recording_started` → (`partial_stt` … — `global_transcriptions` scope only) → `recording_stopped` →
`transcribing_started` → `final_stt` (global_transcriptions scope only) → `transcribing_stopped`.
`recording_state{is_recording:true}` accompanies `recording_started` and
`recording_state{is_recording:false}` accompanies `recording_stopped` — i.e.
the visualization signal drops at mic-stop, independent of the transcription
that follows.

That is the order the daemon *publishes* them in. Each topic of a subscription
is forwarded on its own task, so two events from different topics can still
reach the client in either order — notably
`recording_state{is_recording:false}` and `transcribing_started`, which are
published microseconds apart. Fold lifecycle events into your state without
assuming their relative arrival order: treat a mic-stop as "capture ended", not
as "the cycle is over", and return to idle only on `transcribing_stopped`.

**Lifecycle terminal and optional events:**
- `transcribing_started` is emitted only when model decode actually begins
  (Phase 4). It is skipped entirely when the cycle fails during audio capture,
  and when the capture contained no speech — in the latter case the cycle still
  completes successfully, with `final_stt` carrying empty text.
- `final_stt` is emitted only on a successful transcription (including "no
  speech detected", where `text` is empty). A failed cycle reports its error
  via `transcribing_stopped.error`; no `final_stt` is emitted.
- `transcribing_stopped` always closes the cycle — success or failure —
  and is the unconditional signal clients use to return to idle, regardless
  of whether they observed `transcribing_started` or `final_stt`.

### Audio fan-out

| Topic             | Scope                 | Payload                                                                                   |
|-------------------|-----------------------|------------------------------------------------------------------------------------------|
| `frequency_bands` | `audio_visualization` | `{ "bands_b64", "sample_rate", "total_energy" }` — base64-encoded f32 visualization bands of the microphone |
| `playback_frequency_bands` | `audio_visualization` | The same shape, of the speech being played |

Raw PCM is not exposed on the wire; the daemon computes the frequency bands and
broadcasts only those. `frequency_bands` is what the microphone hears while a
recording runs. `playback_frequency_bands` is what the speakers play; its
`sample_rate` is the *backend's* rate — the analysis runs on the audio as
synthesized, before it is resampled for the output device.

`playback_frequency_bands` carries no text and no utterance id: it says how loud the speakers
are, not what is being said or which request produced it. Knowing *which*
utterance is playing costs [`playback_events`](../../scopes/playback_events.md)
as a separate grant.


### Transcription

| Topic         | Scope                   | Payload                                              |
|---------------|-------------------------|------------------------------------------------------|
| `partial_stt` | `global_transcriptions` | `{ "text", "confidence", "source" }` — live preview text. `source` is `"stream"` (a realtime model's running transcript: `text` is everything heard so far and extends the last event) or `"window"` (a sliding window the daemon re-transcribed: `text` is only the last few seconds and replaces the last event); see [previews](./transcribe.md#previews). Absent from a daemon older than the field. |
| `final_stt`   | `global_transcriptions` | `{ "text", "confidence" }` — final transcription     |

These carry the transcription text of every app, not just the subscriber's own —
hence the dedicated high-sensitivity scope. See
[global_transcriptions.md](../../scopes/global_transcriptions.md).

`confidence` is best-effort: backends that don't expose a confidence score
report `1.0`.

### Playback state

| Topic             | Scope             | Payload                                                                                          |
|-------------------|-------------------|--------------------------------------------------------------------------------------------------|
| `speaking_state`  | `playback_events` | `{ is_speaking: bool, utterance_id?: string }` — `utterance_id` is absent when speech has stopped |
| `speech_progress` | `playback_events` | `{ utterance_id: string, spoken_ms: u64, queued_ms: u64 }`                                       |

For one utterance the events fire in this order:

`speaking_state{is_speaking:true}` → `speech_progress` … (repeatedly, as audio
is rendered) → `speaking_state{is_speaking:false}`.

**What the timings mean.** `spoken_ms` and `queued_ms` are positions within the
utterance derived from the output device's format, not wall-clock time, so they
stay correct while the stream is idle. `spoken_ms > 0` is the first evidence
that audio actually reached the device: an utterance is accepted before any
audio exists for it, and with a remote backend that gap can be seconds long. A
widget that wants to distinguish "synthesizing" from "speaking" uses exactly
that transition.

**When the stop fires.** `speaking_state{is_speaking:false}` marks the end of
the *audio*, not the end of synthesis. The two are far apart: a local backend
runs several times faster than realtime, so the last sample is produced —
and [`POST /tts/speak`](./tts/speak.md) has long since answered `202` — while most of
the utterance is still buffered ahead of the speakers. The daemon holds the
utterance open until that buffer has played out, and keeps publishing
`speech_progress` for the whole of it, so `spoken_ms` climbs to the length of
the utterance before the stop arrives. A client that needs to know when the
voice actually stopped can wait for this event and nothing else.

The one exception is a sink that stops consuming — unplugged, suspended,
disconnected. The daemon waits a few seconds for it, then publishes the stop
anyway rather than leaving the utterance open for good.

**Whose utterance.** The daemon speaks for whoever asked, so these events
describe the *device*, not your requests — an utterance another app started
raises them too. Match `utterance_id` against what
[`POST /tts/speak`](./tts/speak.md) returned to tell your own apart.

**One at a time, newest wins.** A `/tts/speak` while something is playing interrupts
it. The superseded utterance does not emit its own
`speaking_state{is_speaking:false}` — the state belongs to the device, and
announcing a stop for the old id while the new one is playing would show a
subscriber "not speaking" during speech.

### Daemon status

| Topic                   | Scope           | Payload                                                                                                                                                                                                                                                                          |
|-------------------------|-----------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `daemon_status_changed` | `daemon_status` | Heterogeneous; the `status` field discriminates: `loading_model`, `ready` (carries `model_loaded` and optionally `actual_device` / `preferred_device` / `model_name`), `model_switched` (carries the full identity of the now-active model — `model_name`, `source`, `actual_device` — so a client can record it without prior state; emitted whenever a model becomes active: a user switch, the daemon's startup load of the persisted model, or an in-place reload to apply a changed secret/option, each immediately followed by the matching `ready`), `switching_device`, `loading_model_for_device`, `device_switch_error`, `active_backend_changed` (carries `source` — the active backend's repo id, or `null` when the backend is cleared), `settings_changed` (carries `setting` — the changed setting, currently `"language"`, `"update_check_enabled"`, `"update_beta_optin"`, `"post_processor"`, or `"contexts"`; signals clients to re-fetch that settings state, e.g. a per-model language that follows the global value, or the [dictation contexts](./context.md) another app just edited), `update_available` (carries `latest_version` — the candidate release's tag; emitted when a check newly finds an available update or the candidate version changes; clients should refetch [`GET /update`](./update.md) on receipt). Always includes `timestamp`. The variants describing a stage running a model — `loading_model`, `loading_model_for_device`, `model_switched`, `ready`, `switching_device`, `device_switch_error` — also carry `stage`, the [`/stt/pipeline/{stage}`](./pipeline/stage.md) position whose model they are about: every stage reports its own lifecycle here, and these events are the only notice a client gets of a load it did not start (the daemon's startup load, or another client's). A payload without `stage` comes from a daemon older than the field and is stage 1's, the only stage that emitted them then. The daemon-wide facts — `settings_changed`, `update_available`, `active_backend_changed` — carry no stage. |
| `download_progress`     | `daemon_status` | `{ "model_name", "source", "stage", "current_file", "file_index", "total_files", "bytes_downloaded", "total_bytes", "percentage", "status" ("verifying"/"downloading"/"loading_model"/"completed"/"cancelled"/"error"), "eta_seconds", "timestamp", "error"? }`. `bytes_downloaded`/`total_bytes`/`percentage` are per-file — all reset at each file boundary, so `percentage` (0–100) tracks the current file and the `file_index`/`total_files` counter conveys position in the set. Every load opens in `verifying`: each file already on disk is checked against its declared SHA-256 before anything is fetched, and when that check has to read the file (no verification stamp for these exact bytes) `bytes_downloaded` counts the bytes hashed so far, so a multi-GB checksum reports as a moving bar rather than a stall. A load whose files are all present goes `verifying` → `loading_model` without ever reporting `downloading` — a client must therefore take its wording from `status` and not assume a download is under way. `downloading` means bytes are coming off the network for the current file. `loading_model` is emitted once all files are on disk and the backend is loading weights into memory (an untracked phase); `percentage` pins to 100 for `loading_model` and `completed`. `error` is a human-readable failure detail present only on the terminal `status` = `"error"` tick (omitted otherwise), covering any switch failure — download, spawn, or weight-load — so a client can show why a switch failed without a second request. `source` is the backend serving `model_name` and `stage` the [`/stt/pipeline/{stage}`](./pipeline/stage.md) position the download is provisioning for — a model name alone does not say whose download it is, and both stages download through this tracker. A payload without `stage` comes from a daemon older than the field and is stage 1's. Throttled to ~1 % increments, plus an unthrottled publish on each file boundary and status change. |
| `registry_install`      | `daemon_status` | Backend-registry install / refresh progress — a serialized registry event (`install.progress` / `install.completed` / `install.failed` / `refresh.completed` / `refresh.failed`). |

Both features publish `daemon_status_changed` and `download_progress`. Each
payload about a feature names it in `feature`: `"stt"` or `"tts"`. A client that
is about one feature ignores the other's — a speech model loading must not move
a dictation app's progress bar. The daemon's own news names no feature and is
every client's: `update_available`, and `settings_changed` for a setting both
features read (the global `language`, `update_check_enabled`,
`update_beta_optin`). A per-model `language`, `post_processor` and `contexts`
are a feature's. Text to speech has one stage, so its payloads carry no `stage`,
and it adds two `daemon_status_changed` statuses of its own, `preparing_voice`
and `voice_prepared` (carrying `voice` and `model`; see **Preparing a cloned
voice** below).

### Preparing a cloned voice

Before a model can speak in a `voice:<uuid>`, the daemon hands it the reference
clip and the backend derives what it needs from it — a speaker embedding, and
for a clip stored with a transcript the codec codes that make it an in-context
example. That derivation is GPU work over the whole clip, and on a clip length
the backend has not encoded before it can run for seconds to tens of seconds.

```
event: daemon_status_changed
data: {"status":"preparing_voice","voice":"voice:2f8a2d0e-…","model":"qwen3-tts-0.6b-base","timestamp":"…"}

event: daemon_status_changed
data: {"status":"voice_prepared","voice":"voice:2f8a2d0e-…","model":"qwen3-tts-0.6b-base","timestamp":"…"}
```

`voice_prepared` always follows `preparing_voice` for the same id, and carries
`error` only when the preparation failed — absent means it worked, as in
`download_progress`. A failure here is not fatal to anything: the next request
to speak in that voice tries again and reports properly if it still cannot.

**When they fire.** Once per voice per loaded model. [`POST /tts/voice`](./tts/voice.md)
starts a preparation as soon as a clip is saved, so the cost lands while the
user is still looking at the library they just added to rather than on their
first request to speak. A voice that was never prepared that way is prepared by
the first [`POST /tts/speak`](./tts/speak.md) naming it, which is what holds that request
open. A model switch discards every preparation, so the pair fires again against
the new model; backends may cache what they derived between loads, in which case
the second preparation is quick but still announced.

**What a client does with it.** Show that the voice is being prepared, and
expect no audio until `voice_prepared`. A `POST /tts/speak` issued during the
preparation does not queue behind it — a new utterance supersedes the old one,
so pressing again cancels the preview that was about to play.

**What is in the payload.** `voice` is the wire id and nothing else. The label,
the transcript and the recording itself stay behind the
[`voices` scope](../../scopes/voices.md), which is granted separately from the
`daemon_status` scope that carries this event.


## Closing the stream

There is no explicit `unsubscribe` request — closing the HTTP
connection ends the subscription. The stream may also close from
the server side with one of:

| Closing frame      | `data:` payload                                                                  | Client should                                                                                  |
|--------------------|----------------------------------------------------------------------------------|------------------------------------------------------------------------------------------------|
| `event: shutdown`  | `{}`                                                                             | Reconnect later — the daemon is restarting / going away.                                       |
| `event: revoked`   | `{ "reason": "expired" \| "exe_changed" \| ... }`                                | Treat the cached token as gone. Call [`POST /auth/request`](./auth/request.md) before retrying. |

## Errors

| HTTP | `message`             | Meaning                                                                |
|------|-----------------------|------------------------------------------------------------------------|
| 400  | `invalid_topic`       | `topics` was empty, contained an unknown name, or couldn't be parsed.   |
| 401  | `invalid_session`     | Token unknown / expired / `exe_changed` — re-auth and reopen.           |
| 403  | `scope_denied`        | A topic outside the token's granted scopes was requested.               |
| 503  | `connection_rejected` | Server refused the connection (overloaded).                             |
