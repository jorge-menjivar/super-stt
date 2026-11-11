# `/settings/read_method`

Read and set where [`POST /tts/speak/read`](../tts/speak/read.md) takes the text
it speaks from. That endpoint is what a read-aloud keyboard shortcut runs, and
this is text to speech's counterpart to
[`/settings/write_method`](./write_method.md): that one decides how a
transcription reaches the screen, and this one how text on the screen reaches
the voice.

| Method      | Reads                                                                 |
|-------------|-----------------------------------------------------------------------|
| `selection` | The highlighted text, Wayland's primary selection. The default.       |
| `clipboard` | The text last copied.                                                 |

`selection` reads whatever was last highlighted, in any app, with nothing
copied. Most apps keep that text after the highlight is cleared, so it is the
last text highlighted rather than what is highlighted now. `clipboard` reads
only what was copied on purpose.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `settings` scope get `403 scope_denied`.

## `POST /settings/read_method`

**Request:**

```http
POST /settings/read_method HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
Content-Type: application/json

{
  "method": "clipboard"
}
```

| Field    | Type   | Required | Notes                         |
|----------|--------|----------|-------------------------------|
| `method` | string | yes      | `selection` or `clipboard`    |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":      "success",
  "read_method": "clipboard"
}
```

**Errors:**

| HTTP | `message`             | Meaning                                       |
|------|-----------------------|-----------------------------------------------|
| 400  | `Invalid read method` | `method` wasn't `selection` or `clipboard`    |
| 401  | `invalid_session`     | Token unknown / expired / `exe_changed`       |
| 403  | `scope_denied`        | Token lacks the `settings` scope              |

An unknown `method` is **rejected** (`400`) and leaves the stored setting
unchanged. The write is not silently coerced to the default.

## `GET /settings/read_method`

**Request:**

```http
GET /settings/read_method HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":      "success",
  "read_method": "selection"
}
```

**Errors:**

| HTTP | `message`         | Meaning                                  |
|------|-------------------|------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`  |
| 403  | `scope_denied`    | Token lacks the `settings` scope         |
