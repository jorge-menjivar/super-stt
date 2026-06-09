# daemon_status scope

> Scope: **daemon_status** (subscribe, read-only, to model/device transition
> status, model-download progress, and backend-registry install progress on
> [`GET /events`](../endpoints/v1/events.md)).

A Settings UI uses this scope to drive progress bars without polling: it sees a
model switch move through `loading_model` → download ticks → `ready`, and it sees
backend-registry installs report progress. The scope reveals nothing about audio
or transcription text — only daemon configuration/lifecycle state.

It is usually requested alongside [`settings`](./settings.md) (which performs the
mutations these events report on) and the recording/visualization scopes a
Settings UI also shows. Scopes are composable — see [auth.md](../auth.md).

## Topics

| Topic                   | Carries                                                                                                                                                                          |
|-------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `daemon_status_changed` | Heterogeneous; the `status` field discriminates (`loading_model`, `ready`, `model_switched`, `switching_device`, `device_switch_error`, …). Always includes `timestamp`.         |
| `download_progress`     | Per-file model-download tick — `{ model_name, current_file, file_index, total_files, percentage, status, eta_seconds, timestamp, … }`.                                            |
| `registry_install`      | Backend-registry install / refresh progress — a serialized registry event (`install.progress` / `install.completed` / `install.failed` / `refresh.completed` / `refresh.failed`). |

Full payload semantics and the SSE framing rules live on
[`/events`](../endpoints/v1/events.md). A subscription that requests a topic
outside the token's scopes fails the whole stream with `403 scope_denied` before
it opens.

## Errors

| HTTP | `message`             | Meaning                                                                                                          |
|------|-----------------------|----------------------------------------------------------------------------------------------------------------|
| 401  | `invalid_session`     | Token expired, unknown, or binary identity changed; re-issue [`/auth/request`](../endpoints/v1/auth/request.md). |
| 403  | `scope_denied`        | Requested a topic this token's scopes don't grant.                                                              |
| 503  | `connection_rejected` | Server refused the connection.                                                                                  |
