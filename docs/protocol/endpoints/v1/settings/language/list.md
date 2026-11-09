# `GET /settings/language/list`

The tags [`POST /settings/language`](../language.md#post-settingslanguage)
accepts, plus the reserved `auto`. This is what fills the global Primary
Language picker, for both features.

The list is fixed: it is the same on every machine and does not change as
backends are installed or removed. It is not the languages the installed
models declare. The tags are **region-qualified** on purpose (`en-US`,
`es-MX`, `pt-BR`), with one exception: Chinese uses the script subtags
`zh-Hans` and `zh-Hant`, since script, not region, is what tells Simplified
from Traditional. A region is a country code, or a UN M.49 code for a
multi-country region (`es-419`).

Region-qualified is what lets one global setting work across backends that
disagree about granularity. A model may declare `en` or `en-US` — that is the
backend's choice — and the daemon narrows the global tag to whichever the
model actually serves:

1. The tag itself, when the model declares it.
2. Otherwise the base language, when the model declares that (`es-MX` reaches
   a model declaring `es`).
3. Otherwise nothing: the model falls back to its own `primary_language`.

That rule is the daemon's, and a client cannot infer it. Fill the picker from
this list rather than from a BCP-47 catalog of your own: a client that sends
`en` here is refused, because `en` is not on the list.

A tag for one particular model belongs on that model, at
[`/stt/pipeline/{stage}/model/{model}/language`](../../pipeline/language.md) or
[`/tts/pipeline/{stage}/model/{model}/language`](../../tts/pipeline/language.md),
whose own lists are that model's `supported_languages`. Use this endpoint for
the Settings page's language row, and those for a model card's override.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `GET /settings/language/list`

**Request:**

```http
GET /settings/language/list HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":              "success",
  "available_languages": ["auto", "af-ZA", "ar-EG", "ar-SA", "bn-BD", …, "cy-GB"],
  "message":             "Global languages listed"
}
```

| Field                 | Type     | Notes                                                                 |
|-----------------------|----------|-----------------------------------------------------------------------|
| `available_languages` | string[] | `auto` first, then the fixed list below, in that order. One of these, spelled exactly, is what `POST /settings/language` accepts. |

The list after `auto`:

```
af-ZA ar-EG ar-SA bn-BD bg-BG ca-ES zh-Hans zh-Hant hr-HR cs-CZ
da-DK nl-NL en-AU en-CA en-IN en-GB en-US et-EE fi-FI fr-BE
fr-CA fr-FR de-AT de-DE de-CH el-GR he-IL hi-IN hu-HU id-ID
it-IT ja-JP ko-KR ms-MY no-NO fa-IR pl-PL pt-BR pt-PT ro-RO
ru-RU sk-SK es-419 es-MX es-ES es-US sw-KE sv-SE tl-PH ta-IN
te-IN th-TH tr-TR uk-UA ur-PK vi-VN cy-GB
```

Read it from the endpoint rather than copying it: the daemon's copy is the one
`POST /settings/language` checks against.

**Errors:**

| HTTP | `message`         | Meaning                                 |
|------|-------------------|-----------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed` |
| 403  | `scope_denied`    | Token lacks the `settings` scope        |
