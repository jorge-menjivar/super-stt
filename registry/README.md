# Super STT Backend Registry

This directory holds the source of truth for the backend catalog users
browse in the Super STT app (**Library → Browse**). A scheduled GitHub
Action (every 6 hours) reads
`registry.toml`, queries each entry's GitHub repo for its latest release,
validates the release's `backend.toml` and assets, and publishes a single
`index.json` to the `gh-pages` branch.

End users do not interact with this directory.

## Submitting a backend

1. Build and host your backend in your own git repo on a supported forge (currently GitHub). It must include a
   `backend.toml` at the chosen subdirectory (default: repo root), declaring
   `[assets.wasm]` (for wasm backends) or `[[assets.subprocess]]` (for
   subprocess backends), and a `[backend].license` — a recognized open-source
   [SPDX identifier](https://spdx.org/licenses/) (OSI-approved or FSF Free/Libre) or the literal `other`.
   See `docs/protocol/backend/config.md`.

   Declare the lowest [`contract`](../docs/protocol/backend/config.md#contract-generations)
   generation whose fields your manifest uses — `v1` for a backend that only
   transcribes, `v2` for one that declares a `post_processor` model. That is
   the only compatibility statement you make: the indexer stamps each entry
   with the Super STT release its generation needs, and a client that predates
   it says so instead of installing something it cannot drive.

   A client older than the generation *check itself* (0.2.3 and earlier) does
   not have the stamp to read: it lists the entry as installable and refuses
   later, when it parses the manifest it downloaded. The refusal is correct
   either way — nothing is installed that cannot run — but only a client from
   0.2.4 on can explain it up front.
2. Open a PR adding a new entry to `registry.toml` in **alphabetical order**:

   ```toml
   [my-backend]
   repo  = "github.com/your-name/my-backend"
   forge = "github"
   ```

   `forge` is **required**: the git host that publishes your releases
   (`github` is the only supported value today). Optional fields: `subdir`,
   `tag_prefix`, `max_version`. These entry fields are defined by the
   registry schema (`registry.schema.json`, linked from the `#:schema` line
   at the top of `registry.toml`).

   `id` is **required**. The schema linked from `registry.toml` flags an entry
   without one, and the indexer refuses it. Six entries predate the rule and
   still lack an `id`; they are tolerated so the catalog keeps building, and
   each is fixed by adding `[backend].id` to that backend's own repo and
   cutting a release *before* the id is added here — an entry that declares an
   `id` pins the release to it, so the file must be edited last.

3. Reviewers check: the id isn't already claimed by an existing or
   `removed = true` entry; your repo's license is acceptable; you control
   `repo` (CODEOWNERS or a one-time challenge file at HEAD); and
   `allowed_hosts` in your `backend.toml` doesn't request wildcards that
   would be hard to vet.

4. After merge, the indexer auto-discovers releases on your repo. You
   ship new versions by tagging releases — no further PRs to this repo.

   **Attach `backend.toml` as a release asset**, alongside your binaries. The
   indexer pins its SHA-256, and the daemon installs those exact bytes after
   verifying the hash — so every field your manifest declares (languages, model
   files, …) reaches the daemon without re-encoding. This is **required**: a
   release without the `backend.toml` asset is not installable and the indexer
   fails that entry.

### `id`

Every new entry must declare an `id`: a reverse-DNS identifier under a domain
you control, e.g. `com.example.voxtral`. See
[backend/config.md](../docs/protocol/backend/config.md) for the format.

- It must be unique across the registry.
- It must be identical to the `[backend].id` in the `backend.toml` published
  by the release the entry points at. A release whose manifest declares a
  different id, or none, is rejected.

Entries that predate this requirement are accepted without an `id`. They stop
being exempt as soon as they declare one.

## Removing or yanking

- **Yank a specific bad version** without removing the backend: add
  `max_version = "<last-good>"` to the entry. The indexer treats anything
  above as if it didn't exist.
- **Remove the entry entirely** without giving up the id: set
  `removed = true` (keeps the row for audit; prevents squatters).
