# `GET /settings/language/list`

The BCP-47 tags [`POST /settings/language`](../language.md#post-settingslanguage)
accepts, plus the reserved `auto`. This is what fills the global
Primary Language picker.

The list is the **union** of the `supported_languages` of every model installed
on this host, not a general BCP-47 catalog. A tag no installed model can speak
is a setting with no effect: every model would fall through to its own
`primary_language`, and the user would be left looking at a language selector
that changed nothing. Offering only what something can actually speak makes the
setting mean what it looks like it means.

It follows that this list **grows and shrinks as backends are installed and
removed**. A client re-reads it after
[`POST /registry/backend/install`](../../registry/install.md) or
[`DELETE /backend/{backend_id}`](../../backends.md#delete-backendbackend_id)
rather than caching it for the session. A stored global language whose last
supporting model has been uninstalled is left alone — it is still the user's
stated preference, and reinstalling the backend should restore the behavior
they had — so a client rendering a picker tolerates a current value that is not
on the list rather than silently rewriting it.

This is the global counterpart of
[`GET /pipeline/{stage}/model/{model}/language/list`](../../pipeline/language.md#get-pipelinestagemodelmodellanguagelist),
which answers the same question for one model. Use this one for the Settings
page's language row, and that one for a model card's override.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `GET /settings/language/list`

**Request:**

```http
GET /settings/language/list HTTP/1.1
Host: tts.local
Authorization: Bearer tts_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status": "success",
  "available_languages": ["auto", "en", "es-419", "es-ES", "fr", "ja", "zh"]
}
```

| Field                 | Type     | Notes                                                                 |
|-----------------------|----------|-----------------------------------------------------------------------|
| `available_languages` | string[] | `auto` first, then the union of every installed model's `supported_languages`, deduplicated and sorted. One of these is what `POST /settings/language` accepts. |

`auto` is always present, even with no backends installed: it means "let each
model decide", which is a coherent answer whatever is installed. A daemon with
no backends therefore answers `["auto"]` rather than an empty list — the
setting still has one meaningful value, and a client renders the row instead of
hiding it.

**Errors:**

| HTTP | `message`         | Meaning                                 |
|------|-------------------|-----------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed` |
| 403  | `scope_denied`    | Token lacks the `settings` scope        |
