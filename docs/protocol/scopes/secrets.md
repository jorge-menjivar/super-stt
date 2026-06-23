# secrets scope

> Scope: **secrets** (store, check, and clear backend credentials — write-only;
> values are never read back).

The `secrets` scope is the credential-management surface. A `secrets` token can
store a backend's declared secrets (API keys and the like), check whether each
is configured, and clear them. It **cannot read a secret value back** — no
endpoint under this scope returns a stored value. The only reader of a secret's
value is the daemon's own model-load path, which injects it as an
`x-stt-secret-<name>` header to the backend (see
[contract.md](../backend/contract.md#request-headers)); it is never serialized
into a client response.

The scope is deliberately separate from [`settings`](./settings.md). Managing a
backend's non-sensitive [options](../endpoints/v1/backends/options.md) requires
only `settings`; writing its credentials requires `secrets`. Scopes never imply
one another, so a client is granted credential-write access **only** when the
user has approved it explicitly, and the consent prompt names it as a distinct
permission. A Settings UI that configures both requests them together, e.g.
`["settings", "secrets", "status", "daemon_status"]`.

Transport and framing are described in [transport.md](../transport.md); how
scopes compose and how a token is obtained are in [auth.md](../auth.md).

## Why write-only

A secret is the one piece of backend configuration whose value must never flow
back to a client. Even a client the user has approved is given only the ability
to *set* and *clear* a credential and to learn that one *exists* — never to
*retrieve* it. This keeps a stored key from being exfiltrated by any client
that holds (or coerces) a token, and it is the property that makes the same
endpoints safe to expose to a future web-origin client, where the trust in the
caller is weaker than a peer-verified local binary (see
[auth.md — TCP-bound clients](../auth.md#tcp-bound-clients)).

## Endpoint reference

| Endpoint                                                                              | Methods           | Notes                                                                 |
|---------------------------------------------------------------------------------------|-------------------|-----------------------------------------------------------------------|
| [`/backends/{source}/secrets/list`](../endpoints/v1/backends/secrets.md#get-backendssourcesecretslist) | GET | List declared secrets and whether each is configured — no values. |
| [`/backends/{source}/secrets/{name}`](../endpoints/v1/backends/secrets.md)            | GET, POST, DELETE | Check (`configured` only), set, or clear one secret.                  |

A secret value is supplied only in a `POST` request body and is never returned
by any method. See [secrets.md](../endpoints/v1/backends/secrets.md) for the
full request/response shapes and error model.
