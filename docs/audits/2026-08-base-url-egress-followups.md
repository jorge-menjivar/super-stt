# Deferred follow-ups — user-authorized `base_url` egress (August 2026)

Items surfaced while reviewing PR #343 (`feat(daemon): authorize user-set base_url host
for wasm backend egress`) and the follow-up work on branch
`review/base-url-egress-classified`, and deliberately **not** fixed there because each
belongs to its own change. Everything else from both review passes is resolved on that
branch.

File:line references are to the tree at `70149cc7` plus that branch's working tree.
Checkboxes track resolution. Severity: 🔴 high · 🟠 moderate · 🟡 minor.

Context for all three: a WASM backend's outbound reach is the manifest's
`[network].allowed_hosts` (fully SSRF-guarded) plus the one `host:port` a user names in
the backend's `base_url` option (guard relaxed for that authority only). See
[`docs/protocol/backend/config.md`](../protocol/backend/config.md#base_url-and-egress).

---

### [x] 1. 🟠 Egress guard: IPv6 forms that embed an IPv4 destination are classified as public

- **Where:** `super-stt-daemon/src/stt_models/wasm/host.rs` — `is_never_routable_ip` and
  `is_local_ip` normalized IPv6 with `Ipv6Addr::to_ipv4_mapped()`.
- **Problem:** that covers `::ffff:0:0/96` only. Three other encodings reach the same v4
  host and fell through every branch — not never-routable, not local, therefore permitted
  under `EgressScope::PublicOnly`: the deprecated compatible form (`::a.b.c.d`), NAT64
  (`64:ff9b::a.b.c.d`, RFC 6052), and 6to4 (`2002:a.b.c.d::`, RFC 3056).
- **Impact — measured, not assumed.** On an ordinary dual-stack host the compatible and
  6to4 forms do not route: a connect to `::127.0.0.1` times out where `::1` connects. The
  case that matters is **NAT64**, which *is* routed on IPv6-only networks (common on
  mobile and enterprise). There, `64:ff9b::c0a8:101` reaches `192.168.1.1`, so a backend
  controlling a DNS record for a host it declared in `allowed_hosts` could reach RFC1918
  destinations while the guard considered the address public. A private-range bypass on
  those networks — not the loopback bypass this entry first claimed, which does not
  reproduce.
- **Predates this work.** The guard's previous `is_disallowed_ip` had the identical
  `to_ipv4_mapped`-only normalization.
- **Resolved (PR #349):** an `embedded_v4` helper normalizes all four forms and both
  classifiers judge the result by the v4 rules. Two ordering properties are tested: the
  IPv6 checks run *first*, because `to_ipv4()` renders `::1` as `0.0.0.1` and `::` as
  `0.0.0.0` and converting first would stop recognizing loopback as loopback; and embedded
  addresses are classified rather than refused, because a public v4 behind NAT64 is how an
  IPv6-only network reaches the v4 internet at all.

### [ ] 2. 🟡 Egress hooks: both allowlists are deep-cloned on every component invocation

- **Where:** `super-stt-daemon/src/stt_models/wasm/mod.rs` — `WasmBackend::allowlist_hooks()`
  clones `allowed_hosts` and `user_allowed_hosts` into a fresh `AllowlistHooks`; called
  once per batch transcription and once per realtime session.
- **Problem:** the lists are immutable for the backend's lifetime and the hooks only ever
  read them, so each invocation allocates two `Vec<String>` plus one `String` per entry to
  hand the guard data it will not modify.
- **Impact:** small and per-request rather than per-frame; this is allocation hygiene on a
  hot-ish path, not a correctness or latency problem anyone has measured.
- **Predates this work** in the single-list form; PR #343 doubled it by adding the second
  list.
- **Fix:** hold the lists as `Arc<[String]>`, or gather both plus `allow_loopback` into an
  `Arc<EgressPolicy>` that `AllowlistHooks` carries, reducing construction to refcount
  bumps. Keep a single construction point so the batch and realtime paths cannot drift
  about which list is which — that property is pinned by
  `egress_lists_reach_the_hooks_in_their_own_slots` in `super-stt-daemon/tests/wasm_mock.rs`.

### [ ] 3. 🟠 Protocol: the sandbox relaxation is keyed on a magic option *name*, with no typed declaration and no diagnostic

- **Where:** `super_stt_registry_types::manifest::BASE_URL_OPTION` is the single
  definition, resolved by the daemon's egress derivation, discovery's scrub, the indexer's
  publish check, the generated JSON Schema conditional, and the app's Cloud chip.
- **Problem:** a backend declares its configurable endpoint by *naming an option
  `base_url`*. Nothing in the format expresses "this option's value authorizes egress", so
  an author who names it `endpoint`, `api_base`, or `server_url` — all natural, none
  rejected by the parser, the schema, or the indexer — gets a backend whose user-set
  endpoint is never authorized.
- **Impact:** the user sets the option in the settings UI, the daemon injects
  `x-stt-option-<name>`, egress derivation returns nothing because no option is named
  `base_url`, and every transcription fails with `outbound host not allowed: …`. Nothing
  in the log, the UI, or the error text points at the naming convention.
- **Fix (design, not a patch):** give the manifest a typed declaration — an `egress = true`
  flag on the option, or a `[network].user_endpoint_option` key — so the schema, the
  indexer, the daemon, and the app agree through a field rather than a shared string. That
  is a `backend.toml` contract change: it needs a docs-first definition in
  `docs/protocol/backend/config.md`, a schema conditional, and a decision about whether
  `base_url` stays recognized as the legacy convention. Until then, the convention is only
  documented, not enforced, and the failure mode above stays silent — a cheap interim
  mitigation is a `warn!` at discovery when a backend declares no `base_url` option but
  does declare something that looks like an endpoint.

---

## Decided, not deferred

- **No write-time validation of a `base_url` value.** `POST /backends/{source}/options/{name}`
  trims the value and rejects it when empty, but does not check that it parses into a
  host; a malformed value is accepted and surfaces later as a failed transcription. This
  was raised and deliberately kept as-is: validation would catch garbage but not the case
  that actually misleads people (a well-formed URL pointing at the wrong port), and the
  daemon already logs the unreadable case at model load. Revisit only alongside a
  settings-UI change that can show the error usefully.
