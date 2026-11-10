# Building on Super Voice

Super Voice is designed to be built on. The daemon exposes a documented
HTTP protocol over a local socket, and the models it runs are out-of-tree
backends that anyone can author and publish. It has two features, speech to
text and text to speech, behind one socket, one token and one set of scopes. This directory is the
protocol reference; this page is the entry point to it.

There are two ways to build on Super Voice:

- **[Build a client](#build-a-client)** — any app, in any language, that
  wants transcriptions, wants to speak text, follows recording or playback,
  or controls the daemon.
- **[Add your own model](#add-your-own-model)** — package a speech or voice
  model as a backend the daemon can install and run.

---

## Build a client

A client talks to the daemon over an HTTP/1.1 + JSON API on a Unix domain
socket (`<runtime dir>/voice/super-voice-http.sock` — see
[transport.md](./transport.md#where-the-daemon-listens) for where that is on
each platform). No Rust required —
`curl`, Python, Node, or anything with an HTTP client works.

```bash
# 1. Ask for consent. The user approves your app once, for the scopes you
#    request; the daemon returns a session token bound to your binary.
curl --unix-socket "${XDG_RUNTIME_DIR:-$(getconf DARWIN_USER_TEMP_DIR)}/voice/super-voice-http.sock" \
     -X POST http://voice.local/auth/request \
     -H 'Content-Type: application/json' \
     -d '{"app_name":"My App","scopes":["transcribe","status"],"version":"0.1"}'
# → { "session_token": "…", "scopes": [...], "expires_at": "…" }

# 2. Send the token on every subsequent request.
curl --unix-socket "${XDG_RUNTIME_DIR:-$(getconf DARWIN_USER_TEMP_DIR)}/voice/super-voice-http.sock" \
     -X POST http://voice.local/stt/transcribe \
     -H "Authorization: Bearer $TOKEN" -d '{"wait":true}'

# A token holding `speak` makes the daemon talk instead.
curl --unix-socket "${XDG_RUNTIME_DIR:-$(getconf DARWIN_USER_TEMP_DIR)}/voice/super-voice-http.sock" \
     -X POST http://voice.local/tts/speak \
     -H "Authorization: Bearer $TOKEN" -d '{"text":"Hello from my app."}'
```

What the protocol gives you:

- **Consent-based auth.** A token is minted only after the user approves
  your app in a popup, and it is bound to your binary's identity — an app
  cannot widen its own permissions. See [auth.md](./auth.md).
- **Fine-grained scopes.** Request exactly what you need from `transcribe`,
  `speak`, `voices`, `status`, `settings`, `secrets`, `recording_events`,
  `playback_events`, `audio_visualization`, `global_transcriptions`,
  `daemon_status`. Each is documented under [scopes/](./scopes/).
- **Live event streams.** Subscribe over Server-Sent Events
  (`GET /events?topics=…`) to recording state, speaking state, playback
  progress, audio frequency bands, model/download status, and final
  transcription text. See [endpoints/v1/events.md](./endpoints/v1/events.md).
- **Realtime transcription.** Realtime-capable models are driven over a
  WebSocket session at `/stt/transcribe/realtime`.
- **Streaming text in.** Send an LLM's reply as it is generated over a
  WebSocket at [`/tts/speak/stream`](./endpoints/v1/tts/speak/stream.md), and
  the daemon speaks each sentence as it completes.

Reference:

- **[Browse the API reference](https://jorge-menjivar.github.io/super-voice/protocol/)**
  — every endpoint, parameter and response shape, rendered from
  [openapi.json](https://jorge-menjivar.github.io/super-voice/protocol/openapi.json).
  Point a client generator at that file to get a typed client in your
  language.

  Two renderings of the same document, each linking to the other:
  [Swagger UI](https://jorge-menjivar.github.io/super-voice/protocol/) and
  [Scalar](https://jorge-menjivar.github.io/super-voice/protocol/scalar.html).
  Locally, `just openapi-serve` opens the first and `just openapi-serve
  --scalar` the second. The Swagger view offers "Try it out", which reaches a
  running daemon over its TCP listener — a browser cannot dial the Unix
  socket.
- [transport.md](./transport.md) — the wire shape: HTTP framing, SSE, error
  envelopes, connection lifecycle, and a minimal non-Rust client recipe.
- [auth.md](./auth.md) — the consent handshake, tokens, and the full scope
  catalog.
- [endpoints/](./endpoints/) — every endpoint, request/response by request.
- [scopes/](./scopes/) — what each scope unlocks.

The OpenAPI document is generated from the daemon's router, not maintained
alongside it: every route is registered through the same declaration that
carries its documentation. It is not committed; CI generates it on every push
and publishes it at the link above, and `just openapi` writes it locally to
`docs/protocol/openapi.json`. The prose above is not replaced by it — that
explains *when* to call an endpoint and how the pieces fit, where the spec
states the shapes exactly, for tooling.

## The endpoints

Every path is under `/v1`. Five conventions run through the whole surface, and
knowing them means most paths can be guessed rather than looked up:

1. **A feature's routes carry its name** — `/v1/stt/…` for speech to text,
   `/v1/tts/…` for text to speech. What both features share sits directly
   under `/v1/`.
2. **Resource nouns are singular** — `/backend`, `/model`, `/voice`, `/option`.
3. **A collection is the `/list` sub-resource of its singular noun** —
   `/tts/voice/list`, `/backend/list`, `/stt/pipeline/1/model/list`.
4. **What *is* set and what *may be* set are separate paths** — `/language`
   beside `/language/list`, `/device` beside `/device/list`. Only one of the
   two changes when a user picks something, so only one has to be re-read.
5. **Every setting lives under `/settings/`.** Sharing the `settings` *scope*
   is not the same as being a setting: `/backend`, the pipelines, `/registry`,
   `/gpu_info` and `/update` are gated by that scope but are not user
   preferences, so they are not namespaced under it.

### Speech to text

| Path | Methods | Scope | Reference |
|---|---|---|---|
| `/v1/stt/transcribe` | `POST` | `transcribe` | [transcribe.md](./endpoints/v1/transcribe.md) |
| `/v1/stt/transcribe/stop` | `POST` | `transcribe` | [transcribe/stop.md](./endpoints/v1/transcribe/stop.md) |
| `/v1/stt/transcribe/realtime` | `GET` (WebSocket) | `transcribe` | [backend/contract.md](./backend/contract.md#consumer-facing-endpoint) |
| `/v1/stt/context/list` | `GET` | `settings` | [context.md](./endpoints/v1/context.md#get-sttcontextlist) |
| `/v1/stt/context/{id}` | `GET`, `POST`, `DELETE` | `settings` | [context.md](./endpoints/v1/context.md#get-sttcontextid) |
| `/v1/stt/context/active` | `GET`, `POST` | `settings` | [context.md](./endpoints/v1/context.md#get-sttcontextactive) |
| `/v1/stt/backend/{backend_id}/context` | `GET`, `POST`, `DELETE` | `settings` | [context.md](./endpoints/v1/context.md#get-sttbackendbackend_idcontext) |

### Text to speech

| Path | Methods | Scope | Reference |
|---|---|---|---|
| `/v1/tts/speak` | `POST` | `speak` | [tts/speak.md](./endpoints/v1/tts/speak.md) |
| `/v1/tts/speak/stop` | `POST` | `speak` | [tts/speak/stop.md](./endpoints/v1/tts/speak/stop.md) |
| `/v1/tts/speak/stream` | `GET` (WebSocket) | `speak` | [tts/speak/stream.md](./endpoints/v1/tts/speak/stream.md) |
| `/v1/tts/voice/list` | `GET` | `voices` | [tts/voice.md](./endpoints/v1/tts/voice.md#get-ttsvoicelist) |
| `/v1/tts/voice` | `POST` | `voices` | [tts/voice.md](./endpoints/v1/tts/voice.md#post-ttsvoice) |
| `/v1/tts/voice/{id}` | `GET`, `PATCH`, `DELETE` | `voices` | [tts/voice.md](./endpoints/v1/tts/voice.md#get-ttsvoiceid) |
| `/v1/tts/voice/{id}/audio` | `GET` | `voices` | [tts/voice.md](./endpoints/v1/tts/voice.md#get-ttsvoiceidaudio) |

### Session and state

| Path | Methods | Scope | Reference |
|---|---|---|---|
| `/v1/auth/request` | `POST` | — (unauthenticated) | [auth/request.md](./endpoints/v1/auth/request.md) |
| `/v1/auth/status` | `GET` | any | [auth/status.md](./endpoints/v1/auth/status.md) |
| `/v1/ping` | `GET` | any | [ping.md](./endpoints/v1/ping.md) |
| `/v1/events` | `GET` (SSE) | any | [events.md](./endpoints/v1/events.md) |
| `/v1/status` | `GET` | `status` | [status.md](./endpoints/v1/status.md) |
| `/v1/gpu_info` | `GET` | `settings` | [gpu_info.md](./endpoints/v1/gpu_info.md) |
| `/v1/update` | `GET` | `settings` | [update.md](./endpoints/v1/update.md) |
| `/v1/update/check` | `POST` | `settings` | [update/check.md](./endpoints/v1/update/check.md) |

### The pipelines

Each feature has a pipeline: the ordered stages its work passes through.
Speech to text has two, **stage 1, `transcription`** and **stage 2,
`post_processor`** ([pipeline.md](./endpoints/v1/pipeline.md)). Text to speech
has one, **stage 1, `synthesis`**
([tts/pipeline.md](./endpoints/v1/tts/pipeline.md)). Stages are addressed by
number so that a position could be appended without a second endpoint family.
A position a pipeline does not have answers `404 unknown_stage`.

Both families have the same shape, under `/v1/stt` and `/v1/tts`, and every
path needs the `settings` scope:

| Path | Methods | Speech to text | Text to speech |
|---|---|---|---|
| `/pipeline` | `GET` | [pipeline.md](./endpoints/v1/pipeline.md#get-sttpipeline) | [tts/pipeline.md](./endpoints/v1/tts/pipeline.md#get-ttspipeline) |
| `/pipeline/{stage}` | `GET`, `POST`, `DELETE` | [pipeline/stage.md](./endpoints/v1/pipeline/stage.md) | [tts/pipeline/stage.md](./endpoints/v1/tts/pipeline/stage.md) |
| `/pipeline/{stage}/backend/list` | `GET` | [pipeline/backend-list.md](./endpoints/v1/pipeline/backend-list.md) | [tts/pipeline/backend-list.md](./endpoints/v1/tts/pipeline/backend-list.md) |
| `/pipeline/{stage}/device/list` | `GET` | [pipeline/device.md](./endpoints/v1/pipeline/device.md#get-sttpipelinestagedevicelist) | [tts/pipeline/device.md](./endpoints/v1/tts/pipeline/device.md#get-ttspipelinestagedevicelist) |
| `/pipeline/{stage}/model` | `GET`, `POST`, `DELETE` | [pipeline/model.md](./endpoints/v1/pipeline/model.md) | [tts/pipeline/model.md](./endpoints/v1/tts/pipeline/model.md) |
| `/pipeline/{stage}/model/cancel` | `POST` | [pipeline/model.md](./endpoints/v1/pipeline/model.md#post-sttpipelinestagemodelcancel) | [tts/pipeline/model.md](./endpoints/v1/tts/pipeline/model.md#post-ttspipelinestagemodelcancel) |
| `/pipeline/{stage}/model/list` | `GET` | [pipeline/model-list.md](./endpoints/v1/pipeline/model-list.md) | [tts/pipeline/model-list.md](./endpoints/v1/tts/pipeline/model-list.md) |
| `/pipeline/{stage}/model/reload` | `POST` | [pipeline/model.md](./endpoints/v1/pipeline/model.md#post-sttpipelinestagemodelreload) | [tts/pipeline/model.md](./endpoints/v1/tts/pipeline/model.md#post-ttspipelinestagemodelreload) |
| `/pipeline/{stage}/model/{model}/device` | `GET`, `POST` | [pipeline/device.md](./endpoints/v1/pipeline/device.md) | [tts/pipeline/device.md](./endpoints/v1/tts/pipeline/device.md) |
| `/pipeline/{stage}/model/{model}/device/list` | `GET` | [pipeline/device.md](./endpoints/v1/pipeline/device.md#get-sttpipelinestagemodelmodeldevicelist) | [tts/pipeline/device.md](./endpoints/v1/tts/pipeline/device.md#get-ttspipelinestagemodelmodeldevicelist) |
| `/pipeline/{stage}/model/{model}/language` | `GET`, `POST`, `DELETE` | [pipeline/language.md](./endpoints/v1/pipeline/language.md) | [tts/pipeline/language.md](./endpoints/v1/tts/pipeline/language.md) |
| `/pipeline/{stage}/model/{model}/language/list` | `GET` | [pipeline/language.md](./endpoints/v1/pipeline/language.md#get-sttpipelinestagemodelmodellanguagelist) | [tts/pipeline/language.md](./endpoints/v1/tts/pipeline/language.md#get-ttspipelinestagemodelmodellanguagelist) |
| `/pipeline/{stage}/model/{model}/voice` | `GET`, `POST`, `DELETE` | — | [tts/pipeline/voice.md](./endpoints/v1/tts/pipeline/voice.md) |
| `/pipeline/{stage}/model/{model}/voice/list` | `GET` | — | [tts/pipeline/voice.md](./endpoints/v1/tts/pipeline/voice.md#get-ttspipelinestagemodelmodelvoicelist) |

### Backends

| Path | Methods | Scope | Reference |
|---|---|---|---|
| `/v1/backend/list` | `GET` | `settings` | [backends.md](./endpoints/v1/backends.md) |
| `/v1/backend/{backend_id}` | `DELETE` | `settings` | [backends.md](./endpoints/v1/backends.md#delete-backendbackend_id) |
| `/v1/backend/{backend_id}/option/list` | `GET` | `settings` | [backends/options.md](./endpoints/v1/backends/options.md) |
| `/v1/backend/{backend_id}/option/{name}` | `GET`, `POST`, `DELETE` | `settings` | [backends/options.md](./endpoints/v1/backends/options.md) |
| `/v1/backend/{backend_id}/secret/list` | `GET` | `secrets` | [backends/secrets.md](./endpoints/v1/backends/secrets.md) |
| `/v1/backend/{backend_id}/secret/{name}` | `GET`, `POST`, `DELETE` | `secrets` | [backends/secrets.md](./endpoints/v1/backends/secrets.md) |
| `/v1/registry/backend/list` | `GET` | `settings` | [registry/backends.md](./endpoints/v1/registry/backends.md) |
| `/v1/registry/backend/preview` | `POST` | `settings` | [registry/preview.md](./endpoints/v1/registry/preview.md) |
| `/v1/registry/backend/install` | `POST` | `settings` | [registry/install.md](./endpoints/v1/registry/install.md) |
| `/v1/registry/backend/refresh` | `POST` | `settings` | [registry/refresh.md](./endpoints/v1/registry/refresh.md) |
| `/v1/registry/backend/update` | `POST` | `settings` | [registry/update.md](./endpoints/v1/registry/update.md) |

`{backend_id}` is the backend's repo `source`, percent-encoded.

### Settings

| Path | Methods | Scope | Reference |
|---|---|---|---|
| `/v1/settings/audio_theme` | `GET`, `POST` | `settings` | [settings/audio_theme.md](./endpoints/v1/settings/audio_theme.md) |
| `/v1/settings/audio_theme/list` | `GET` | `settings` | [settings/audio_theme/list.md](./endpoints/v1/settings/audio_theme/list.md) |
| `/v1/settings/audio_theme/test` | `POST` | `settings` | [settings/audio_theme/test.md](./endpoints/v1/settings/audio_theme/test.md) |
| `/v1/settings/language` | `GET`, `POST`, `DELETE` | `settings` | [settings/language.md](./endpoints/v1/settings/language.md) |
| `/v1/settings/language/list` | `GET` | `settings` | [settings/language/list.md](./endpoints/v1/settings/language/list.md) |
| `/v1/settings/notification_method` | `GET`, `POST` | `settings` | [settings/notification_method.md](./endpoints/v1/settings/notification_method.md) |
| `/v1/settings/preview_typing` | `GET`, `POST` | `settings` | [settings/preview_typing.md](./endpoints/v1/settings/preview_typing.md) |
| `/v1/settings/recording_stop_mode` | `GET`, `POST` | `settings` | [settings/recording_stop_mode.md](./endpoints/v1/settings/recording_stop_mode.md) |
| `/v1/settings/update_beta_optin` | `GET`, `POST` | `settings` | [settings/update_beta_optin.md](./endpoints/v1/settings/update_beta_optin.md) |
| `/v1/settings/update_check_enabled` | `GET`, `POST` | `settings` | [settings/update_check_enabled.md](./endpoints/v1/settings/update_check_enabled.md) |
| `/v1/settings/volume` | `GET`, `POST` | `settings` | [settings/volume.md](./endpoints/v1/settings/volume.md) |
| `/v1/settings/write_method` | `GET`, `POST` | `settings` | [settings/write_method.md](./endpoints/v1/settings/write_method.md) |
| `/v1/settings/write_method/test` | `POST` | `settings` | [settings/write_method/test.md](./endpoints/v1/settings/write_method/test.md) |

## Add your own model

A model is delivered by a **backend**: an out-of-tree program the daemon
loads at runtime. Backends implement one HTTP-shaped contract, packaged in
one of two transports:

| Transport      | Use for                              | Isolation                                   |
|----------------|--------------------------------------|---------------------------------------------|
| WASM component | Cloud / API providers, light CPU     | wasmtime sandbox; network egress allowlisted |
| Native subprocess | Local, GPU-accelerated models     | Network-isolated; systemd + seccomp hardened |

To make a model available in Super Voice:

1. **Build a backend** implementing the daemon↔backend contract —
   [backend/contract.md](./backend/contract.md), plus
   [backend/wasm.md](./backend/wasm.md) or
   [backend/subprocess.md](./backend/subprocess.md) for your transport, and
   [backend/config.md](./backend/config.md) for the `backend.toml` manifest.
2. **Publish it** by opening a PR that adds your repo to the registry — see
   [registry/README.md](../../registry/README.md). A nightly job discovers
   your releases and publishes them to the catalog every user browses;
   afterwards, shipping a new version is just tagging a release.

## Security model

Auth, transport, and the daemon's security posture are described in the
references above and in [../SECURITY.md](../SECURITY.md).
