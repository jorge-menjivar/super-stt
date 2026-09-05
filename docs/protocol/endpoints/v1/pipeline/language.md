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

The symmetry goes one level further. What is *set* and what may *be set* are
separate endpoints, as they are for the device:

| | is set | may be set |
|---|---|---|
| device   | [`/device`](./device.md#get-pipelinestagemodelmodeldevice)   | [`/device/list`](./device.md#get-pipelinestagemodelmodeldevicelist) |
| language | `/language`                                                  | [`/language/list`](#get-pipelinestagemodelmodellanguagelist)        |

`{model}` is the model name as
[`GET /pipeline/{stage}/model/list`](./model-list.md) spells it; names contain
only alphanumerics and hyphens, so the segment is used as-is.

```
/pipeline/1/model/whisper-large-v3/language
```

Every stage answers it. A post-processor is monolingual and says so in
`multilingual` — a real answer rather than an error, since the point of
addressing stages by position is that they answer the same verbs.

> **Moved from `/backends/{backend_id}/models/{model}/language`.** The model's
> backend has to be filling a stage now, where the old spelling could reach any
> installed model. That costs nothing real: a language control is only ever
> shown on a stage's card, so the backend is selected by the time anyone asks.
> A model that is *selected but not yet loaded* still resolves — that is how a
> card shows its language before Load. An empty stage answers
> `400 invalid_backend`, the same precondition [device](./device.md) has always
> had.

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
    "primary":   "en"             // the model's primary_language (the fallback)
  }
}
```

For a non-multilingual model: `"multilingual": false`, `"effective": null`,
`"override": null`, `"primary": "en"`, `"source": "default"`.

The tags this model accepts are **not** here — they are
[`GET .../language/list`](#get-pipelinestagemodelmodellanguagelist), the way a
model's device list is not part of its device. This endpoint answers what is
set; that one answers what may be set, and only one of them changes when the
user picks a language.

## `GET /pipeline/{stage}/model/{model}/language/list`

What `POST` will accept for this model: the tags it serves, plus the reserved
`auto` for letting it detect the language itself.

**Request:**

```http
GET /pipeline/1/model/whisper-large-v3/language/list HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```json
{
  "status": "success",
  "available_languages": ["auto", "en", "es-419", "es-ES", "fr"]
}
```

Fill a language picker from this rather than from a general BCP-47 list: a tag
the model does not serve is refused, and offering one is an error the user only
discovers by choosing it.

**Empty for a monolingual model**, which has nothing to choose however many tags
its manifest lists — the same shape
[`/device/list`](./device.md#get-pipelinestagemodelmodeldevicelist) answers with
for a model that runs remotely. A client hides the control on an empty list
rather than special-casing a status.

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
| `language` | string | yes      | One of the tags [`/language/list`](#get-pipelinestagemodelmodellanguagelist) offers. To clear, `DELETE`. |

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
| 400  | `unsupported_language` | `language` is not one [`/language/list`](#get-pipelinestagemodelmodellanguagelist) offers — a tag the model does not serve, or any tag at all for a monolingual model |
| 401  | `invalid_session`      | Token unknown / expired / `exe_changed`                                                                      |
| 403  | `scope_denied`         | Token lacks the `settings` scope                                                                             |
| 404  | `unknown_backend`      | No installed backend has that `source`                                                                       |
| 404  | `unknown_model`        | `{model}` is not served by that backend                                                                      |
