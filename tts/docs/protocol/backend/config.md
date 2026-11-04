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

A JSON Schema for this file is generated from the manifest types Super TTS
shares with Super STT in
[`super-engine-spec`](https://github.com/super-libre/super-engine), plus Super
TTS's own fields in `super-tts-registry-types`, and published to GitHub Pages
by CI (it is not committed to the repo). Backends in other repositories reference it at
`https://jorge-menjivar.github.io/super-tts/backend.schema.json`.
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
├── kokoro-backend          # entrypoint (subprocess binary) …
│                           # … or kokoro.wasm (WASM component)
└── models/                 # populated by the daemon at load time
    └── kokoro-82m/
        ├── config.json
        └── kokoro-v1_0.pth
```

The daemon never writes outside a backend's own directory, and a backend
reads only from it. Files a model needs are downloaded by the daemon into
the `dest` paths declared in the configuration (see
[`[[models.files]]`](#modelsfiles)).

## `[backend]`

Backend identity and packaging.

```toml
[backend]
source      = "github.com/super-tts/kokoro"
name        = "Kokoro (local)"
version     = "0.1.0"
kind        = "subprocess"
entrypoint  = "kokoro-backend"
product     = "super-tts"
contract    = "v1"
license     = "Apache-2.0"
description = "Local Kokoro text-to-speech."
```

| Field        | Type   | Required        | Notes                                                                 |
|--------------|--------|-----------------|-----------------------------------------------------------------------|
| `id`         | string | for publication | Globally unique reverse-DNS identifier for the backend, e.g. `app.super-tts.kokoro`. Names the directory the backend is installed into. Required for a backend to be listed in the registry. |
| `source`     | string | yes             | Canonical repository id for this backend. Becomes the `source` of every model it provides (see [identity](./contract.md#model-identity)). Must be unique across installed backends. |
| `name`       | string | yes             | Human-readable display name.                                          |
| `version`    | string | yes             | Backend version (semver).                                            |
| `kind`       | string | yes             | `subprocess` or `wasm` — selects the transport.                       |
| `entrypoint` | string | yes             | Path, relative to the backend directory, to the executable (`subprocess`) or the `.wasm` component (`wasm`). |
| `product`    | string | yes             | The product the backend is built for: `super-tts`. A manifest naming any other product is refused. See [the product](#the-product). |
| `contract`   | string | yes             | The [contract generation](#contract-generations) the backend implements. `v1` is the only one so far. Declare the lowest generation whose fields you use; a generation this build does not know is rejected. |
| `license`    | string | for publication | SPDX identifier of a current OSI-approved or FSF Free/Libre license (e.g. `Apache-2.0`, `MIT`, `GPL-3.0-only`), or the literal `other` for a license outside that set. Required for registry publication; optional for locally installed backends. |
| `description`| string | yes             | One-line, human-readable summary, shown under the backend's name in the Library, the Browse listing and the manual-install preview. Must not be empty or only whitespace: for a backend the registry does not list it is the only description the app has. |

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
`kokoro`: `app.super-tts.kokoro` and `com.example.kokoro` coexist.

`id` names the install directory. It is not part of model identity, which is
the `(name, source)` pair described in [contract.md](./contract.md).

#### The product

Super TTS and Super STT read the same manifest format, and both number their
contracts from `v1`. Each ignores the keys only the other reads, so without
`product` nothing in a manifest tells their backends apart. A manifest naming
any product but `super-tts` is refused, including one already installed:

```
this backend is built for super-stt, not super-tts
```

Super STT published backends before the field existed, so it reads a manifest
without one as its own. That is why Super TTS requires it: a manifest that
leaves it out would install into Super STT.

### Contract generations

`contract` is the one thing a manifest says about what it needs from Super
TTS. A generation names a set of manifest fields and backend routes, and each
generation **extends** the one before rather than replacing it: a later
generation is everything the earlier one defines, plus what it adds. Declare
the lowest generation whose fields you use.

| Generation | Covers                                                                                                                                             | First supported by |
|------------|------------------------------------------------------------------------------------------------------------------------------------------------------|--------------------|
| `v1`       | The contract as this document describes it: discovery from `backend.toml`, `POST /v1/load`, `POST /v1/synthesize`, `POST /v1/cancel`, the cloned-voice registration routes for models that declare them, and [`POST /v1/prepare`](./contract.md#post-v1prepare) for models that declare [`prepare_files`](#models). | Super TTS 0.1.0    |

`v1` is currently the only generation, so every manifest declares it and there
is no second row to choose between. The machinery below is described anyway,
because its whole value is that it is already in place: it is what will let a
`v2` be introduced without every Super TTS released before it having to be
taught something first.

Extending the contract does not oblige a backend to serve all of it. Which
routes a backend must implement is decided by the models it declares, not by
its generation: a backend whose models are all `preset` or `described` never
receives [`POST /v1/voices`](./contract.md#post-v1voices) and need not
implement it.
The generation says what the daemon may *expect to find*; the models say what
it will actually call.

You never write a Super TTS version. The generation implies one, and the
registry carries it: the indexer stamps every index entry with `min_client`,
the release that first understood that entry's generation. It is **reported,
never compared** — a daemon decides compatibility by whether it can parse the
generation at all, so a prerelease of the release named there (`0.2.0-beta.1`,
which semver orders *below* `0.2.0`) is not wrongly locked out. It is there so
that a client meeting a backend it cannot install can name the version to
update to, instead of showing only a failure to parse.

The refusal itself needs no field. `contract` is a closed set on the reading
side, so a build that predates a generation cannot parse a manifest declaring
it, and says which ones it does know:

```
unknown contract `v2`; this build knows v1
```

It is the *absence* of knowledge that refuses. That is what makes every
generation added in future gate itself against every Super TTS already
released — this one included — with no `min_version` field anyone had to think
to add first.

Which generation introduced, or began requiring, each manifest field lives in
one table (`Tts::CONTRACT_FIELDS`, in `super-tts-registry-types`). Its one row
so far is `v1` requiring [`[backend].product`](#the-product). Adding a field
to a `v2` means adding a row, and both the manifest parser and the published
`backend.schema.json` learn the rule from that row without being taught
separately, so an editor bound to the schema flags exactly what the parser
would refuse, before anything is released.

A row expresses one of two rules. A field **added** in a generation may not be
spelled under an older one:

```
`[[models]].example_field` requires `contract = "v2"`, but this manifest declares
`contract = "v1"` — raise the contract to use the field, or remove the field to
stay on `v1`
```

Both fixes are named because they are not equivalent. Raising the contract is
right for a manifest that means to use the field, but it also raises the
release floor for everyone installing the backend; an author who wrote the
field's *default* value out by hand wants the other fix. Spelling a newer
field with its default value still counts as declaring it — the rule is about
what the document says, not about what the parsed struct ends up holding.

A field a generation **requires** is the mirror image, for closing an
optionality that only survived for backward compatibility:

```
`contract = "v2"` requires `[backend].example_field`, which this manifest does not
declare — add it, or drop to `contract = "v1"` where it is optional
```

Only field *names* are versioned this way. A generation that widens an
existing field's value set instead — a new `voice_kinds` entry, a new device —
cannot be expressed in that table, and is caught by that field's own parser.

These rules apply to a manifest being **admitted**: a registry install, a
custom repo, an import from a directory, and the indexer itself. A backend
already installed on this machine is parsed with them switched off. Their job
is to stop a manifest getting in, and that one is already in — enforcing them
at discovery would make a backend that installed cleanly under an earlier
build vanish from the catalog, taking its downloaded models out of reach, over
a file the user did not write and cannot be expected to edit.

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
value through the settings UI; the daemon stores it encrypted in the system
keyring and never writes it to disk in plaintext.

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
name        = "output_format"
label       = "Output format"
description = "Container the provider returns audio in."
type        = "string"
default     = "wav"
choices     = ["wav", "mp3", "opus"]

[[options]]
name        = "temperature"
label       = "Temperature"
description = "How freely the model samples."
type        = "float"
default     = 0.9
min         = 0.6
max         = 1.2
step        = 0.1
```

A `bool` option is a switch: its two values are named by its type, so there is
nothing else to offer. Every other option is a free-text field unless it says
otherwise.

### Types

The daemon stores every option as text, because an `x-tts-option-*` header is
text. `type` is the claim about what that text parses back to, and the daemon
keeps the claim: `POST /v1/backend/{backend_id}/option/{name}` answers
`400 invalid_value` for a value that is not of the declared type, naming the
type rather than listing choices — a `warm` typed into a `float` is a different
mistake from a value that is merely off the dropdown, and saying "accepts one
of:" to the first would leave the reader to infer why.

`string` accepts anything, which is what makes it the default: an option that
declares no type is free text and always was. `integer` takes what parses as a
64-bit signed integer, `bool` takes exactly `true` or `false`, and `float`
takes what parses as a double — except the infinities and not-a-number, which
parse but are the values that turn an arithmetic bug into a silent one.

A `float` is for a setting whose steps are not whole: a sampling temperature, a
gain, a threshold. Declaring it `string` and parsing it in the backend is the
same thing with the check moved to the far side of the wire, where the only way
left to report the problem is to fail a synthesis the user already asked for.

The type binds the manifest's own values too. A `default` or a `choices` entry
that is not of the declared type is refused at publication, since it names a
value the daemon would then refuse to store.

### Ranges and sliders

A numeric option can declare bounds. `min` and `max` are inclusive, and the
daemon keeps them the way it keeps `choices`: a write outside them is
`400 invalid_value`, naming the range.

`step` is the increment the value moves in, and declaring all three is what
makes the option a **slider** — the same shape-implies-control rule that makes
a `choices` option a dropdown and a `bool` a switch. An option bounded at both
ends with a declared grid has nothing left to type; one missing any of the
three still does, and gets a field.

The grid belongs to the control, not to the contract. The daemon enforces
`min` and `max` and ignores `step`, because a value between two notches is
still one the option said it could take — refusing it would make the option
narrower than its own bounds say, and would turn every float that is not
exactly representable into a bug report. A client that renders the slider is
what keeps to the grid.

The rules, all checked at publication:

- `min`, `max` and `step` are only for `integer` and `float`. A range over
  strings is not a thing the manifest can mean.
- `min <= max`, and every bound is finite.
- `step` is greater than zero, no larger than the range it divides, and needs
  both `min` and `max` — a slider with one end has nothing to slide between.
- An `integer` option's bounds are whole numbers, or its slider produces
  values the option itself refuses.
- A `default` is inside the range.
- An option declares `choices` **or** a range, never both: a closed set is
  already the values it accepts, and a client cannot render a dropdown and a
  slider at once.

| Field  | Type      | Notes                                                          |
|--------|-----------|----------------------------------------------------------------|
| `min`  | number    | Lowest value accepted, inclusive. Numeric options only.        |
| `max`  | number    | Highest value accepted, inclusive. Numeric options only.       |
| `step` | number    | The increment the control moves in. With `min` and `max`, renders a slider. Not enforced. |

`choices` is how it says otherwise. Declaring it means the option accepts a
closed set, and the client renders a dropdown over that set instead of a text
box. Declare it whenever the option has one. Writing the allowed values into
the `description` instead leaves the user a text field, and a value that is
merely close enough — `mp3 ` with a trailing space, or an `ogg` the backend
never had — is stored and injected into the load headers as though the backend
knew it.

The daemon refuses a write of any value the list does not offer, so `choices`
is a contract and not only a hint:
`POST /v1/backend/{backend_id}/option/{name}` answers `400 invalid_value`,
naming what is on offer. The settings dropdown is not the only thing that
writes there, and the stored value has to stay one the backend understands.
Clearing an override is still `DELETE`, whatever the option's shape.

A stored value that is no longer offered — a backend that dropped a choice its
user had picked — selects nothing rather than the nearest row, so the user
picks again knowingly instead of being silently moved.

Omitting `choices` is the open-ended option, which is what every manifest
written before the field, and every genuinely free-form option like
`base_url`, already is.

| Field         | Type           | Required | Notes                                                  |
|---------------|----------------|----------|--------------------------------------------------------|
| `name`        | string         | yes      | snake_case identifier the backend reads the value by. `[a-z][a-z0-9_]*`, unique within the table. |
| `label`       | string         | no       | Human-readable label shown in the settings UI. Falls back to `name` when absent. |
| `description` | string         | yes      | Help text shown beside the input in the settings UI.   |
| `type`        | string         | no       | `string`, `integer`, `float`, or `bool`. Drives the input the UI renders — `bool` gets a switch, an option with `choices` a dropdown, everything else a text field — and what the daemon accepts as a value. Default `string`. |
| `default`     | matches `type` | no       | Value used when the user sets none. Forbidden on `base_url` — see below. When `choices` is present, must be one of them. |
| `choices`     | array of `type` | no      | The values this option accepts. Renders a dropdown, and the daemon refuses to store anything else. Omit it for an open-ended option. Entries must be unique, and a `bool` must not declare any. |
| `required`    | bool           | no       | Whether a value must be set before the backend can load. Default `false`. |

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
[`PUT .../option/base_url`](../endpoints/v1/backends/options.md).

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
That is what the missing `x-tts-option-base_url` header means when the user has
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
[`POST /backends/{source}/options/{name}`](../endpoints/v1/backends/options.md)
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
file   = "kokoro-x86_64-unknown-linux-gnu-cpu.tar.gz"
target = "x86_64-unknown-linux-gnu"
accel  = "cpu"

[[assets.subprocess]]
file       = "kokoro-x86_64-unknown-linux-gnu-cuda12-sm75.tar.gz"
target     = "x86_64-unknown-linux-gnu"
accel      = "cuda"
cuda_major = 12
cuda_sm    = 75

[[assets.subprocess]]
file       = "kokoro-x86_64-unknown-linux-gnu-cuda12-cudnn-sm75.tar.gz"
target     = "x86_64-unknown-linux-gnu"
accel      = "cuda"
cuda_major = 12
cuda_sm    = 75
cudnn      = true

[[assets.subprocess]]
file   = "kokoro-x86_64-unknown-linux-gnu-rocm.tar.gz"
target = "x86_64-unknown-linux-gnu"
accel  = "rocm"
gfx    = ["gfx1030", "gfx1100", "gfx1101"]

[[assets.subprocess]]
file       = "kokoro-x86_64-unknown-linux-gnu-vulkan.tar.gz"
target     = "x86_64-unknown-linux-gnu"
accel      = "vulkan"
vulkan_api = "1.3"

# A build carrying more than one runtime — `accel` as an array — matches
# either. Host matching then considers every accelerator listed.
[[assets.subprocess]]
file       = "kokoro-x86_64-unknown-linux-gnu-cuda-rocm.tar.gz"
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
| `accel` | string or array of strings | yes | A single value, or a non-empty array, drawn from `"cpu"`, `"cuda"`, `"rocm"`, `"metal"`, `"vulkan"`. An array declares one build carrying more than one runtime (e.g. a llama.cpp binary built with both CUDA and HIP support); the build then matches a host that satisfies any of the accelerators listed. |
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
    "xtts-x86_64-unknown-linux-gnu-cuda13.tar.gz.part00",
    "xtts-x86_64-unknown-linux-gnu-cuda13.tar.gz.part01",
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
```

| Field       | Type | Required | Notes                                                                              |
|-------------|------|----------|------------------------------------------------------------------------------------|
| `websocket` | bool | no       | Opt into the `super-engine:realtime/ws` import and the `super-engine:realtime/ws-server` export (see [wasm.md — Realtime](./wasm.md#realtime-websocket)). When `true`, the daemon wires those interfaces into the WASM component for every session on a realtime model. **wasm-only** — a `subprocess` backend declaring `websocket = true` is rejected at discovery. Default `false`. |

## `[[models]]`

One entry per model the backend provides. Each model is identified on the
wire by `(name, source)`, where `source` is the `[backend].source`
above.

```toml
[[models]]
name                 = "kokoro-82m"
multilingual         = true
primary_language     = "en"
supported_languages  = ["en", "es", "fr", "it", "pt"]  # abbreviated
supported_devices    = ["cpu", "gpu"]
estimated_vram_bytes = 419430400
max_input_chars      = 500
output_sample_rate   = 24000
default_voice        = "af_heart"

[[models.voices]]
id       = "af_heart"
label    = "Heart"
language = "en"
```

| Field                    | Type            | Required | Notes                                                            |
|--------------------------|-----------------|----------|------------------------------------------------------------------|
| `name`                   | string          | yes      | Wire model name.                                                 |
| `multilingual`           | bool            | no       | Whether the model accepts more than one language. Default `true`. When `true`, `POST /v1/synthesize` accepts a `language` from `supported_languages`. |
| `primary_language`       | string          | yes      | Default language code (e.g. `en`); used when `language` is omitted. |
| `supported_languages`    | array of string | yes      | Language codes the model accepts; must include `primary_language`. When `multilingual` is `false`, it is exactly `[primary_language]`. |
| `supported_devices`      | array of string | yes      | Whether the model can use an accelerator at all — which accelerator an installed build actually targets is a property of the [asset](#assets), not the model. Non-empty, drawn from `["cpu", "gpu", "none"]`. `"cuda"` and `"metal"` are accepted input spellings for `"gpu"`; the daemon normalizes them and never emits them. `"none"` is the sentinel for remote/online models and must be the only entry when present. |
| `estimated_vram_bytes`   | integer         | no       | Conservative GPU memory estimate. Default `0`; use `0` for cloud models. |
| `processing_interval_ms` | integer         | no       | Suggested minimum interval between streaming passes, in ms.      |
| `realtime`               | bool            | no       | When `true`, the model is driven over the realtime WebSocket transport rather than batch `POST /v1/synthesize`. Requires `[capabilities] websocket = true`. Default `false`. **Reserved**: the transport works but no session payload contract is defined yet, so such a model is not reachable by a client — see [contract.md](./contract.md#realtime-sessions-reserved). |
| `max_input_chars`        | integer         | no       | Longest `text` the model accepts in one `POST /v1/synthesize`. Absent means unbounded: the daemon sends whole utterances and never splits for length. When set, the daemon chunks on sentence boundaries to stay under it. Must be non-zero. |
| `output_sample_rate`      | integer         | no       | Native output rate in Hz, e.g. `24000`. **Advisory** — the authoritative rate is the `x-tts-sample-rate` response header on each synthesis, since a manifest cannot know what a cloud provider will actually return. It lets the settings UI show a rate and the daemon pre-size buffers before the first response. Must be 8000–192000. |
| `default_voice`          | string          | no       | Voice used when a request omits `voice`. **Required** when `voices` is non-empty, and must name one of them. |
| `voice_kinds`            | array of string | no       | Which `voice` id shapes this model accepts, from `["preset", "cloned", "described"]`. Default `["preset"]`. Non-empty. The daemon refuses a shape the model did not opt into, so a backend never sees an id it cannot resolve. |
| `clone_ref_seconds`      | number          | no       | Longest reference audio accepted for a cloned voice, in seconds. **Required** when `voice_kinds` contains `cloned`, forbidden otherwise. Must be positive. The daemon trims a longer recording to this before registering it, so the value is a budget rather than a filter. |
| `clone_needs_transcript` | boolean         | no       | Whether registering a cloned voice also requires the reference clip's transcript. Default `false`; forbidden without `cloned` in `voice_kinds`. Set it for in-context cloning, which conditions on the words as well as the audio — the daemon then refuses to register a clip stored without one, instead of letting every synthesis fail. Speaker-embedding cloning leaves it unset. |
| `voices`                 | array of table  | no       | Preset voices the model provides — see [`[[models.voices]]`](#voices). Empty for models whose voices are entirely cloned or described. |
| `prepare_files`          | array of string | no       | The files the model's [prepare](./contract.md#post-v1prepare) reads, each a `destination` from the model's [`files`](#modelsfiles). The daemon downloads these first, starts the backend and sends `POST /v1/prepare`, then downloads the rest while the backend builds its kernels. Declaring it promises the backend serves `POST /v1/prepare` for this model, and that the prepare reads only these files. An entry that is not one of the model's `destination`s is refused. Default empty: no prepare. |
| `provider`               | string          | no       | Compatibility field. Not part of model identity and read by nothing in the daemon; it is echoed back verbatim as `provider` in [`POST /v1/load`](./contract.md#post-v1load) so a backend that still validates it keeps loading. |

> **Compatibility.** `provider` was part of model identity before it became
> `(name, source)`. Backends released against the earlier contract compare the
> `provider` in `POST /v1/load` against their own fixed value and answer
> `400 invalid_model` on a mismatch, so a manifest declaring `provider` still
> has it forwarded on load. New backends should omit it, and should not
> validate it if they accept it.

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

`multilingual`, `primary_language`, and `supported_languages` together
describe language capability. When `multilingual` is `true`,
`POST /v1/synthesize` may carry a `language`, which must be one of
`supported_languages`; when omitted, `primary_language` is used. When
`multilingual` is `false`, the model speaks only `primary_language`.

`supported_devices` declares whether the model can use an accelerator at all.
It says nothing about *which* accelerator — CUDA, ROCm, Vulkan — since one
asset can serve several models and one of them may have no GPU path; that
detail lives on the asset that ends up installed (see
[`[[assets.subprocess]]`](#assets)) and is reported per-backend as
`installed_accel` (see [`GET /backends`](../endpoints/v1/backends.md)). The
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
    { url = "https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/config.json",
      destination = "models/kokoro-82m/config.json" },
    { url = "https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/kokoro-v1_0.pth",
      destination = "models/kokoro-82m/kokoro-v1_0.pth", sha256 = "9f86d0…" },
]
```

The block form is identical TOML and may be used instead:

```toml
[[models.files]]
url         = "https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/config.json"
destination = "models/kokoro-82m/config.json"
```

| Field         | Type   | Required | Notes                                                                |
|---------------|--------|----------|---------------------------------------------------------------------|
| `url`         | string | yes      | Full download URL for the file. Any host.                            |
| `destination` | string | yes      | Relative file path (including filename) under the backend directory. Also the variant key — see below. |
| `sha256`      | string | no       | Expected SHA-256, hex-encoded; verified after download.              |

`destination` must be a relative path that stays inside the backend directory:
absolute paths, `..` traversal, and backslashes are rejected.

#### Per-architecture variants

An entry may also carry a **host selector**, written in the same vocabulary
[`[[assets.subprocess]]`](#assets) uses for a build. Entries sharing a
`destination` are then variants of one file: the daemon scores each against the
machine, downloads the best match, and leaves the rest alone. The backend reads
a fixed path and never learns which variant it got.

| Field        | Type            | Notes                                                                                      |
|--------------|-----------------|--------------------------------------------------------------------------------------------|
| `accel`      | string or array | Acceleration families this variant is for. Absent — the default — matches every host.        |
| `cuda_major` | integer         | Matches a host whose installed CUDA runtime is at least this. Requires `cuda` in `accel`.    |
| `cuda_sm`    | integer or array | Compute capabilities this variant covers, e.g. `90` or `[86, 89]`. Requires `cuda`.         |
| `gfx`        | array of string | AMD architecture targets, in `--offload-arch` spelling. Requires `rocm`.                     |
| `vulkan_api` | string          | Minimum Vulkan API version, e.g. `1.3`. Requires `vulkan`.                                   |
| `optional`   | bool            | Whether the model can load without this destination. Default `false`. Must agree across the variants of one destination. |

`cuda_sm` is a list where an asset's is a single value, because the two answer
different questions. A build that omits it is a fat binary with PTX behind it,
so "any capability" is a true claim and enumerating is rarely useful. A file has
no JIT to fall back on — kernels compiled for `sm_90` are inert on `sm_86` — but
one file may still carry entries for several devices, which is exactly what a
pre-warmed kernel cache is. Saying so once beats declaring the same URL and hash
under each. The bare number and the list are the same field: `cuda_sm = 90` and
`cuda_sm = [90]` mean the same thing, as `accel` already works.

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
preference — what is on disk must not change when the user toggles between CPU
and GPU. Candidates are ranked by accel family first (a native runtime over
Vulkan over `cpu` over an entry with no selector at all), then by that family's
own discriminators, where an exact `cuda_sm` or `gfx` beats a variant that
matched only the family. Ties go to the first declared, so declaration order is
your preference order. It is the same ranking that picks a build, applied to the
data beside it.

**When nothing matches.** A destination whose variants all miss the host is a
hole, and `optional` says which kind. Weights are load-bearing, so the default
(`false`) fails the load, naming what the host offered and what the variants
wanted — better than a backend erroring on a file it was never handed. A
pre-warmed kernel cache is not: `optional = true` skips the destination, and the
model loads without it and rebuilds what it needs.

## Example: local backend (subprocess)

A Kokoro backend providing two models, loaded from Hugging Face. Kokoro ships
its weights as a single `.pth` plus a voice pack per speaker.

```toml
[backend]
source      = "github.com/super-tts/kokoro"
name        = "Kokoro (local)"
version     = "0.1.0"
kind        = "subprocess"
entrypoint  = "kokoro-backend"
product     = "super-tts"
contract    = "v1"
license     = "Apache-2.0"
description = "Local Kokoro text-to-speech."

[network]
allowed_hosts = []

[[models]]
name                 = "kokoro-82m"
multilingual         = true
primary_language     = "en"
supported_languages  = ["en", "es", "fr", "it", "pt"]  # abbreviated
supported_devices    = ["cpu", "gpu"]
estimated_vram_bytes = 419430400
max_input_chars      = 500
output_sample_rate   = 24000
default_voice        = "af_heart"
files = [
    { url = "https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/kokoro-v1_0.pth",
      destination = "models/kokoro-82m/kokoro-v1_0.pth" },
    { url = "https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/config.json",
      destination = "models/kokoro-82m/config.json" },
]

[[models.voices]]
id       = "af_heart"
label    = "Heart"
language = "en"
tags     = ["warm", "female"]

[[models.voices]]
id       = "am_puck"
label    = "Puck"
language = "en"
tags     = ["bright", "male"]

[[models]]
name                 = "xtts-v2"
multilingual         = true
primary_language     = "en"
supported_languages  = ["en", "es", "fr", "de", "zh"]  # abbreviated
supported_devices    = ["gpu"]
estimated_vram_bytes = 4294967296
output_sample_rate   = 24000

# This one clones as well as presetting, so it declares both kinds and the
# reference-audio budget that `cloned` requires.
voice_kinds       = ["preset", "cloned"]
clone_ref_seconds = 30.0
# This model derives a speaker embedding from the audio alone, so it does not
# set `clone_needs_transcript`; a model that clones in-context would.
default_voice     = "en_sample"

files = [
    { url = "https://huggingface.co/coqui/XTTS-v2/resolve/main/model.pth",
      destination = "models/xtts-v2/model.pth" },
    { url = "https://huggingface.co/coqui/XTTS-v2/resolve/main/config.json",
      destination = "models/xtts-v2/config.json" },
    { url = "https://huggingface.co/coqui/XTTS-v2/resolve/main/vocab.json",
      destination = "models/xtts-v2/vocab.json" },
]

[[models.voices]]
id       = "en_sample"
label    = "English (built-in)"
language = "en"
```

## Example: cloud backend (WASM)

An OpenAI backend. No model files; one egress host; one secret and one option.

```toml
[backend]
source      = "github.com/super-tts/openai"
name        = "OpenAI"
version     = "0.1.0"
kind        = "wasm"
entrypoint  = "openai.wasm"
product     = "super-tts"
contract    = "v1"
license     = "Apache-2.0"
description = "OpenAI cloud text-to-speech API."

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
name                = "gpt-4o-mini-tts"
multilingual        = true
primary_language    = "en"
supported_languages = ["en", "es", "fr", "de", "zh"]  # abbreviated
supported_devices   = ["none"]
output_sample_rate  = 24000
default_voice       = "alloy"

# This model takes free-text delivery guidance as well as a named voice, so
# it declares `described` alongside `preset`.
voice_kinds = ["preset", "described"]

[[models.voices]]
id       = "alloy"
language = "en"

[[models.voices]]
id       = "shimmer"
language = "en"

[[models]]
name                = "tts-1-hd"
multilingual        = true
primary_language    = "en"
supported_languages = ["en", "es", "fr", "de", "zh"]  # abbreviated
supported_devices   = ["none"]
output_sample_rate  = 24000
default_voice       = "alloy"

[[models.voices]]
id       = "alloy"
language = "en"
```

## Validation

- String-valued enums (`kind`, file `source`, option `type`) are
  **snake_case**; unknown values are rejected and a backend whose
  configuration fails validation is skipped during discovery rather than
  loaded with defaults.
- Whether a model is online/remote is decided solely by `supported_devices`
  (the `none` sentinel).
- An option's `choices`, when declared, must be unique, must contain the
  option's `default` if it declares one, and must not appear on a `bool` — a
  switch already names its two values. The registry indexer refuses to publish
  a manifest breaking any of these, and the daemon refuses to store a value
  the list does not offer (`400 invalid_value`). Enforced on both sides of
  publication because the indexer only sees what is published, and the write
  can come from anything holding a `settings` token.
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
