# Super STT Backend Registry

This directory holds the source of truth for the backend catalog that ships
in the Super STT app's Download tab. A nightly GitHub Action reads
`registry.toml`, queries each entry's GitHub repo for its latest release,
validates the release's `backend.toml` and assets, and publishes a single
`index.json` to the `gh-pages` branch.

End users do not interact with this directory.

`registry.toml` carries a `#:schema` directive pointing at the registry schema
published on GitHub Pages. The schema is generated from the entry types — run
`just gen-schemas` to produce it locally (written to a gitignored
`target/schemas/`); CI regenerates and publishes it. Do not edit it by hand.

## Submitting a backend

1. Build and host your backend in your own git repo on a supported forge (currently GitHub). It must include a
   `backend.toml` at the chosen subdirectory (default: repo root), declaring
   `[assets.wasm]` (for wasm backends) or `[[assets.subprocess]]` (for
   subprocess backends), and a `[backend].license` — a recognized open-source
   SPDX identifier (OSI-approved or FSF Free/Libre) or the literal `other`.
   See `docs/protocol/backend/config.md`.
2. Open a PR adding a new entry to `registry.toml` in **alphabetical order**:

   ```toml
   [my-backend]
   repo  = "github.com/your-name/my-backend"
   forge = "github"
   ```

   `forge` is **required**: the git host that publishes your releases
   (`github` is the only supported value today). Optional fields: `subdir`,
   `tag_prefix`, `max_version`. See the comments at the top of
   `registry.toml` and the spec at
   `docs/superpowers/specs/2026-05-29-backend-registry-design.md`.

3. Reviewers check: id is not on the reserved list (below); your repo's
   license is acceptable; you control `repo` (CODEOWNERS or a one-time
   challenge file at HEAD); and `allowed_hosts` in your `backend.toml`
   doesn't request wildcards that would be hard to vet.

4. After merge, the indexer auto-discovers releases on your repo. You
   ship new versions by tagging releases — no further PRs to this repo.

   **Attach `backend.toml` as a release asset**, alongside your binaries. The
   indexer pins its SHA-256, and the daemon installs those exact bytes after
   verifying the hash — so every field your manifest declares (languages, model
   files, …) reaches the daemon without re-encoding. This is **required**: a
   release without the `backend.toml` asset is not installable and the indexer
   fails that entry.

## Reserved ids

These ids are reserved for the upstream maintainers and may not be claimed
by third-party backends:

- `openai`, `anthropic`, `mistral`, `deepgram`, `voxtral`, `whisper`
- `azure`, `google`, `gcp`, `aws`, `bedrock`
- `super-stt`, `super-stt-*`

## Removing or yanking

- **Yank a specific bad version** without removing the backend: add
  `max_version = "<last-good>"` to the entry. The indexer treats anything
  above as if it didn't exist.
- **Remove the entry entirely** without giving up the id: set
  `removed = true` (keeps the row for audit; prevents squatters).
