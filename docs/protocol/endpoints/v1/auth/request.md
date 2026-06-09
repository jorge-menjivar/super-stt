# `POST /auth/request`

Trigger the consent popup and mint a fresh session token. This is
the only endpoint a new client can call before it has a token.

See [auth.md](../../auth.md) for the broader handshake design and
the full list of `auth_denied` reasons.

## Auth

- **Required scope:** none. This is the bootstrap endpoint.
- `Authorization` header is ignored if present.

## `POST /auth/request`

Spawn the consent popup. The user is shown the requesting binary's
identity, the scopes being requested, and an Allow / Deny choice.
The call blocks until the user chooses, dismisses, or the popup
times out (60 s default).

**Request:**

```http
POST /auth/request HTTP/1.1
Host: stt.local
Content-Type: application/json

{
  "app_name": "Super STT Settings App",
  "scopes":   ["settings", "status", "daemon_status"],
  "version":  "0.10.0"
}
```

| Field      | Type     | Required | Notes                                                                       |
|------------|----------|----------|-----------------------------------------------------------------------------|
| `app_name` | string   | yes      | Declared (untrusted) name displayed to the user in the popup                |
| `scopes`   | string[] | yes      | Non-empty array of known scope names (see [auth.md](../../auth.md))         |
| `version`  | string   | no       | Free-form version string for the consent popup; shown for UX context only   |

**Response (200, on Allow):**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "status":        "success",
  "session_token": "stt_…64hex…",
  "scopes":        ["settings", "status", "daemon_status"],
  "expires_at":    "2026-06-04T12:34:56Z"
}
```

| Field           | Type   | Notes                                                                  |
|-----------------|--------|------------------------------------------------------------------------|
| `session_token` | string   | Bearer token. Carry it as `Authorization: Bearer <token>` from now on. |
| `scopes`        | string[] | Echo of the granted scope set, confirming what the token actually grants |
| `expires_at`    | string   | ISO 8601 UTC timestamp, 30 days after issue                            |

**Response (403, on failure):**

```http
HTTP/1.1 403 Forbidden
Content-Type: application/json

{
  "status":  "error",
  "message": "auth_denied",
  "data":    { "reason": "user_denied" }
}
```

| `data.reason`         | Meaning                                                                                  | Client should                                                          |
|-----------------------|------------------------------------------------------------------------------------------|------------------------------------------------------------------------|
| `user_denied`         | User clicked Deny in the popup.                                                          | Do **not** auto-retry. Offer an explicit Retry button.                 |
| `user_denied_cached`  | This identity was already denied; popup wasn't shown. Sticky until the daemon restarts.  | Same — surface a hint that restarting the daemon clears the deny.       |
| `user_dismissed`      | User closed the popup without choosing, or it timed out.                                 | Recoverable. Re-prompt when the user takes an action that requires it. |
| `popup_failed`        | Consent popup couldn't be shown (no display server, no Wayland session, etc.).           | Fall back to read-only mode if possible.                               |
| `invalid_scope`       | `scopes` was empty, missing, or contained a name that isn't a known scope.               | Fix the request body.                                                  |
| `throttled`           | Too many `/auth/request` calls from this peer in a short window.                         | Back off and retry later.                                              |
| `uid_mismatch`        | Peer UID doesn't match the daemon's effective UID. The daemon only serves its own user; cross-user requests under a shared group are rejected before the popup. | Run as the same user as the daemon (typically the desktop user).         |

The token returned by this endpoint should be persisted in a
**secure** store (the platform keyring, libsecret/KWallet, the OS
credential manager) — plaintext on disk would let any other process
running as the same user impersonate the app.
