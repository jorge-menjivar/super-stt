<!-- SPDX-License-Identifier: GPL-3.0-only -->
# JSON Schemas

These JSON Schemas are **generated** from the canonical Rust types in
`super-stt-registry-types` — do not edit them by hand. Run `just gen-schemas`
after changing those types; CI fails if the committed files are stale.

- [`backend.schema.json`](./backend.schema.json) — the `backend.toml` manifest
  contract every backend ships (see `docs/protocol/backend/config.md`).
- [`registry.schema.json`](./registry.schema.json) — the maintainer-facing
  `registry/registry.toml`.

## Using the schema in your backend repository

Add a `#:schema` directive as the first comment line of your `backend.toml` to
get autocomplete, hover docs, and validation in any taplo-based editor
(Even Better TOML, Helix, Zed, the `taplo` CLI):

```toml
#:schema https://raw.githubusercontent.com/jorge-menjivar/super-stt/main/schemas/backend.schema.json
```

Pin to a tag instead of `main` if you want the schema to stay fixed to a
specific contract revision.
