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
| `daemon_status_changed` | A discriminated union keyed on `status` (schema per variant [below](#daemon_status_changed-variants)). Every event also carries `timestamp` (RFC 3339).                          |
| `download_progress`     | Per-file model-download tick — `{ model_name, current_file, file_index, total_files, percentage, status, eta_seconds, timestamp, error?, … }`. `error` carries the failure detail on the terminal `status` = `"error"` tick (omitted otherwise).                                            |
| `registry_install`      | Backend-registry install / refresh progress — a serialized registry event (`install.progress` / `install.completed` / `install.failed` / `refresh.completed` / `refresh.failed`). |

### `daemon_status_changed` variants

Each event is a JSON object whose `status` field selects the variant. Every
event also carries a `timestamp` (RFC 3339 string). Clients switch on `status`
and read only the fields for that variant; unknown variants should be ignored.

| `status`                   | Fields (besides `status` + `timestamp`)                                                     |
|----------------------------|---------------------------------------------------------------------------------------------|
| `loading_model`            | `new_model` (string)                                                                        |
| `loading_model_for_device` | `model` (string), `target_device` (string)                                                  |
| `model_switched`           | `model_name` (string), `provider` (string), `source` (string), `actual_device` (string)     |
| `ready`                    | `model_loaded` (bool); optional `model_name`, `actual_device`, `preferred_device` (strings)  |
| `switching_device`         | `from_device` (string), `target_device` (string), `model` (string)                          |
| `device_switch_error`      | `error` (string), `failed_device` (string), `model` (string)                                |
| `active_backend_changed`   | `source` (string, or `null` when the active backend was cleared)                            |
| `settings_changed`         | `setting` (string — the name of what changed, e.g. `"language"`)                            |

The destination device is named `target_device` on both `loading_model_for_device`
and `switching_device` (previously `switching_device` used `to_device`). The
daemon builds these from a single typed `DaemonStatusEvent` enum, so producer and
consumer keys cannot drift.

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
