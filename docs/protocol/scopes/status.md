# status scope

> Scope: **status** (read the daemon's current operational state — loaded model
> and device — through [`GET /status`](../endpoints/v1/status.md); no
> configuration access, no recording control and no ability to make it speak).

The `status` scope is the smallest read grant. It exposes a single endpoint and
leaks nothing about other apps' activity. It pairs naturally with
[`transcribe`](./transcribe.md): a client that drives recordings reads
`busy` here to implement toggle behavior. It pairs the same way with
[`speak`](./speak.md): a client that drives speech reads text to speech's
`busy` for a speak/silence toggle.

For the richer operator views — in-flight model switches, GPU memory, device
introspection — use the [`settings`](./settings.md) scope's
[`GET /stt/pipeline/1`](../endpoints/v1/pipeline/stage.md),
[`GET /tts/pipeline/1`](../endpoints/v1/tts/pipeline/stage.md),
[`GET /tts/pipeline/1/model`](../endpoints/v1/tts/pipeline/model.md) and the
`device` endpoints under each
([speech to text](../endpoints/v1/pipeline/device.md#get-sttpipelinestagemodelmodeldevice),
[text to speech](../endpoints/v1/tts/pipeline/device.md#get-ttspipelinestagemodelmodeldevice)).

## Endpoint reference

| Endpoint                                 | Methods | Notes                                 |
|------------------------------------------|---------|---------------------------------------|
| [`/status`](../endpoints/v1/status.md)   | GET     | Current daemon state (model + device) |

[`/auth/request`](../endpoints/v1/auth/request.md),
[`/auth/status`](../endpoints/v1/auth/status.md), and
[`/ping`](../endpoints/v1/ping.md) require only a valid token, not the `status`
scope. Authentication is shared across all scopes — see [auth.md](../auth.md).
