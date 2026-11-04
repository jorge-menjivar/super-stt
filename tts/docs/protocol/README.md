# Building on Super TTS

Super TTS is designed to be built on. The daemon exposes a documented
HTTP protocol over a local socket, and the models it runs are out-of-tree
backends that anyone can author and publish. This directory is the
protocol reference; this page is the entry point to it.

There are two ways to build on Super TTS:

- **[Build a client](#build-a-client)** — any app, in any language, that
  wants to speak text, follow playback, or control the daemon.
- **[Add your own model](#add-your-own-model)** — package a voice model
  as a backend the daemon can install and run.

---

## Build a client

A client talks to the daemon over an HTTP/1.1 + JSON API on a Unix domain
socket (`$XDG_RUNTIME_DIR/tts/super-tts-http.sock`). No Rust required —
`curl`, Python, Node, or anything with an HTTP client works.

```bash
# 1. Ask for consent. The user approves your app once, for the scopes you
#    request; the daemon returns a session token bound to your binary.
curl --unix-socket "$XDG_RUNTIME_DIR/tts/super-tts-http.sock" \
     -X POST http://tts.local/auth/request \
     -H 'Content-Type: application/json' \
     -d '{"app_name":"My App","scopes":["speak","status"],"version":"0.1"}'
# → { "session_token": "tts_…", "scopes": [...], "expires_at": "…" }

# 2. Send the token on every subsequent request.
curl --unix-socket "$XDG_RUNTIME_DIR/tts/super-tts-http.sock" \
     -X POST http://tts.local/speak \
     -H "Authorization: Bearer $TTS_TOKEN" -d '{"text":"Hello from my app."}'
```

What the protocol gives you:

- **Consent-based auth.** A token is minted only after the user approves
  your app in a popup, and it is bound to your binary's identity — an app
  cannot widen its own permissions. See [auth.md](./auth.md).
- **Fine-grained scopes.** Request exactly what you need from `speak`,
  `status`, `settings`, `secrets`, `voices`, `playback_events`,
  `audio_visualization`, `daemon_status`. Each is documented under
  [scopes/](./scopes/).
- **Live event streams.** Subscribe over Server-Sent Events
  (`GET /events?topics=…`) to speaking state, playback progress, audio
  frequency bands, and model/download status. See
  [endpoints/v1/events.md](./endpoints/v1/events.md).
- **Streaming text in.** Send an LLM's reply as it is generated over a
  WebSocket at [`/speak/stream`](./endpoints/v1/speak/stream.md), and the
  daemon speaks each sentence as it completes.

Reference:

- [transport.md](./transport.md) — the wire shape: HTTP framing, SSE, error
  envelopes, connection lifecycle, and a minimal non-Rust client recipe.
- [auth.md](./auth.md) — the consent handshake, tokens, and the full scope
  catalog.
- [endpoints/](./endpoints/) — every endpoint, request/response by request.
- [scopes/](./scopes/) — what each scope unlocks.

## The endpoints

Every path is under `/v1`. Four conventions run through the whole surface, and
knowing them means most paths can be guessed rather than looked up:

- **Resource nouns are singular** — `/backend`, `/model`, `/voice`, `/option`.
- **A collection is the `/list` sub-resource of its singular noun** —
  `/voice/list`, `/backend/list`, `/pipeline/1/model/list`.
- **What *is* set and what *may be* set are separate paths** — `/language`
  beside `/language/list`, `/device` beside `/device/list`. Only one of the two
  changes when a user picks something, so only one has to be re-read.
- **Every setting lives under `/settings/`, and nothing else does.** Sharing
  the `settings` *scope* is not the same as being a setting: `/backend`,
  `/pipeline`, `/registry`, `/gpu_info` and `/update` are gated by that scope
  but are not user preferences, so they are not namespaced under it.

### Speaking

| Path | Methods | Scope | Reference |
|---|---|---|---|
| `/v1/speak` | `POST` | `speak` | [speak.md](./endpoints/v1/speak.md) |
| `/v1/speak/stop` | `POST` | `speak` | [speak/stop.md](./endpoints/v1/speak/stop.md) |
| `/v1/speak/stream` | `GET` (WebSocket) | `speak` | [speak/stream.md](./endpoints/v1/speak/stream.md) |
| `/v1/voice/list` | `GET` | `voices` | [voice.md](./endpoints/v1/voice.md#get-voicelist) |
| `/v1/voice` | `POST` | `voices` | [voice.md](./endpoints/v1/voice.md#post-voice) |
| `/v1/voice/{id}` | `GET`, `PATCH`, `DELETE` | `voices` | [voice.md](./endpoints/v1/voice.md#get-voiceid) |
| `/v1/voice/{id}/audio` | `GET` | `voices` | [voice.md](./endpoints/v1/voice.md#get-voiceidaudio) |

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

### The pipeline

The ordered stages an utterance passes through. There is exactly one —
**stage 1, `synthesis`** — and it is addressed by number so that a second
position could be appended without a second endpoint family. See
[pipeline.md](./endpoints/v1/pipeline.md).

| Path | Methods | Scope | Reference |
|---|---|---|---|
| `/v1/pipeline` | `GET` | `settings` | [pipeline.md](./endpoints/v1/pipeline.md#get-pipeline) |
| `/v1/pipeline/{stage}` | `GET`, `POST`, `DELETE` | `settings` | [pipeline/stage.md](./endpoints/v1/pipeline/stage.md) |
| `/v1/pipeline/{stage}/backend/list` | `GET` | `settings` | [pipeline/backend-list.md](./endpoints/v1/pipeline/backend-list.md) |
| `/v1/pipeline/{stage}/device/list` | `GET` | `settings` | [pipeline/device.md](./endpoints/v1/pipeline/device.md#get-pipelinestagedevicelist) |
| `/v1/pipeline/{stage}/model` | `GET`, `POST`, `DELETE` | `settings` | [pipeline/model.md](./endpoints/v1/pipeline/model.md) |
| `/v1/pipeline/{stage}/model/cancel` | `POST` | `settings` | [pipeline/model.md](./endpoints/v1/pipeline/model.md#post-pipelinestagemodelcancel) |
| `/v1/pipeline/{stage}/model/list` | `GET` | `settings` | [pipeline/model-list.md](./endpoints/v1/pipeline/model-list.md) |
| `/v1/pipeline/{stage}/model/reload` | `POST` | `settings` | [pipeline/model.md](./endpoints/v1/pipeline/model.md#post-pipelinestagemodelreload) |
| `/v1/pipeline/{stage}/model/{model}/device` | `GET`, `POST` | `settings` | [pipeline/device.md](./endpoints/v1/pipeline/device.md) |
| `/v1/pipeline/{stage}/model/{model}/device/list` | `GET` | `settings` | [pipeline/device.md](./endpoints/v1/pipeline/device.md#get-pipelinestagemodelmodeldevicelist) |
| `/v1/pipeline/{stage}/model/{model}/language` | `GET`, `POST`, `DELETE` | `settings` | [pipeline/language.md](./endpoints/v1/pipeline/language.md) |
| `/v1/pipeline/{stage}/model/{model}/language/list` | `GET` | `settings` | [pipeline/language.md](./endpoints/v1/pipeline/language.md#get-pipelinestagemodelmodellanguagelist) |
| `/v1/pipeline/{stage}/model/{model}/voice` | `GET`, `POST`, `DELETE` | `settings` | [pipeline/voice.md](./endpoints/v1/pipeline/voice.md) |
| `/v1/pipeline/{stage}/model/{model}/voice/list` | `GET` | `settings` | [pipeline/voice.md](./endpoints/v1/pipeline/voice.md#get-pipelinestagemodelmodelvoicelist) |

A position this build does not have answers `404 unknown_stage` — so
`GET /v1/pipeline/2` is an error today, and the shape of the answer when it
stops being one.

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
| `/v1/settings/allow_online_models` | `GET`, `POST` | `settings` | [settings/allow_online_models.md](./endpoints/v1/settings/allow_online_models.md) |
| `/v1/settings/audio_theme` | `GET`, `POST` | `settings` | [settings/audio_theme.md](./endpoints/v1/settings/audio_theme.md) |
| `/v1/settings/audio_theme/list` | `GET` | `settings` | [settings/audio_theme/list.md](./endpoints/v1/settings/audio_theme/list.md) |
| `/v1/settings/audio_theme/test` | `POST` | `settings` | [settings/audio_theme/test.md](./endpoints/v1/settings/audio_theme/test.md) |
| `/v1/settings/custom_models_dir` | `GET`, `POST` | `settings` | [settings/custom_models_dir.md](./endpoints/v1/settings/custom_models_dir.md) |
| `/v1/settings/language` | `GET`, `POST`, `DELETE` | `settings` | [settings/language.md](./endpoints/v1/settings/language.md) |
| `/v1/settings/language/list` | `GET` | `settings` | [settings/language/list.md](./endpoints/v1/settings/language/list.md) |
| `/v1/settings/notification_method` | `GET`, `POST` | `settings` | [settings/notification_method.md](./endpoints/v1/settings/notification_method.md) |
| `/v1/settings/update_beta_optin` | `GET`, `POST` | `settings` | [settings/update_beta_optin.md](./endpoints/v1/settings/update_beta_optin.md) |
| `/v1/settings/update_check_enabled` | `GET`, `POST` | `settings` | [settings/update_check_enabled.md](./endpoints/v1/settings/update_check_enabled.md) |
| `/v1/settings/volume` | `GET`, `POST` | `settings` | [settings/volume.md](./endpoints/v1/settings/volume.md) |

## Add your own model

A model is delivered by a **backend**: an out-of-tree program the daemon
loads at runtime. Backends implement one HTTP-shaped contract, packaged in
one of two transports:

| Transport      | Use for                              | Isolation                                   |
|----------------|--------------------------------------|---------------------------------------------|
| WASM component | Cloud / API providers, light CPU     | wasmtime sandbox; network egress allowlisted |
| Native subprocess | Local, GPU-accelerated models     | Network-isolated; systemd + seccomp hardened |

To make a model available in Super TTS:

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
