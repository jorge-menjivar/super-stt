# `/pipeline/{stage}/model/{model}/language`

Read and set a model's **language override**, and read the daemon's resolved
effective language. The override is one of the model's `supported_languages`,
the reserved `auto`, or absent (Automatic — inherits the global
[`/settings/language`](../settings/language.md), else the model's
`primary_language`). It is stored per `(source, model)` and survives model
switches. Only multilingual models accept an override.

Addressed through the stage that runs the model, exactly like its
[device](./device.md), and for the same reason: both are per-model preferences,
and the stage is what resolves a bare model name against the backend filling
it. Two preferences of the same shape addressed two different ways is something
a client author has to memorise rather than infer.

`{model}` is the model name as
[`GET /pipeline/{stage}/model/list`](./model-list.md) spells it; names contain
only alphanumerics and hyphens, so the segment is used as-is.

```
/pipeline/1/model/whisper-large-v3/language
```

Every stage answers it. A post-processor is monolingual and says so in
`multilingual` — a real answer rather than an error, since the point of
addressing stages by position is that they answer the same verbs.

> **Moved from `/backends/{source}/models/{model}/language`.** One consequence:
> the model's backend must now be *selected into a stage*. A model belonging to
> an installed but unselected backend has no path to it, where the
> `{source}`-addressed spelling could reach any installed model. That is the
> same precondition [device](./device.md) has always had, and `400
> invalid_backend` is the answer when the stage is empty.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required on every request.
- A token without the `settings` scope gets `403 scope_denied`.

## `GET /pipeline/{stage}/model/{model}/language`

Returns the daemon's full resolution for the named model.

**Request:**

```http
GET /pipeline/1/model/whisper-large-v3/language HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```jsonc
{
  "status": "success",
  "language": {
    "multilingual": true,
    "source":    "override",      // "override" | "global" | "default"
    "effective": "es-419",        // the value sent to the backend; null = omitted (model primary)
    "override":  "es-419",        // the stored per-model value, or null
    "primary":   "en",            // the model's primary_language (the fallback)
    "supported": ["en", "es-419", "es-ES", "fr"]
  }
}
```

For a non-multilingual model: `"multilingual": false`, `"supported": ["en"]`,
`"effective": null`, `"override": null`, `"primary": "en"`, `"source": "default"`.

## `POST /pipeline/{stage}/model/{model}/language`

**Request:**

```http
POST /pipeline/1/model/whisper-large-v3/language HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{ "language": "es-419" }
```

| Field      | Type   | Required | Notes                                                              |
|------------|--------|----------|--------------------------------------------------------------------|
| `language` | string | yes      | One of the model's `supported_languages`, or `auto`. To clear, `DELETE`. |

**Response (200):** the resolution block (as `GET`).

## `DELETE /pipeline/{stage}/model/{model}/language`

Clear the override (back to Automatic).

**Request:**

```http
DELETE /pipeline/1/model/whisper-large-v3/language HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):** the resolution block (as `GET`).

## Errors

| HTTP | `message`              | Meaning                                                                                                      |
|------|------------------------|--------------------------------------------------------------------------------------------------------------|
| 400  | `unsupported_language` | `language` is not in the model's `supported_languages` (and isn't `auto`), or the model is not multilingual |
| 401  | `invalid_session`      | Token unknown / expired / `exe_changed`                                                                      |
| 403  | `scope_denied`         | Token lacks the `settings` scope                                                                             |
| 404  | `unknown_backend`      | No installed backend has that `source`                                                                       |
| 404  | `unknown_model`        | `{model}` is not served by that backend                                                                      |
