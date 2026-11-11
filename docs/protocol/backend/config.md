# Backend Configuration

Every backend ships a `backend.toml` at the root of its directory. The
configuration is the backend's declaration of identity: which models it provides,
which servers it connects to, which files those models need, and which
secrets it requires. The daemon reads it to **discover** a backend without
starting it — discovery is a filesystem scan, so installing a backend costs
nothing until a model it provides is selected.

This document is part of the [backend protocol](./contract.md); see also
[wasm.md](./wasm.md) and [subprocess.md](./subprocess.md) for how the
configuration's fields are honored per transport.

One `backend.toml` serves both of Super Voice's features, speech to text and
text to speech: each model's [`role`](#model-roles) says which it is for, and
one backend may hold models of both.

A JSON Schema for this file is generated from the shared manifest types in
[`super-engine-spec`](https://github.com/super-libre/super-engine), plus Super
Voice's own fields in `super-voice-registry-types`, and published to GitHub
Pages by CI (it is not committed to the repo). Backends in other repositories
reference it at
`https://jorge-menjivar.github.io/super-voice/backend.schema.json`.
Add that URL as a `#:schema` comment line at the top of a `backend.toml` to get
autocomplete and validation in taplo-based editors. Generate it locally with
`just gen-schemas`, which writes to a gitignored `target/schemas/`.

## Discovery

A backend is a directory whose root contains `backend.toml`. The daemon scans
its configured backend search paths and treats every such directory as a
backend. Representative layout:

```text
<backend-dir>/
├── backend.toml            # this configuration
├── whisper-backend         # entrypoint (subprocess binary) …
│                           # … or whisper.wasm (WASM component)
└── models/                 # populated by the daemon at load time
    └── whisper-tiny/
        ├── config.json
        ├── tokenizer.json
        └── model.safetensors
```

The daemon never writes outside a backend's own directory, and a backend
reads only from it. Files a model needs are downloaded by the daemon into
the `dest` paths declared in the configuration (see
[`[[models.files]]`](#modelsfiles)).

## `[backend]`

Backend identity and packaging.

```toml
[backend]
id          = "app.super-voice.whisper"
source      = "github.com/super-voice/whisper"
name        = "Whisper (local)"
version     = "0.1.0"
kind        = "subprocess"
entrypoint  = "whisper-backend"
product     = "super-voice"
contract    = "v4"
license     = "Apache-2.0"
description = "Local Whisper speech-to-text."
```

| Field        | Type   | Required        | Notes                                                                 |
|--------------|--------|-----------------|-----------------------------------------------------------------------|
| `id`         | string | **yes** from `contract = "v2"`; for publication under `v1` | Globally unique reverse-DNS identifier for the backend, e.g. `app.super-voice.voxtral`. Names the directory the backend is installed into, and is what the registry matches an entry against. Required outright from `v2`; see [contract generations](#contract-generations). |
| `source`     | string | yes             | Canonical repository id for this backend. Becomes the `source` of every model it provides (see [identity](./contract.md#model-identity)). Must be unique across installed backends. |
| `name`       | string | yes             | Human-readable display name.                                          |
| `version`    | string | yes             | Backend version (semver).                                            |
| `kind`       | string | yes             | `subprocess` or `wasm` — selects the transport.                       |
| `entrypoint` | string | yes             | Path, relative to the backend directory, to the executable (`subprocess`) or the `.wasm` component (`wasm`). |
| `product`    | string | **yes** from `contract = "v4"` | The product the backend is built for: `super-voice`, or `super-stt`, which Super Voice continues. A manifest naming any other product is refused under every contract. Below `v4` it may be left out, and the backend is read as a Super Voice one; see [contract generations](#contract-generations). |
| `contract`   | string | yes             | The [contract generation](#contract-generations) the backend implements: `v1`, `v2`, `v3` or `v4`. Declare the lowest generation whose fields you use. Unknown values are rejected. |
| `license`    | string | for publication | SPDX identifier of a current OSI-approved or FSF Free/Libre license (e.g. `Apache-2.0`, `MIT`, `GPL-3.0-only`), or the literal `other` for a license outside that set. Required for registry publication; optional for locally installed backends. |
| `description`| string | yes             | One-line, human-readable summary shown in the registry/Browse listing. |

`license` is checked against the SPDX license list embedded in the registry
indexer — no network access — and must be a single, current (non-deprecated)
SPDX identifier that the list marks OSI-approved or FSF Free/Libre, or the
literal `other`. License *expressions* (`MIT OR Apache-2.0`) are not accepted;
declare a single identifier or `other`. A backend declaring `other` is still
published — the app surfaces its license as "Other" — so the value is a
conscious declaration, not an omission.

#### `id` format

- Lowercase ASCII letters, digits, `-`, and `.` only.
- At least three `.`-separated segments.
- Each segment is non-empty, begins with a letter, and does not end with `-`.
- No leading, trailing, or consecutive dots.
- At most 255 bytes.

The reverse-DNS form namespaces a backend under a domain its author
controls, so two unrelated authors may both publish a backend named
`voxtral`: `app.super-stt.voxtral` and `com.example.voxtral` coexist.

`id` names the install directory. It is not part of model identity, which is
the `(name, source)` pair described in [contract.md](./contract.md).

### Contract generations

`contract` is the one thing a manifest says about what it needs from Super
Voice. A generation names a set of manifest fields and backend routes, and
each generation **extends** the one before rather than replacing it: `v2` is
everything `v1` defines, plus what `v2` adds. Declare the lowest generation
whose fields you use: a backend that only transcribes is `v1` however new it
is.

Super Voice continues Super STT's line, so every backend published for Super
STT declares one of these generations and keeps its meaning. Text to speech
joins at `v4`, the first generation Super Voice ships.

| Generation | Adds                                                                         | First supported by |
|------------|------------------------------------------------------------------------------|--------------------|
| `v1`       | The base contract: transcription over [`POST /v1/transcribe`](./contract.md#post-v1transcribe). | Super STT 0.2.0    |
| `v2`       | [`[[models]].role`](#model-roles) and [`POST /v1/process`](./contract.md#post-v1process): transcript post-processors. [`[[models]].force_preview_support`](#models): live previews for a model that has none of its own. Also **requires** [`[backend].id`](#backend). | Super STT 0.2.4    |
| `v3`       | No manifest field. The daemon grants a subprocess backend a [cache directory](./subprocess.md) that survives the process, and reads the [load progress](./contract.md#get-v1status) (`phase`, `step`, `progress`) of `GET /v1/status`. Declare it when a load builds GPU kernels: an older daemon rebuilds them on every load and cuts a slow one off at its flat load limit. | Super Voice 0.3.0  |
| `v4`       | [`[[models]].prepare_files`](#models) and [`POST /v1/prepare`](./contract.md#post-v1prepare): a first load's GPU kernels are built from the model's configuration files while its weights still download. Text to speech: `role = "synthesis"`, its model keys, and [`POST /v1/synthesize`](./contract.md#post-v1synthesize) with the cloned-voice routes for models that declare them. **Requires** [`[backend].product`](#backend). | Super Voice 0.3.0  |

Extending the contract does not oblige a backend to serve all of it. Which
routes a backend must implement is decided by the models it declares, not by
its generation: a backend serving only `post_processor` models implements
`POST /v1/process` and never `POST /v1/transcribe`. The generation says what
the daemon may *expect to find*; the models say what it will actually call.

You never write a Super Voice version. The generation implies one, and the
registry carries it: the indexer stamps every index entry with `min_client`,
the release that first understood that entry's generation (`0.3.0` for `v4`).
It is **reported, never compared**. A daemon decides compatibility by whether
it can parse the generation at all, so a prerelease of the release named there
(`0.3.0-beta.1`, which semver orders *below* `0.3.0`) is not wrongly locked
out. `min_client` is there so that a client meeting a backend it cannot
install can name the version to update to, instead of showing only a failure
to parse.

A Super Voice that predates the generation cannot parse it, so it shows the
backend in **Browse** as needing a newer Super Voice, naming the version, and
does so whether or not "Show incompatible" is on, since that toggle is for
hardware this machine cannot satisfy, not for something the user can fix by
updating.

The refusal itself needs no field. `contract` is a closed set on the reading
side, so a build that predates a generation cannot parse a manifest declaring
it, and says which ones it does know:

```
unknown contract `v5`; this build knows v1, v2, v3, v4
```

It is the *absence* of knowledge that refuses. That is what makes every
generation added in future gate itself against every Super Voice already
released, this one included, with no `min_version` field anyone had to think
to add first.

Which generation introduced, or began requiring, each manifest field lives in
one table (`CONTRACT_FIELDS`, in `super-voice-registry-types`). Both the
manifest parser and the published `backend.schema.json` learn the rule from
that row, so an editor bound to the schema flags exactly what the parser would
refuse, before anything is released.

A row expresses one of two rules:

1. A field **added** in a generation may not be spelled under an older one:

   ```
   `[[models]].role` requires `contract = "v2"`, but this manifest declares
   `contract = "v1"` — raise the contract to use the field, or remove the field to
   stay on `v1`
   ```

   Both fixes are named because they are not equivalent. Raising the contract
   is right for a manifest that means to use the field, but it also raises the
   release floor for everyone installing the backend; an author who wrote the
   field's *default* value out by hand wants the other fix. Spelling a newer
   field with its default value still counts as declaring it: the rule is about
   what the document says, not about what the parsed manifest ends up holding.
2. A field a generation **requires** is the mirror image, for closing an
   optionality that only survived for backward compatibility. `id` is the
   first: a published backend has always needed one (the indexer refuses a
   release without it), but the field stayed optional so that backends
   installed before it existed keep loading. No `v2` backend can predate it,
   so `v2` requires it, and the failure moves from a rejected release to the
   author's editor:

   ```
   `contract = "v2"` requires `[backend].id`, which this manifest does not declare
   — add it, or drop to `contract = "v1"` where it is optional
   ```

These rules apply to a manifest being **admitted**: a registry install, a
custom repo, an import from a directory, and the indexer itself. A backend
already installed on this machine is parsed with them switched off. Their job
is to stop a manifest getting in, and that one is already in. Enforcing them
at discovery would make a backend that installed cleanly under an earlier
build vanish from the catalog, taking its downloaded models out of reach, over
a file the user did not write and cannot be expected to edit. The `product`
rule below is not exempt: a backend built for another product cannot run here
however it got onto disk.

`product` keeps other products' backends out. Super Voice reads a manifest
naming `super-voice` or `super-stt` as its own, since it continues Super STT
and every backend published before the rename names it. It refuses any other
product under every contract:

```
this backend is built for <product>, not super-voice
```

Every backend published before the field existed is a speech-to-text backend
of this line, so below `v4` it may be left out, and a manifest without it is
read as a Super Voice one. `v4` requires it.

Only field *names* are versioned this way. A generation that widens an
existing field's value set instead (a new model `role`, a new device) cannot
be expressed in that table. `role = "synthesis"` is the one such value so
far, and the parser refuses it below `v4` itself:

```
model `m` has role = synthesis, which requires `contract = "v4"`; this manifest
declares `contract = "v2"`
```

## `[network]`

Outbound network the backend is permitted to reach.

```toml
[network]
allowed_hosts = ["api.openai.com"]
```

| Field           | Type             | Required | Notes                                                              |
|-----------------|------------------|----------|--------------------------------------------------------------------|
| `allowed_hosts` | array of string  | no       | Host or `host:port` egress allowlist. Empty or absent ⇒ no network beyond a user-set [`base_url`](#base_url-and-egress). |

`allowed_hosts` is honored for `wasm` backends, where the daemon enforces it
on every outbound request (see [wasm.md](./wasm.md#network-egress)).
`subprocess` backends run with no network regardless; the field must be
empty for them.

## `[[secrets]]`

Encrypted credentials the backend needs at runtime, such as API keys. A
backend may declare several — a primary and a fallback key, or keys for
different upstreams — and use whichever it needs. The user supplies each
value through the settings UI; the daemon stores it in its secrets file,
which only the user can read, and never in its config.

```toml
[[secrets]]
name        = "openai_api_key"
label       = "OpenAI API key"
description = "Used to authenticate requests to api.openai.com."

[[secrets]]
name        = "azure_openai_key"
label       = "Azure OpenAI key"
description = "Used only when an Azure deployment is set."
required    = false
```

| Field         | Type   | Required | Notes                                                       |
|---------------|--------|----------|-------------------------------------------------------------|
| `name`        | string | yes      | snake_case identifier the backend reads the value by. `[a-z][a-z0-9_]*`, unique within the table. |
| `label`       | string | no       | Human-readable label shown in the settings UI. Falls back to `name` when absent. |
| `description` | string | yes      | Help text shown beside the input in the settings UI.        |
| `required`    | bool   | no       | Whether a value must be set before the backend can load. Default `false`. |

## `[[options]]`

Non-secret configuration the user can set through the settings UI — a
base-URL override, a timeout, and so on. Options are declared like secrets
and shown beside them, but the daemon stores their values as plaintext
configuration rather than encrypting them.

```toml
[[options]]
name        = "base_url"
label       = "API base URL"
description = "Override the API base URL, e.g. for a gateway."
type        = "string"

[[options]]
name        = "request_timeout_seconds"
label       = "Request timeout"
description = "Per-request timeout in seconds."
type        = "integer"
default     = 30

[[options]]
name        = "strip_filler_words"
label       = "Remove filler sounds"
description = "Drop um, uh and similar from the transcript."
type        = "bool"
default     = true

[[options]]
name        = "styling"
label       = "Styling"
description = "The register the transcript is rewritten in."
type        = "string"
default     = "semi-formal"
choices     = ["casual", "semi-casual", "semi-formal", "formal"]
```

A `bool` option is rendered as a switch, so it has no Save button: flipping it
writes immediately, and flipping it back to `default` clears the stored
override rather than writing a value identical to it. Declare a `default` for
every `bool` — without one the switch starts off, and turning it off again has
nothing to revert to.

An option that declares `choices` is rendered as a dropdown, and behaves the
same way: picking writes, and picking the `default` back off the list clears
the override. Declare them whenever the option takes a closed set of values.
Writing the allowed values into the `description` instead leaves the user a
text field, and a value that is merely close enough is stored and sent to the
backend as if it were valid.

The daemon refuses a write of any value not on the list, so `choices` is a
contract and not only a hint: `POST /backend/{id}/option/{name}` answers
`400 invalid_value`. A `bool` must not declare `choices` — it is already a
switch over the two values its type names — and the `default`, when there is
one, has to be on the list. Both are refused at publication.

An option value is delivered as an `x-voice-option-<name>` request header, which
sets the shape a value may take: at most 4000 characters, and no control
characters. The daemon refuses a user's write of anything else with
`400 invalid_value`, and a `default` that breaks the same rule is refused at
publication — a default is what a user gets for touching nothing, so one that
cannot be delivered would break the backend for everyone who installs it and
could not be undone from the settings UI.

| Field         | Type           | Required | Notes                                                  |
|---------------|----------------|----------|--------------------------------------------------------|
| `name`        | string         | yes      | snake_case identifier the backend reads the value by. `[a-z][a-z0-9_]*`, unique within the table. |
| `label`       | string         | no       | Human-readable label shown in the settings UI. Falls back to `name` when absent. |
| `description` | string         | yes      | Help text shown beside the input in the settings UI.   |
| `type`        | string         | no       | `string`, `integer`, `float`, or `bool`. Drives the input the UI renders: `bool` gets a switch that writes on the flip, everything else a text field the user saves. The daemon refuses a value that is not of this type. Default `string`. |
| `default`     | matches `type` | no       | Value used when the user sets none. Forbidden on `base_url` — see below. When `choices` is present, must be one of them. |
| `choices`     | array of `type` | no      | The values this option accepts. Renders a dropdown, and the daemon refuses to store anything else. Omit it for an open-ended option. Entries must be unique, and a `bool` must not declare any. |
| `min`         | number         | no       | Lowest value accepted, inclusive. `integer` and `float` only. |
| `max`         | number         | no       | Highest value accepted, inclusive. `integer` and `float` only. |
| `step`        | number         | no       | The increment a control moves in. With `min` and `max`, renders a slider. Not enforced. See [ranges and sliders](#ranges-and-sliders). |
| `required`    | bool           | no       | Whether a value must be set before the backend can load. Default `false`. |

#### Types

`type` is also a contract. The daemon stores every value as text, because an
option request header is text, and `type` is the claim about what that text
parses back to. The daemon keeps the claim: `POST /backend/{id}/option/{name}`
answers `400 invalid_value` for a value that is not of the declared type,
naming the type rather than listing choices. A `warm` typed into a `float` is
a different mistake from a value that is merely off the dropdown.

1. `string`, the default, takes anything. An option that declares no type is
   free text and always was.
2. `integer` takes what parses as a 64-bit signed integer.
3. `float` takes what parses as a double, except the infinities and
   not-a-number. They parse, but they are the values that turn an arithmetic
   bug into a silent one.
4. `bool` takes exactly `true` or `false`.

A `float` is for a setting whose steps are not whole: a sampling temperature,
a gain, a threshold. Declaring it `string` and parsing it in the backend is
the same thing with the check moved to the far side of the wire, where the
only way left to report the problem is to fail a request the user already
made.

The type binds the manifest's own values too. A `default` or a `choices` entry
that is not of the declared type is refused at publication, since it names a
value the daemon would then refuse to store.

#### Ranges and sliders

A numeric option can declare bounds. `min` and `max` are inclusive, and the
daemon keeps them the way it keeps `choices`: a write outside them is
`400 invalid_value`, naming the range.

`step` is the increment the value moves in, and declaring all three is what
makes the option a **slider** — the same shape-implies-control rule that makes
a `choices` option a dropdown and a `bool` a switch. An option bounded at both
ends with a declared grid has nothing left to type; one missing any of the
three still does, and gets a field. The text-to-speech settings app renders
such an option as a slider. The speech-to-text settings app does not use
`step` yet.

The grid belongs to the control, not to the contract. The daemon enforces
`min` and `max` and ignores `step`, because a value between two notches is
still one the option said it could take. Refusing it would make the option
narrower than its own bounds say, and would turn every float that is not
exactly representable into a bug report. A client that renders the slider is
what keeps to the grid.

The rules, all checked at publication:

1. `min`, `max` and `step` are only for `integer` and `float`. A range over
   strings is not a thing the manifest can mean.
2. `min <= max`, and every bound is finite.
3. `step` is greater than zero, no larger than the range it divides, and needs
   both `min` and `max` — a slider with one end has nothing to slide between.
4. A slider needs a `default`. It always shows a position, so it has to rest on
   a value the backend named.
5. An `integer` option's bounds are whole numbers, or its slider produces
   values the option itself refuses.
6. A `default` is inside the range.
7. An option declares `choices` **or** a range, never both: a closed set is
   already the values it accepts, and a client cannot render a dropdown and a
   slider at once.

#### `base_url` and egress

An option named `base_url` is the convention for a backend's configurable
endpoint. When the user sets one, the daemon treats its authority as
**user-authorized egress** for the backend: it is added to the WASM transport's
egress set, and the SSRF resolver guard is relaxed for it (see
[wasm.md — Network egress](./wasm.md#network-egress)). This lets a cloud backend
be pointed at an arbitrary gateway — public, local, or on a private network —
without re-installing the backend.

The egress set is consulted per outbound connection, so changing `base_url`
takes effect on the backend's next request. Nothing is reloaded, and the value
is checked when it is written rather than when the model next loads: a value no
host can be read from is refused by
[`POST .../option/list/base_url`](../endpoints/v1/backends/options.md).

The name is load-bearing: the daemon recognizes `base_url` and nothing else. An
option called `endpoint`, `api_base`, or `server_url` is a perfectly valid
option, but its value authorizes no egress, so a backend that reads one instead
will have every request refused with `outbound host not allowed` once the user
points it somewhere the manifest's `allowed_hosts` does not cover.

`base_url` is the one option a manifest may declare but not supply a value for.
The reason is the paragraph below — the value authorizes egress the sandbox
would otherwise refuse, and a value the backend author wrote is not user intent.
Every other option keeps its default.

A manifest that declares one is refused **at publication**: the registry indexer
rejects the release, so it never reaches a user. A backend installed some other
way still loads, with the option intact and the declared value dropped and
logged — an author's mistake costs the user a setting, not the backend. Either
way the value never takes effect, so a backend that needs a working endpoint out
of the box carries it in the component and treats the option as an override.
That is what the missing `x-voice-option-base_url` header means when the user has
set nothing.

The value the backend receives is **canonical**. The daemon parses it and
re-serializes it as
`scheme://host[:port][/path]`: the scheme is lowercased, and supplied when
absent; userinfo is stripped; a trailing slash is removed; any query or
fragment is dropped. A port appears only when the user gave one — the daemon
does not add the scheme's default, which would otherwise travel to the upstream
in the `Host` header. The path is preserved exactly as written: it plays no part
in egress, and only the backend knows which path its own API serves.

Normalizing in the daemon rather than in each backend keeps every backend
working from the same value the daemon authorized, and spares each one its own
URL parser.

The same rewrite is applied when the value is **set**, so what
[`POST /backend/{backend_id}/option/{name}`](../endpoints/v1/backends/options.md)
stores is already canonical and the settings field reads back the endpoint that
will be dialed. The scheme is why this is worth doing at the write boundary
rather than only at load: whether a request is encrypted should not be
invisible in the field the user is looking at. A value that yields no host is
the exception — it is stored as typed, so the load-time refusal can name it,
rather than being dropped.

#### The scheme a value without one is read as

A value carrying no scheme is read as `http` when its host is an address the
daemon can see is local — a loopback or private-range IP literal, or the name
`localhost` — and as `https` otherwise. A local endpoint is nearly always a
plaintext one, and reading it as `https` fails every time; a public endpoint is
the opposite. The choice is logged.

Only a value that names no scheme is decided this way, and it is decided before
anything connects. A value that says `https` stays `https` however it fails. The
daemon never retries a failed TLS connection over plaintext: that would let
anyone able to break the handshake move the user's audio and credentials into
the clear, and it would look like success.

A host the daemon cannot classify without resolving it — any name other than
`localhost` — is read as `https`. Guessing `https` for a plaintext endpoint
costs a failed connection, which is loud and recoverable. Guessing `http` for a
TLS one discloses whatever the request carries. Where the two are not equally
wrong, the daemon takes the loud failure.

A value the daemon cannot read as a URL fails the model load, with a message
naming the option. It is not quietly dropped: falling back to the backend's
built-in endpoint would send the user's audio and credentials to the very vendor
they had configured their way out of.

The daemon derives exactly one `host:port` authority from the configured value.
An explicit port is taken as written; otherwise the scheme's default applies —
`http` and `ws` ⇒ 80, `https` and `wss` ⇒ 443 — over the scheme the value names
or the one it is [read as](#the-scheme-a-value-without-one-is-read-as), so the
port the daemon authorizes is the one the backend dials. Any path, query, or
userinfo in the value plays no part in
this derivation, and a value that yields no host contributes nothing: the
backend keeps whatever egress its manifest declares.

The host is authorized on its own as well, so a gateway stays reachable on its
other ports — but only the derived `host:port` carries the relaxation below.
Another port on the same host is therefore reachable while it is public, and
refused once it is local or private.

Relaxing the guard is safe because the value is **the user's only**: the daemon
reads it from config set through the settings-scoped API, never from the
component and never from the manifest, so a backend cannot self-authorize a
metadata endpoint or localhost target. The relaxation lifts the loopback and
private-range blocks — reaching a gateway on `127.0.0.1` or `10.0.0.0/8` is the
point of the option — and nothing further. Link-local addresses
(`169.254.0.0/16`, `fe80::/10`), including the cloud metadata endpoint
`169.254.169.254`, along with the unspecified and broadcast addresses, stay
refused for every backend however they were authorized. Manifest-declared
`[network].allowed_hosts` entries remain fully SSRF-guarded.

Both secrets and options reach the backend the same way — injected request
headers on every `/v1` request — and differ only in how the daemon stores
them at rest. See [request headers](./contract.md#request-headers).

## `[assets]`

Declares the binary artifacts a release publishes, so the registry indexer and
the daemon's installer can find them without guessing. The shape depends on the
backend's `kind`.

`[assets]` is required for registry publication (the indexer rejects a release
without the table matching the backend's `kind`) and optional for backends
installed locally — an imported directory has no release artifacts to declare.

Wasm backends declare a single file:

```toml
[assets]
wasm = "openai.wasm"   # filename on the release; must be a `wasm32` component
```

Subprocess backends declare one entry per built variant. Selection axes are
`target`, `accel`, and — gated on which accelerator(s) `accel` names —
`cuda_major`/`cuda_sm`/`cudnn` (`cuda`), `gfx` (`rocm`), and `vulkan_api`
(`vulkan`). Each variant names its archive with `file`, or with `parts` when
the `.tar.gz` exceeds the 2 GiB release-asset limit (see [Multi-part assets](#multi-part-assets)).

```toml
[[assets.subprocess]]
file   = "voxtral-x86_64-unknown-linux-gnu-cpu.tar.gz"
target = "x86_64-unknown-linux-gnu"
accel  = "cpu"

[[assets.subprocess]]
file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-sm75.tar.gz"
target     = "x86_64-unknown-linux-gnu"
accel      = "cuda"
cuda_major = 12
cuda_sm    = 75

[[assets.subprocess]]
file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-cudnn-sm75.tar.gz"
target     = "x86_64-unknown-linux-gnu"
accel      = "cuda"
cuda_major = 12
cuda_sm    = 75
cudnn      = true

[[assets.subprocess]]
file   = "voxtral-x86_64-unknown-linux-gnu-rocm.tar.gz"
target = "x86_64-unknown-linux-gnu"
accel  = "rocm"
gfx    = ["gfx1030", "gfx1100", "gfx1101"]

[[assets.subprocess]]
file       = "voxtral-x86_64-unknown-linux-gnu-vulkan.tar.gz"
target     = "x86_64-unknown-linux-gnu"
accel      = "vulkan"
vulkan_api = "1.3"

# A GPU build that also runs on the CPU lists both. A model on it moves to
# the CPU without installing the CPU build; one that lists `cuda` alone has
# no CPU path, and the CPU is not offered for it.
[[assets.subprocess]]
file       = "voxtral-x86_64-unknown-linux-gnu-cuda12-sm86.tar.gz"
target     = "x86_64-unknown-linux-gnu"
accel      = ["cuda", "cpu"]
cuda_major = 12
cuda_sm    = 86

# A build carrying more than one runtime — `accel` as an array — matches
# either. Host matching then considers every accelerator listed.
[[assets.subprocess]]
file       = "voxtral-x86_64-unknown-linux-gnu-cuda-rocm.tar.gz"
target     = "x86_64-unknown-linux-gnu"
accel      = ["cuda", "rocm"]
cuda_major = 12
cuda_sm    = 75
gfx        = ["gfx1030", "gfx1100"]
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `file` | string | one of `file`/`parts` | Filename on the GitHub release. Subprocess: `.tar.gz`; wasm: `.wasm`. |
| `parts` | array of strings | one of `file`/`parts` | **Subprocess only.** Ordered release filenames whose byte-for-byte concatenation is the variant's `.tar.gz`. Use instead of `file` when the archive would exceed the 2 GiB release-asset limit. See [Multi-part assets](#multi-part-assets). |
| `target` | string | yes | Rust target triple. Tier-1/2 only; indexer rejects unknown. |
| `accel` | string or array of strings | yes | Every framework the build runs on, and only those: a single value, or a non-empty array, drawn from `"cpu"`, `"cuda"`, `"rocm"`, `"metal"`, `"vulkan"`. The daemon offers a model a framework only when a build lists it, and moves a model to one without another install only when the installed build lists it. A GPU build that also runs on the CPU lists `"cpu"` too (`["cuda", "cpu"]`); one that does not, does not. An array also declares one build carrying several GPU runtimes (e.g. a llama.cpp binary built with both CUDA and HIP support); the build then matches a host that satisfies any of them. When two builds serve a host equally well, the daemon installs the one listing fewer frameworks, so a CPU-only host gets the CPU build rather than a `["cuda", "cpu"]` one. |
| `gfx` | array of strings | for `accel` containing `rocm`; forbidden otherwise | AMD architecture targets in `--offload-arch` spelling (`"gfx1030"`, `"gfx90a"`). There is deliberately no wildcard: HIP code objects are architecture-specific AMDGCN ISA with no JIT path, so a build that does not list the host's target cannot run on it. A fat build lists every target it carries. |
| `vulkan_api` | string | no; allowed only when `accel` contains `vulkan` | Minimum Vulkan API version this build requires, as `"major.minor"` (e.g. `"1.3"`). |
| `cuda_major` | integer | for `accel` containing `cuda` | CUDA major version this build targets. |
| `cuda_sm` | integer | no | Compute capability (e.g. `75`, `86`, `90`). Omit to match **any** compute capability — use this for framework builds (e.g. a PyTorch wheel) whose kernels are multi-architecture. When both an exact-SM and a wildcard asset match a host, the exact-SM asset is preferred. Allowed only when `accel` contains `cuda`. |
| `cudnn` | bool | no | Defaults `false`. Allowed only when `accel` contains `cuda`. |

A variant gives `file` **or** `parts`, never both (and `wasm` always uses a
single `file`).

### Multi-part assets

A single GitHub release asset may not exceed **2 GiB**. A build whose `.tar.gz`
is larger — typically a CUDA framework bundle (PyTorch ships ~2.5 GiB of CUDA
libraries) — is split into ordered parts, each a separate release asset under
the limit, and listed in `parts` instead of `file`:

```toml
[[assets.subprocess]]
parts      = [
    "qwen3-asr-x86_64-unknown-linux-gnu-cuda13.tar.gz.part00",
    "qwen3-asr-x86_64-unknown-linux-gnu-cuda13.tar.gz.part01",
]
target     = "x86_64-unknown-linux-gnu"
accel      = "cuda"
cuda_major = 13
```

The parts' **byte-for-byte concatenation, in the order listed**, reconstitutes
the original `.tar.gz`. The daemon downloads each part, verifies it, concatenates
them in order, then extracts the result. The registry indexer pins every part
independently (`{url, size, sha256}`), so every delivered byte is hash-verified —
there is no separate whole-archive digest. Splitting is purely a delivery
detail: the reassembled archive obeys the same archive-contents rules below, and
host selection (`target`/`accel`/`cuda_*`) is unaffected.

### Subprocess archive contents

A subprocess `.tar.gz` (the reassembled archive, when delivered as
[parts](#multi-part-assets)) MUST contain `bin/<entrypoint>` (the path that the
backend's `[backend].entrypoint` resolves to after extraction). Tarballs
containing path-traversal entries (`..`, absolute paths) or symlinks that
escape the archive root are rejected by the registry indexer and by the
daemon's installer.

## `[capabilities]`

Optional feature flags that unlock transport extensions beyond the base `/v1`
contract. All fields default to `false` and may be omitted entirely.

```toml
[capabilities]
websocket = true
context   = true
```

| Field       | Type | Required | Notes                                                                              |
|-------------|------|----------|------------------------------------------------------------------------------------|
| `websocket` | bool | no       | Opt into the `super-engine:realtime/ws` import and the `super-engine:realtime/ws-server` export (see [wasm.md — Realtime](./wasm.md#realtime-websocket)). When `true`, the daemon wires those interfaces into the WASM component for every session on a realtime model. **wasm-only** — a `subprocess` backend declaring `websocket = true` is rejected at discovery. Default `false`. |
| `context`   | bool | no       | Speech to text. Opt into being handed the user's [dictation context](../endpoints/v1/context.md): an `x-voice-prompt` and an `x-voice-vocabulary` header on every `/v1` request (see [contract.md — Request headers](./contract.md#request-headers)). Unlike `websocket` this is **not** transport-restricted — both headers ride the ordinary request, so a `subprocess` backend reads them exactly as a `wasm` one does. Declare it only if the backend actually reads them: a settings UI uses the flag to tell the user which backends will hear a vocabulary they typed. Default `false`, which means neither header is sent. |
| `streaming_input` | bool | no | Text to speech. **Reserved**: it would opt a backend into an incremental-text path, holding a provider's prosodic context across deltas. The daemon implements no such path yet, so a manifest setting it to `true` is refused rather than silently ignored. Default `false`. |

## `[[models]]`

One entry per model the backend provides. Each model is identified on the
wire by `(name, source)`, where `source` is the `[backend].source`
above.

```toml
[[models]]
name                   = "whisper-tiny"
role                   = "transcription"
multilingual           = true
primary_language       = "en"
supported_languages    = ["en", "es", "fr", "de", "zh"]  # abbreviated
supported_devices      = ["cpu", "gpu"]
estimated_vram_bytes   = 262144000
processing_interval_ms = 1000
```

| Field                    | Type            | Required | Notes                                                            |
|--------------------------|-----------------|----------|------------------------------------------------------------------|
| `name`                   | string          | yes      | Wire model name.                                                 |
| `role`                   | string          | no       | What the model is for: `transcription`, `post_processor` or `synthesis`. Default `transcription`. A `v2` field; `synthesis` needs `v4`. See [model roles](#model-roles). |
| `multilingual`           | bool            | no       | Whether the model accepts more than one language. Default `true`. When `true`, the model's route (`POST /v1/transcribe`, `/v1/process` or `/v1/synthesize`) accepts a `language` from `supported_languages`. |
| `primary_language`       | string          | yes      | Default language code (e.g. `en`); used when `language` is omitted. |
| `supported_languages`    | array of string | yes      | Language codes the model accepts; must include `primary_language`. When `multilingual` is `false`, it is exactly `[primary_language]`. |
| `supported_devices`      | array of string | yes      | Whether the model can use an accelerator at all — which accelerator an installed build actually targets is a property of the [asset](#assets), not the model. Non-empty, drawn from `["cpu", "gpu", "none"]`. `"cuda"` and `"metal"` are accepted input spellings for `"gpu"`; the daemon normalizes them and never emits them. `"none"` is the sentinel for remote/online models and must be the only entry when present. |
| `estimated_vram_bytes`   | integer         | no       | Conservative GPU memory estimate. Default `0`; use `0` for cloud models. |
| `processing_interval_ms` | integer         | no       | Suggested minimum interval between streaming passes, in ms.      |
| `realtime`               | bool            | no       | When `true`, the model is driven over the consumer-facing WebSocket endpoint (`GET /v1/stt/transcribe/realtime`) rather than batch `POST /v1/transcribe`. Requires `[capabilities] websocket = true`. Default `false`. For a synthesis model it is **reserved**: the transport works, but no session payload is defined yet. |
| `force_preview_support`  | bool            | no       | **Speech to text only.** A `v2` field. Force live previews onto a model that has none of its own: while its microphone is open, the daemon re-transcribes a sliding window of the capture every `processing_interval_ms` and shows the result. Each pass is a full transcription the final pass repeats, and for an online model a billed request whose output is discarded, so it is off unless the manifest turns it on. Default `false`. Not consulted for a `realtime` model, which streams its own previews. See [previews](../endpoints/v1/transcribe.md#previews). |
| `max_input_chars`        | integer         | no       | **Text to speech only.** Longest `text` the model accepts in one `POST /v1/synthesize`. Absent means unbounded: the daemon sends whole utterances and never splits for length. When set, the daemon chunks on sentence boundaries to stay under it. Must be non-zero. |
| `output_sample_rate`      | integer         | no       | **Text to speech only.** Native output rate in Hz, e.g. `24000`. **Advisory** — the authoritative rate is the `x-voice-sample-rate` response header on each synthesis, since a manifest cannot know what a cloud provider will actually return. It lets the settings UI show a rate and the daemon pre-size buffers before the first response. Must be 8000–192000. |
| `default_voice`          | string          | no       | **Text to speech only.** Voice used when a request omits `voice`. **Required** when `voices` is non-empty, and must name one of them. |
| `voice_kinds`            | array of string | no       | **Text to speech only.** Which `voice` id shapes this model accepts, from `["preset", "cloned", "described"]`. Default `["preset"]`. Non-empty. The daemon refuses a shape the model did not opt into, so a backend never sees an id it cannot resolve. |
| `clone_ref_seconds`      | number          | no       | **Text to speech only.** Longest reference audio accepted for a cloned voice, in seconds. **Required** when `voice_kinds` contains `cloned`, forbidden otherwise. Must be positive. The daemon trims a longer recording to this before registering it, so the value is a budget rather than a filter. |
| `clone_needs_transcript` | boolean         | no       | **Text to speech only.** Whether registering a cloned voice also requires the reference clip's transcript. Default `false`; forbidden without `cloned` in `voice_kinds`. Set it for in-context cloning, which conditions on the words as well as the audio — the daemon then refuses to register a clip stored without one, instead of letting every synthesis fail. Speaker-embedding cloning leaves it unset. |
| `voices`                 | array of table  | no       | **Text to speech only.** Preset voices the model provides — see [`[[models.voices]]`](#voices). Empty for models whose voices are entirely cloned or described. |
| `prepare_files`          | array of string | no       | The files the model's [prepare](./contract.md#post-v1prepare) reads, each a `destination` from the model's [`files`](#modelsfiles). The daemon downloads these first, starts the backend and sends `POST /v1/prepare`, then downloads the rest while the backend builds its kernels. Declaring it promises the backend serves `POST /v1/prepare` for this model, and that the prepare reads only these files. An entry that is not one of the model's `destination`s is refused. Default empty: no prepare. A `v4` field. |
| `provider`               | string          | no       | Compatibility field. Not part of model identity and read by nothing in the daemon; it is echoed back verbatim as `provider` in [`POST /v1/load`](./contract.md#post-v1load) so a backend that still validates it keeps loading. |

> **Compatibility.** `provider` was part of model identity before it became
> `(name, source)`. Backends released against the earlier contract compare the
> `provider` in `POST /v1/load` against their own fixed value and answer
> `400 invalid_model` on a mismatch, so a manifest declaring `provider` still
> has it forwarded on load. New backends should omit it, and should not
> validate it if they accept it.

`multilingual`, `primary_language`, and `supported_languages` together
describe language capability. When `multilingual` is `true`,
`POST /v1/transcribe` may carry a `language`, which must be one of
`supported_languages`; when omitted, `primary_language` is used. When
`multilingual` is `false`, the model transcribes only `primary_language`.

### Model roles

A model's `role` says what it is for. A model that leaves it out
transcribes, as every model did before the field existed in `v2`. Declare it
on every model of a backend that holds more than one role, so the manifest
reads the way the daemon does.

`role` decides which `/v1` route the daemon drives a model over, and where the
model can be selected:

| `role`           | Feature        | Route                    | Selected through                                        |
|------------------|----------------|--------------------------|---------------------------------------------------------|
| `transcription`  | speech to text | `POST /v1/transcribe`    | [`/stt/pipeline/1/model`](../endpoints/v1/pipeline.md)      |
| `post_processor` | speech to text | `POST /v1/process`       | [`/stt/pipeline/2`](../endpoints/v1/pipeline.md)            |
| `synthesis`      | text to speech | `POST /v1/synthesize`    | the text-to-speech pipeline                             |

The roles do not cross: selecting a model where another role belongs is
refused with `400 invalid_model`, since the model would be driven over a
route its backend does not serve.

A model takes the keys of its own feature. The text-to-speech keys
(`max_input_chars`, `output_sample_rate`, `default_voice`, `voice_kinds`,
`clone_ref_seconds`, `clone_needs_transcript`, `voices`) are refused on a
speech-to-text model, and `force_preview_support` on a synthesis model:
they would mean nothing there, so declaring one is a manifest bug.

```
model `m` has role = transcription, which does not take `voices`
```

A backend may declare models of several roles in one manifest — a local
weights bundle that both transcribes and cleans up, or a cloud provider that
both transcribes and speaks — and the daemon runs one instance of it per
selected model. Everything else is the same for every role: a model declares
`files`, `supported_devices` and languages the same way, is installed and
provisioned the same way, and reads the same `[[options]]` and
`[[secrets]]`.

A post-processor cannot set `realtime = true`: realtime is a property of
streaming audio in, and a post-processor is handed a finished transcript. A
manifest declaring both is refused wherever it is read: at publication, at
install, and at discovery.

```toml
[[models]]
name                = "cleanup-small"
role                = "post_processor"
primary_language    = "en"
supported_languages = ["en", "es", "fr"]
supported_devices   = ["cpu", "gpu"]

[[models]]
name                 = "kokoro-82m"
role                 = "synthesis"
primary_language     = "en"
supported_languages  = ["en", "es", "fr", "it", "pt"]
supported_devices    = ["cpu", "gpu"]
max_input_chars      = 500
output_sample_rate   = 24000
default_voice        = "af_heart"

[[models.voices]]
id       = "af_heart"
label    = "Heart"
language = "en"
```

<a id="voices"></a>
### `[[models.voices]]`

The preset voices a model provides, one table per voice. The settings UI renders
these as the voice picker; the daemon validates a request's `voice` against
them.

```toml
[[models.voices]]
id       = "af_heart"
label    = "Heart"
language = "en"
tags     = ["warm", "female"]
```

| Field      | Type            | Required | Notes                                                                          |
|------------|-----------------|----------|--------------------------------------------------------------------------------|
| `id`       | string          | yes      | The `voice` sent on `POST /v1/synthesize`. Unique within the model.            |
| `label`    | string          | no       | Display name for the picker. Falls back to `id`.                               |
| `language` | string          | no       | Primary language of this voice. Must be one of the model's `supported_languages`. |
| `tags`     | array of string | no       | Free-form, for filtering in the picker. Not interpreted by the daemon.         |

**Voice id shapes.** `voice_kinds` declares which of three shapes the model can
resolve:

| Kind        | Id shape       | Meaning                                                                 |
|-------------|----------------|-------------------------------------------------------------------------|
| `preset`    | a bare id      | One of the `[[models.voices]]` entries.                                  |
| `cloned`    | `voice:<uuid>` | A user-cloned voice. The daemon pushes the reference audio once, over [`POST /v1/voices`](./contract.md#post-v1voices), the first time the loaded model is asked for it — not on the synthesis request. Requires `clone_ref_seconds`. |
| `described` | `desc:<text>`  | A free-text voice description, for models that synthesize a voice from one. |

A model that declares only `preset` (the default) never receives a `voice:` or
`desc:` id — the daemon rejects those before they reach the backend, so a
backend need not defend against a shape it did not opt into.

`supported_devices` declares whether the model can use an accelerator at all.
It says nothing about *which* accelerator — CUDA, ROCm, Vulkan — since one
asset can serve several models and one of them may have no GPU path; that
detail lives on the asset that ends up installed (see
[`[[assets.subprocess]]`](#assets)) and is reported per-backend as
`installed_accel` (see [`GET /backend/list`](../endpoints/v1/backends.md)). The
settings app uses `supported_devices` to present the device choice at load
time. `"none"` marks a remote/online model with no local compute; mixing
`"none"` with any local device (`"cpu"` / `"gpu"`) is a contradiction and the
manifest is rejected.

### `[[models.files]]`

The files a model needs, and where to place them. `files` is an array in which
each entry describes **one file**: a download URL and the path to write it to.
The daemon fetches every file before calling `POST /v1/load`, one at a time in
the order listed, except that the model's [`prepare_files`](#models) come
first. Files are fetched the same way regardless of host — no source is given
special treatment. Cloud models declare no files.

Written compactly as an inline-table array on the model:

```toml
files = [
    { url = "https://huggingface.co/openai/whisper-tiny/resolve/main/config.json",
      destination = "models/whisper-tiny/config.json" },
    { url = "https://huggingface.co/openai/whisper-tiny/resolve/main/model.safetensors",
      destination = "models/whisper-tiny/model.safetensors", sha256 = "9f86d0…" },
]
```

The block form is identical TOML and may be used instead:

```toml
[[models.files]]
url         = "https://huggingface.co/openai/whisper-tiny/resolve/main/config.json"
destination = "models/whisper-tiny/config.json"
```

| Field         | Type   | Required | Notes                                                                |
|---------------|--------|----------|---------------------------------------------------------------------|
| `url`         | string | yes      | Full download URL for the file. Any host.                            |
| `destination` | string | yes      | Relative file path (including filename) under the backend directory. |
| `sha256`      | string | no       | Expected SHA-256, hex-encoded; verified after download.              |

`destination` must be a relative path that stays inside the backend directory:
absolute paths, `..` traversal, and backslashes are rejected.

#### Per-architecture variants

**Text to speech only.** A synthesis model's file entry may also carry a
**host selector**, written in the same vocabulary
[`[[assets.subprocess]]`](#assets) uses for a build. Entries sharing a
`destination` are then variants of one file: the daemon scores each against
the machine, downloads the best match, and leaves the rest alone. The backend
reads a fixed path and never learns which variant it got.

A speech-to-text model that sets `accel` on a file is refused, because the
daemon does not choose between variants for that feature yet:

```
model `m`: file `cache/kernels.bin` names a host (`accel`), and speech to text does not pick between file variants yet
```

| Field        | Type             | Notes                                                                                      |
|--------------|------------------|--------------------------------------------------------------------------------------------|
| `accel`      | string or array  | Acceleration families this variant is for. Absent — the default — matches every host.        |
| `cuda_major` | integer          | Matches a host whose installed CUDA runtime is at least this. Requires `cuda` in `accel`.    |
| `cuda_sm`    | integer or array | Compute capabilities this variant covers, e.g. `90` or `[86, 89]`. Requires `cuda`.         |
| `gfx`        | array of string  | AMD architecture targets, in `--offload-arch` spelling. Requires `rocm`.                     |
| `vulkan_api` | string           | Minimum Vulkan API version, e.g. `1.3`. Requires `vulkan`.                                   |
| `optional`   | bool             | Whether the model can load without this destination. Default `false`. Must agree across the variants of one destination. |

`cuda_sm` is a list where an asset's is a single value, because the two answer
different questions. A build that omits it is a fat binary with PTX behind it,
so "any capability" is a true claim and enumerating is rarely useful. A file
has no JIT to fall back on — kernels compiled for `sm_90` are inert on
`sm_86` — but one file may still carry entries for several devices, which is
exactly what a pre-warmed kernel cache is. Saying so once beats declaring the
same URL and hash under each. The bare number and the list are the same field:
`cuda_sm = 90` and `cuda_sm = [90]` mean the same thing, as `accel` already
works.

Every field of a file's selector is optional, which is the one place this
vocabulary is looser than an asset's. An asset has to *run* on the host, so it
must name `cuda_major` alongside `cuda` and a `gfx` target alongside `rocm`; a
file is data whose meaning belongs to the backend, so `accel = "cuda"` on its
own is a legitimate "for any CUDA host". Naming a narrower variant as well is
how you get both.

```toml
# Kernels precompiled for one card, with a wildcard CUDA build behind it and
# nothing at all for anyone else.
[[models.files]]
url         = "https://example.invalid/kernels-sm90.bin"
destination = "cache/kernels.bin"
accel       = "cuda"
cuda_sm     = 90
optional    = true

[[models.files]]
url         = "https://example.invalid/kernels-cuda.bin"
destination = "cache/kernels.bin"
accel       = "cuda"
optional    = true
```

**How a variant is chosen.** Host capability decides, never the user's device
preference — what is on disk must not change when the user toggles between
CPU and GPU. Candidates are ranked by accel family first (a native runtime over
Vulkan over `cpu` over an entry with no selector at all), then by that family's
own discriminators, where an exact `cuda_sm` or `gfx` beats a variant that
matched only the family. Ties go to the first declared, so declaration order is
your preference order. It is the same ranking that picks a build, applied to
the data beside it.

**When nothing matches.** A destination whose variants all miss the host is a
hole, and `optional` says which kind:

1. Weights are load-bearing, so the default (`false`) fails the load, naming
   what the host offered and what the variants wanted. That is better than a
   backend erroring on a file it was never handed.
2. A pre-warmed kernel cache is not: `optional = true` skips the destination,
   and the model loads without it and rebuilds what it needs.

## Example: local backend (subprocess)

A Whisper backend providing two models, loaded from Hugging Face. Whisper
models ship `config.json`, `tokenizer.json`, and a single
`model.safetensors`.

```toml
[backend]
id          = "app.super-voice.whisper"
source      = "github.com/super-voice/whisper"
name        = "Whisper (local)"
version     = "0.1.0"
kind        = "subprocess"
entrypoint  = "whisper-backend"
product     = "super-voice"
contract    = "v4"
license     = "Apache-2.0"
description = "Local Whisper speech-to-text."

[network]
allowed_hosts = []

[[models]]
name                   = "whisper-tiny"
role                   = "transcription"
multilingual           = true
primary_language       = "en"
supported_languages    = ["en", "es", "fr", "de", "zh"]  # abbreviated
supported_devices      = ["cpu", "gpu"]
estimated_vram_bytes   = 262144000
processing_interval_ms = 1000
files = [
    { url = "https://huggingface.co/openai/whisper-tiny/resolve/main/config.json",
      destination = "models/whisper-tiny/config.json" },
    { url = "https://huggingface.co/openai/whisper-tiny/resolve/main/tokenizer.json",
      destination = "models/whisper-tiny/tokenizer.json" },
    { url = "https://huggingface.co/openai/whisper-tiny/resolve/main/model.safetensors",
      destination = "models/whisper-tiny/model.safetensors" },
]

[[models]]
name                   = "voxtral-mini"
role                   = "transcription"
multilingual           = true
primary_language       = "en"
supported_languages    = ["en", "es", "fr", "de", "zh"]  # abbreviated
supported_devices      = ["gpu"]
estimated_vram_bytes   = 8589934592
processing_interval_ms = 2000

# Voxtral ships tekken.json instead of tokenizer.json, and multi-shard
# weights.
files = [
    { url = "https://huggingface.co/mistralai/Voxtral-Mini-3B-2507/resolve/main/config.json",
      destination = "models/voxtral-mini/config.json" },
    { url = "https://huggingface.co/mistralai/Voxtral-Mini-3B-2507/resolve/main/tekken.json",
      destination = "models/voxtral-mini/tekken.json" },
    { url = "https://huggingface.co/mistralai/Voxtral-Mini-3B-2507/resolve/main/model-00001-of-00002.safetensors",
      destination = "models/voxtral-mini/model-00001-of-00002.safetensors" },
    { url = "https://huggingface.co/mistralai/Voxtral-Mini-3B-2507/resolve/main/model-00002-of-00002.safetensors",
      destination = "models/voxtral-mini/model-00002-of-00002.safetensors" },
]
```

## Example: cloud backend (WASM)

An OpenAI backend. No model files; one egress host; one secret and one option.

```toml
[backend]
id          = "app.super-voice.openai"
source      = "github.com/super-voice/openai"
name        = "OpenAI"
version     = "0.1.0"
kind        = "wasm"
entrypoint  = "openai.wasm"
product     = "super-voice"
contract    = "v4"
license     = "Apache-2.0"
description = "OpenAI cloud transcription API."

[network]
allowed_hosts = ["api.openai.com"]

[[secrets]]
name        = "openai_api_key"
label       = "OpenAI API key"
description = "Used to authenticate requests to api.openai.com."

# No `default`: it is forbidden on `base_url`. The component carries the
# stock endpoint and treats this option as an override.
[[options]]
name        = "base_url"
label       = "API base URL"
description = "Override the API base URL, e.g. for a gateway."
type        = "string"

[[models]]
name                = "whisper-1"
role                = "transcription"
multilingual        = true
primary_language    = "en"
supported_languages = ["en", "es", "fr", "de", "zh"]  # abbreviated
supported_devices   = ["none"]

[[models]]
name                = "gpt-4o-transcribe"
role                = "transcription"
multilingual        = true
primary_language    = "en"
supported_languages = ["en", "es", "fr", "de", "zh"]  # abbreviated
supported_devices   = ["none"]
```

## Validation

- String-valued enums (`kind`, file `source`, option `type`) are
  **snake_case**; unknown values are rejected and a backend whose
  configuration fails validation is skipped during discovery rather than
  loaded with defaults.
- Whether a model is online/remote is decided solely by `supported_devices`
  (the `none` sentinel).
- An option's `choices`, when declared, must be unique, must contain the
  option's `default` if it declares one, and must not appear on a `bool`. The
  registry indexer refuses to publish a manifest breaking any of these, and
  the daemon refuses to store a value the list does not offer.
- Secret and option `name`s are **snake_case** identifiers matching
  `[a-z][a-z0-9_]*` (e.g. `openai_api_key`, `base_url`), unique within their
  table. The `name` is the wire identifier the backend reads the value by;
  `label` is the human-readable text shown beside the input in the settings
  UI. Secret values are stored encrypted; option values are stored as
  plaintext.
- `[backend].id`, when present, must match the [`id` format](#id-format)
  above; a manifest declaring a malformed `id` fails to parse, and the
  backend is skipped during discovery. It must also be unique across
  installed backends, since it names the install directory: the registry
  indexer refuses to publish an index in which two entries declare the same
  `id`, and an install whose target directory already holds a backend
  declaring a different `[backend].source` is refused rather than allowed to
  replace it.
- `[backend].source` must be unique across installed backends. When two
  installed directories declare the same `source`, the daemon serves exactly
  one of them and removes the other. The survivor is chosen in this order:
  highest `[backend].version`; then the directory named after the backend's
  own `[backend].id`, its canonical location; then the lexicographically
  first directory name, so the outcome is stable across scans. Version leads
  because a backend updated in place is the newer install whatever its
  directory happens to be called. Before the other directory is removed,
  every model file it holds that the survivor's manifest still declares at
  the same `destination` *and* the same `url` is moved across, so resolving a
  duplicate never costs a re-download. A directory whose `backend.toml` does
  not parse is never a candidate — neither to survive nor to be removed:
  without a readable `source` there is no evidence it is the same backend.
- `[backend].description` is required: a one-line, human-readable summary
  shown in the registry/Browse listing. A manifest that omits it fails to
  parse, and the backend is skipped during discovery.
- `[backend].license` is required for registry publication: a current
  OSI-approved or FSF Free/Libre SPDX identifier, or the literal `other`. The
  indexer rejects a release whose manifest omits the field or declares an
  unrecognized value. Locally installed backends may omit it.
- Each `[[assets.subprocess]]` variant declares exactly one of `file` or
  `parts`; `parts`, when used, must be non-empty and its filenames are
  concatenated in the listed order. The indexer rejects any single release
  asset — a `file` or an individual part — larger than 2 GiB (the GitHub
  release-asset limit).
- A `subprocess` backend with a non-empty `allowed_hosts` is rejected — the
  transport provides no network.
- `primary_language` must appear in `supported_languages`. When
  `multilingual` is `false`, `supported_languages` must be exactly
  `[primary_language]`.
- `supported_devices` is required and non-empty for every model. Each entry
  must be one of `cpu`, `gpu`, `none` — `cuda` and `metal` are accepted input
  spellings normalized to `gpu` — and the sentinel `none` (remote / online
  model) must be the only entry when present. A backend whose
  manifest violates any of these is skipped during discovery.
- A `subprocess` backend that declares `[capabilities] websocket = true` is
  rejected at discovery — realtime WebSocket support is wasm-only.
- Any model entry with `realtime = true` in a backend whose
  `[capabilities] websocket` is `false` or absent is rejected at discovery.
- Every model declares `role`, one of `transcription`, `post_processor` or
  `synthesis`. A missing or unrecognized role fails to parse and the backend
  is skipped during discovery (a typo'd role would otherwise silently put the
  model in the wrong place). A model declaring `role = "post_processor"`
  together with `realtime = true` is rejected at discovery, and the registry
  indexer refuses such a release.
- A model may declare only the keys of its own feature: a text-to-speech key
  on a speech-to-text model, or `force_preview_support` on a synthesis model,
  is refused.
- On a synthesis model: `voice_kinds` must be non-empty; voice `id`s must be
  unique; a model with `voices` must name one of them as `default_voice`; a
  voice's `language` must be in `supported_languages`; `clone_ref_seconds` is
  required with `cloned` in `voice_kinds`, forbidden without it, and must be
  positive; `clone_needs_transcript` requires `cloned`; `max_input_chars` must
  be non-zero; `output_sample_rate` must be within 8000–192000.
- `[capabilities] streaming_input = true` is refused: the path it would opt
  into does not exist yet.
