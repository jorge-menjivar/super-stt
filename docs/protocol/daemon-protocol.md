# Daemon Protocol

One picture of the daemon: where a request enters, what carries it, and which
route group answers it. Everything here is specified in detail elsewhere —
[transport.md](./transport.md) for the wire shape, [auth.md](./auth.md) for
the handshake and scopes, [endpoints/](./endpoints/) for each route, and
[backend/contract.md](./backend/contract.md) for the far side of the pipeline.
This page exists so that a reader can see how those fit together before
reading any of them.

The HTTP API is the whole client surface. The D-Bus name beside it answers one
question — is the daemon up — and is deliberately not a second way in.

```mermaid
graph TD
    subgraph Daemon ["Super Voice daemon"]
        D_Auth[Auth Service]
        D_API[HTTP API /v1]
        D_SSE[Event Stream]
        D_DBus[D-Bus Interface]
        D_Core[Core Logic / STT and TTS Pipelines]
    end

    %% Authentication Flow
    D_Auth -- "Consent Popup / Token" --> D_API

    %% Command Flow
    D_API -- "DaemonRequest" --> D_Core
    D_Core -- "DaemonResponse" --> D_API

    %% Event Flow
    D_Core -- "Events (Recording, STT, Playback, Synthesis, Progress, Audio Levels)" --> D_SSE
    D_Core -- "Liveness (ping, no signals)" --> D_DBus

    %% HTTP Route Groups
    D_API --> R_Auth["/auth"]
    D_API --> R_Transcribe["/stt/transcribe"]
    D_API --> R_Context["/stt/context"]
    D_API --> R_Speak["/tts/speak"]
    D_API --> R_Voice["/tts/voice"]
    D_API --> R_Pipeline["/stt/pipeline, /tts/pipeline"]
    D_API --> R_Backend["/backend"]
    D_API --> R_Registry["/registry"]
    D_API --> R_Settings["/settings"]
    D_API --> R_Top["top level"]

    %% Auth Endpoints
    R_Auth -.-> E1["POST /request"]
    R_Auth -.-> E2["GET /status"]

    %% Transcribe Endpoints
    R_Transcribe -.-> E3["POST /"]
    R_Transcribe -.-> E4["POST /stop"]
    R_Transcribe -.-> E5["GET /realtime (WebSocket)"]

    %% Dictation context Endpoints
    R_Context -.-> E6["GET /list"]
    R_Context -.-> E7["GET/POST/DELETE /{id}"]
    R_Context -.-> E8["GET/POST /active"]

    %% Speak Endpoints
    R_Speak -.-> E9["POST /"]
    R_Speak -.-> E10["POST /stop"]
    R_Speak -.-> E11["GET /stream (WebSocket)"]

    %% Voice Endpoints — the cloned-voice library
    R_Voice -.-> E12["GET /list"]
    R_Voice -.-> E13["POST /"]
    R_Voice -.-> E14["GET/PATCH/DELETE /{id}"]
    R_Voice -.-> E15["GET /{id}/audio"]

    %% Pipeline Endpoints — speech to text has stages 1 and 2, text to speech stage 1
    R_Pipeline -.-> E16["GET /"]
    R_Pipeline -.-> E17["GET/POST/DELETE /{stage}"]
    R_Pipeline -.-> E18["GET /{stage}/backend/list<br/>GET /{stage}/device/list"]
    R_Pipeline -.-> E19["GET/POST/DELETE /{stage}/model"]
    R_Pipeline -.-> E20["GET /{stage}/model/list"]
    R_Pipeline -.-> E21["POST /{stage}/model/cancel<br/>POST /{stage}/model/reload"]
    R_Pipeline -.-> E22["GET/POST /{stage}/model/{model}/device<br/>GET /{stage}/model/{model}/device/list"]
    R_Pipeline -.-> E23["GET/POST/DELETE /{stage}/model/{model}/language<br/>GET /{stage}/model/{model}/language/list"]
    R_Pipeline -.-> E24["GET/POST/DELETE /{stage}/model/{model}/voice<br/>GET /{stage}/model/{model}/voice/list<br/>(text to speech only)"]

    %% Backend Endpoints
    R_Backend -.-> E25["GET /list"]
    R_Backend -.-> E26["DELETE /{backend_id}"]
    R_Backend -.-> E27["GET /{backend_id}/option/list<br/>GET/POST/DELETE /{backend_id}/option/{name}"]
    R_Backend -.-> E28["GET /{backend_id}/secret/list<br/>GET/POST/DELETE /{backend_id}/secret/{name}"]
    R_Backend -.-> E29["GET/POST/DELETE /stt/backend/{backend_id}/context"]

    %% Registry Endpoints
    R_Registry -.-> E30["GET /backend/list"]
    R_Registry -.-> E31["POST /backend/preview"]
    R_Registry -.-> E32["POST /backend/install"]
    R_Registry -.-> E33["POST /backend/refresh"]
    R_Registry -.-> E34["POST /backend/update"]

    %% Settings Endpoints
    R_Settings -.-> E35["GET/POST /volume"]
    R_Settings -.-> E36["GET/POST /audio_theme<br/>GET /audio_theme/list<br/>POST /audio_theme/test"]
    R_Settings -.-> E37["GET/POST/DELETE /language<br/>GET /language/list"]
    R_Settings -.-> E38["GET/POST /write_method<br/>POST /write_method/test"]
    R_Settings -.-> E39["GET/POST /notification_method<br/>GET/POST /idle_unload"]
    R_Settings -.-> E40["GET/POST /preview_typing<br/>GET/POST /recording_stop_mode"]
    R_Settings -.-> E41["GET/POST /update_check_enabled<br/>GET/POST /update_beta_optin"]

    %% Top-level Endpoints
    R_Top -.-> E42["GET /ping"]
    R_Top -.-> E43["GET /status"]
    R_Top -.-> E44["GET /gpu_info"]
    R_Top -.-> E45["GET /update<br/>POST /update/check"]
    R_Top -.-> E46["GET /events?topics=..."]
    E46 --> D_SSE

    %% Backend transport
    D_Core -- "POST /v1/load, /v1/transcribe, /v1/process, /v1/synthesize, /v1/cancel" --> B_WASM["WASM component<br/>(in-process)"]
    D_Core -- "same routes over a Unix socket" --> B_Proc["Subprocess backend"]

    %% D-Bus Interface
    D_DBus --> DB_Int["com.github.jorge_menjivar.SuperVoice1"]
    DB_Int --> DB_Obj["/com/github/jorge_menjivar/SuperVoice"]
    DB_Obj -.-> DB_M1["ping()"]
    DB_Obj -.-> DB_M2["get_status()"]
```

