# `POST /tts/speak/read`

Read text from the screen and speak it, like [`POST /tts/speak`](../speak.md).
Where it reads from is the user's read method,
[`/settings/read_method`](../../settings/read_method.md): `selection`, the
highlighted text, or `clipboard`, the text last copied. The request carries no
text. Which text is read is the user's choice, like the voice.

This is what a read-aloud keyboard shortcut runs. `voice speak --read` runs it,
and the installer binds that to Super+Shift+Space on COSMIC.

**Every refusal is also a desktop notification**, unless
[`/settings/notification_method`](../../settings/notification_method.md) is
`off`. Nobody is looking at a shortcut's response, so the user hears nothing and
sees why: nothing to read, too much to read, the text could not be read, and
every failure `POST /tts/speak` notifies on its own (no model loaded, the audio
device, the backend).

**Not a toggle.** Like `POST /tts/speak`, it interrupts whatever is playing. A
shortcut that should fall silent on a second press reads `busy` on
[`GET /status`](../../status.md) first, and calls
[`POST /tts/speak/stop`](./stop.md) when it is `true`. `voice speak --read` does
exactly that.

The text is read through Wayland's data-control protocol
(`ext_data_control_v1`, or `zwlr_data_control_v1`), so the daemon's session needs
a compositor that offers it. COSMIC offers both. Elsewhere every request fails
with `500`.

## Auth

- **Required scope:** `speak`.
- `Authorization: Bearer <session_token>` is required.
- Tokens without the `speak` scope get `403 scope_denied`.

## Request

```http
POST /tts/speak/read HTTP/1.1
Host: voice.local
Authorization: Bearer …64hex…
```

No request body.

## Response (202)

```http
HTTP/1.1 202 Accepted
Content-Type: application/json

{
  "status": "success",
  "utterance_id": "utt_9f2c1e40"
}
```

The same as [`POST /tts/speak`](../speak.md#response-202): the text was read,
and the utterance is queued and playing out.

## Errors

| HTTP | `error_code`       | Meaning                                                                      |
|------|--------------------|------------------------------------------------------------------------------|
| 400  | `invalid_value`    | More than one utterance may be (`text_too_long`, over 50 000 characters)     |
| 401  | —                  | `invalid_session`: token unknown / expired / `exe_changed`                   |
| 403  | —                  | `scope_denied`: token lacks the `speak` scope                                |
| 409  | `nothing_to_read`  | Nothing selected, or nothing copied, or only something that is not text      |
| 409  | `model_not_loaded` | No text to speech model is loaded                                            |
| 429  | —                  | `rate_limited`: too many requests; back off                                  |
| 500  | `internal`         | The text could not be read (`could_not_read: …`), or speaking failed         |
