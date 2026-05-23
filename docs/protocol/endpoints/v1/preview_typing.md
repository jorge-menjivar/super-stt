# `/preview_typing`

Read and toggle the preview-typing default. When enabled,
intermediate transcription text is typed into the focused window as
it's being recognized — letting the user see partial results
without waiting for the final transcript. Per-request overrides via
[`POST /transcribe`](./transcribe.md)'s `data.preview` take
precedence over the default set here.

This flag is independent of `write_mode` on `/transcribe`: if
`write_mode` is `false` on a request, no typing happens at all,
regardless of `preview_typing`.

## Auth

- **Required scope:** `settings`.
- `Authorization: Bearer <session_token>` is required.
- `client` / `widget` tokens get `403 scope_denied`.

## `POST /preview_typing`

**Request:**

```http
POST /preview_typing HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
Content-Type: application/json

{
  "enabled": true
}
```

| Field     | Type | Required | Notes                                |
|-----------|------|----------|--------------------------------------|
| `enabled` | bool | yes      | `true` enables preview typing       |

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":                 "success",
  "preview_typing_enabled": true
}
```

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Not a `settings` token                                        |

## `GET /preview_typing`

**Request:**

```http
GET /preview_typing HTTP/1.1
Host: stt.local
Authorization: Bearer stt_…64hex…
```

**Response (200):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":                 "success",
  "preview_typing_enabled": false
}
```

**Errors:**

| HTTP | `message`         | Meaning                                                       |
|------|-------------------|---------------------------------------------------------------|
| 401  | `invalid_session` | Token unknown / expired / `exe_changed`                       |
| 403  | `scope_denied`    | Not a `settings` token                                        |