## Reading the graph

**Every path is versioned.** The route groups above are drawn without the
`/v1` prefix every request carries: `/tts/speak` is `POST /v1/tts/speak`. The
prefix is part of the client protocol and is not the same number as a
backend's [contract generation](./backend/config.md#contract-generations).

**A feature's routes carry its name.** Speech to text's routes sit under
`/stt/` and text to speech's under `/tts/`. What both features share — auth,
events, status, backends, the registry, settings, updates — sits directly
under `/v1/`.

**The two pipelines differ in length.** `/stt/pipeline` has two stages, 1
transcription and 2 post-processing. `/tts/pipeline` has one, 1 synthesis. The
path is parameterized because the shape is worth keeping — a stage owns its
backend, its model, and that model's device and language — but a position a
pipeline does not have answers `404 unknown_stage`, naming the stages that do
exist. `GET /v1/tts/pipeline/2` is one such request.

**`/settings` is not the same as the `settings` scope.** Everything the user
can set lives under `/v1/settings/`. `/backend`, the pipelines, `/registry`,
`/gpu_info` and `/update` are all reached with a `settings` token but are not
settings — they select and manage what runs, rather than storing a
preference. See [scopes/settings.md](./scopes/settings.md).

**The event stream is one connection, not one per topic.** A client asks for
the topics it wants on a single `GET /v1/events` subscription, and a topic
outside the token's scopes fails the whole stream with `403 scope_denied`
before it opens — rather than being quietly dropped from a stream that then
looks healthy and never delivers. See
[endpoints/v1/events.md](./endpoints/v1/events.md).

## Why D-Bus carries so little

The daemon owns a session-bus name and serves two methods on it: `ping()` and
a coarse `get_status()` that reports only that something is serving the name
and which version it is.

It does not publish signals. The daemon has recording, transcription,
playback and audio-level events, and they are exactly what a bus signal would
be good for — but a session-bus signal is readable by every process on the
bus, while the same information on `GET /v1/events` costs a
[`recording_events`](./scopes/recording_events.md),
[`global_transcriptions`](./scopes/global_transcriptions.md),
[`playback_events`](./scopes/playback_events.md) or
[`audio_visualization`](./scopes/audio_visualization.md) grant the user has to
approve. Publishing both would make that consent prompt a formality: anything
denied there could listen on the bus instead.

What is left on the bus is what carries nothing the user would want gated —
the well-known name, so "is the daemon running?" is answerable without opening
the socket, and a liveness ping. Whether the daemon is recording or
*speaking*, and which models it loaded, stay behind the `status` scope on
[`GET /v1/status`](./endpoints/v1/status.md).

The bus service is Linux only. macOS has no session bus, so it is not served
there.
