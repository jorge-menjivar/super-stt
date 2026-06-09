# status scope

> Scope: **status** (read the daemon's current operational state — loaded model
> and device — through [`GET /status`](../endpoints/v1/status.md); no
> configuration access and no recording control).

The `status` scope is the smallest read grant. It exposes a single endpoint and
leaks nothing about other apps' activity. It pairs naturally with
[`transcribe`](./transcribe.md): a client that drives recordings reads
`busy` here to implement toggle behavior.

For the richer operator views — in-flight model switches, GPU memory, device
introspection — use the [`settings`](./settings.md) scope's
[`GET /active_model`](../endpoints/v1/active_model.md) and
[`GET /active_device`](../endpoints/v1/active_device.md).

## Endpoint reference

| Endpoint                                 | Methods | Notes                                 |
|------------------------------------------|---------|---------------------------------------|
| [`/status`](../endpoints/v1/status.md)   | GET     | Current daemon state (model + device) |

[`/auth/request`](../endpoints/v1/auth/request.md),
[`/auth/status`](../endpoints/v1/auth/status.md), and
[`/ping`](../endpoints/v1/ping.md) require only a valid token, not the `status`
scope. Authentication is shared across all scopes — see [auth.md](../auth.md).
