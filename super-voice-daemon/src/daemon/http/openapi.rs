// SPDX-License-Identifier: GPL-3.0-only
//! The `OpenAPI` document for the daemon protocol.
//!
//! The document is generated from the router, not written beside it: every
//! `/v1` route is registered through [`utoipa_axum::routes!`], which reads the
//! `#[utoipa::path]` attribute on the handler it points at. A route and its
//! documentation are therefore one declaration — adding a route without
//! documenting it does not compile, and changing a path changes both.
//!
//! `just openapi` writes the result to `docs/protocol/openapi.json`;
//! `just openapi-check` fails when the committed file is stale, so a protocol
//! change cannot merge without the published spec moving with it.
//!
//! The prose reference under `docs/protocol/` is not replaced by any of this.
//! It explains *when* to call an endpoint and how the pieces fit; the spec
//! states the shapes exactly, for tooling and for a client generator.

use utoipa::Modify;
use utoipa::OpenApi;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};

/// Base document: everything that is true of the protocol as a whole rather
/// than of one endpoint. The paths and schemas are filled in from the router
/// (see [`super::v1::openapi`]).
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Super Voice daemon protocol",
        description = "\
HTTP/1.1 + JSON on two transports that serve the identical API.

One daemon serves both of Super Voice's features. What is speech to text's own is under \
`/v1/stt`, what is text to speech's own under `/v1/tts`, and what the daemon has once \
— auth, events, status, backends, the registry, settings, updates — at `/v1`.

**A Unix domain socket** under the per-user runtime directory at \
`<runtime dir>/voice/super-voice-http.sock` (override with `SUPER_VOICE_HTTP_SOCKET`), which is \
what native clients use. The runtime directory is `$XDG_RUNTIME_DIR` on Linux and the \
Darwin per-user temp directory on macOS — ask the daemon rather than assuming, or read \
the `runtime_dir` server variable below. Its filesystem permissions are the first layer \
of access control, and the daemon reads the peer credentials on each connection to \
identify the calling binary. It is the second server below, listed by its real path — \
dial that path directly and send whatever `Host` you like, since the daemon ignores it.

**A loopback TCP listener**, which is what a browser can reach, since no browser can dial \
a Unix socket. It is the first server below, and it is a real address. There are no peer \
credentials on TCP, so a caller there is identified by its `Origin` instead — a claim from \
the browser rather than a fact from the kernel, which is why the consent dialog says so.

Every endpoint except `POST /v1/auth/request` requires `Authorization: Bearer <token>`. \
A token is minted only after the user approves your client in a consent dialog, and is \
bound to whichever identity was approved — a binary or an origin, never interchangeable. \
A client cannot widen its own permissions. See `docs/protocol/auth.md`.

With curl, over the socket:

```
curl --unix-socket \"${XDG_RUNTIME_DIR:-$TMPDIR}/voice/super-voice-http.sock\" \\
     -H \"Authorization: Bearer $VOICE_TOKEN\" \\
     http://voice.local/v1/ping
```",
        license(name = "GPL-3.0-only", identifier = "GPL-3.0-only"),
        contact(name = "Super Voice", url = "https://github.com/jorge-menjivar/super-stt"),
    ),
    // The servers list is filled in by `LocalServers`, not here: the TCP
    // entry's URL embeds `config::DEFAULT_TCP_PORT`, and a macro attribute
    // takes a literal. Writing the port twice is exactly how the document
    // comes to advertise an address the daemon is not on.
    modifiers(&BearerAuth, &LocalServers),
    // The registry types are shared, generic over what they hold, and a type
    // reached only through a type parameter is not collected from the routes
    // on its own. See `super_engine_spec::registry`.
    components(schemas(
        super_stt_shared::registry::RegistryBackend,
        super_stt_shared::registry::RegistryModel,
    )),
    tags(
        (name = "auth", description = "Consent handshake and token probing."),
        (name = "health", description = "Liveness and what the daemon is currently running."),
        (name = "transcribe", description = "Start, stop and stream transcription."),
        (name = "speak", description = "Synthesize text, play it, and stop it — including the incremental WebSocket path for text that is still being produced."),
        (name = "voices", description = "The cloned-voice library: reference recordings the user owns, and the ids that name them on `POST /v1/tts/speak`."),
        (name = "events", description = "Server-Sent Events for recording and playback state, audio levels, model and download progress, and final transcripts."),
        (name = "pipeline", description = "Each feature's ordered stages, under `/v1/stt/pipeline` and `/v1/tts/pipeline`: which backend fills each, which model runs there, and on what device. Stages are numbered from 1 within each feature."),
        (name = "settings", description = "Stored daemon preferences, one value apiece, all under `/v1/settings`, one of each for both features: audio cues, volume, write and notification methods, language, update policy. Sharing the `settings` scope is not the same as being a setting \u{2014} `backends`, `pipeline` and `registry` are guarded by it too."),
        (name = "hardware", description = "What the daemon can see of this machine: GPUs, drivers, runtimes."),
        (name = "update", description = "Whether a newer daemon exists, and asking it to look now."),
        (name = "backends", description = "Installed backends: their models, options and secrets."),
        (name = "registry", description = "The published backend catalog: browse, install, update, uninstall."),
        (name = "contexts", description = "Named dictation contexts \u{2014} what the user is dictating, as a prompt for a model that follows instructions and a vocabulary of terms for one that does not. One is active at a time, and a backend may be pointed at another or at none."),
    ),
)]
pub(crate) struct ApiDoc;

/// The two addresses the daemon answers on.
///
/// Order matters: tooling that offers to send a request uses the first server,
/// and only one of these can actually receive one. The TCP listener is a real
/// address a browser can reach; the second is the socket's own path, spelled
/// `unix://`, which no HTTP client can resolve — it is there to be read, not
/// dialed.
struct LocalServers;

/// The example runtime directory shown for the `runtime_dir` server variable.
///
/// Per-platform because the two do not look remotely alike, and a reader on
/// the wrong one would take `/run/user/1000` for a path to try. It is an
/// illustration either way — the real value is this user's own — so the
/// macOS form keeps a placeholder where the per-user hash goes rather than
/// printing a directory that belongs to whoever generated the document.
#[cfg(target_os = "linux")]
const DEFAULT_RUNTIME_DIR: &str = "/run/user/1000";
/// See [`DEFAULT_RUNTIME_DIR`].
#[cfg(target_os = "macos")]
const DEFAULT_RUNTIME_DIR: &str = "/var/folders/xx/xxxxxxxxxxxxxxxxxxxxxxxxxxxx/T";

impl Modify for LocalServers {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::{ServerBuilder, ServerVariableBuilder};
        openapi.servers = Some(vec![
            ServerBuilder::new()
                .url(format!(
                    "http://127.0.0.1:{}",
                    crate::config::DEFAULT_TCP_PORT
                ))
                .description(Some(
                    "The daemon's default loopback TCP listener. Your browser can reach this.",
                ))
                .build(),
            ServerBuilder::new()
                // The socket's real path, not an invented hostname. It is what
                // a reader has to type, and naming it here saves them going to
                // find it. `unix://` + an absolute path is the spelling Docker
                // and friends use, so it reads as a socket rather than a host.
                .url("unix://{runtime_dir}/voice/super-voice-http.sock")
                .parameter(
                    "runtime_dir",
                    ServerVariableBuilder::new()
                        .default_value(DEFAULT_RUNTIME_DIR)
                        .description(Some(
                            "Your per-user runtime directory: `$XDG_RUNTIME_DIR` \
                             (`/run/user/<uid>`) on a systemd host, or the Darwin \
                             per-user temp directory (`/var/folders/<xx>/<hash>/T`, what \
                             `getconf DARWIN_USER_TEMP_DIR` prints) on macOS. `/tmp/stt` \
                             is the fallback on both. Override the whole socket path with \
                             `SUPER_VOICE_HTTP_SOCKET`.",
                        )),
                )
                .description(Some(
                    "The Unix socket native clients use. No browser can dial it. Use with curl instead.",
                ))
                .build(),
        ]);
    }
}

/// The one security scheme: the session token from `POST /v1/auth/request`,
/// presented as a bearer token. Declared here rather than per endpoint so the
/// scheme has a single definition; which *scopes* each endpoint needs is stated
/// on the endpoint, since that is where it differs.
struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi
            .components
            .as_mut()
            .expect("the derived document always carries a components object");
        components.add_security_scheme(
            "session_token",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "Session token from `POST /v1/auth/request`. Valid for 30 days, and \
                         bound to whoever the user approved: the calling binary over the \
                         Unix socket, or the page's origin over TCP. It stops working if \
                         that identity changes.",
                    ))
                    .build(),
            ),
        );
    }
}

/// Restate each operation's required scope in its summary, where a reader can
/// actually see it.
///
/// The scope is already declared, in `security` on every `#[utoipa::path]`, and
/// that declaration is the authority — a contract test checks it against the
/// guard the route actually sits behind. What it is not is *visible*: Swagger UI
/// renders scopes only for `OAuth2` and `OpenID` Connect schemes, where it runs the
/// flow itself. Ours is a plain bearer scheme, so the UI shows a padlock, drops
/// the scope array, and leaves a reader to work out which of six scopes an
/// endpoint wants by reading prose.
///
/// So this copies it into the summary, which is the line shown beside the path
/// in the collapsed operation list — the one place you can compare endpoints
/// without opening them. Copied rather than written by hand in 66 summaries:
/// a second hand-maintained statement of the same fact is one that goes stale,
/// and a summary claiming the wrong scope is worse than one that omits it.
///
/// Runs after assembly rather than as an [`ApiDoc`] modifier, because the paths
/// are merged in by the router *after* the base document is built — a modifier
/// here would walk an empty map.
pub(crate) fn annotate_scopes(openapi: &mut utoipa::openapi::OpenApi) {
    for item in openapi.paths.paths.values_mut() {
        let operations = [
            &mut item.get,
            &mut item.put,
            &mut item.post,
            &mut item.delete,
            &mut item.options,
            &mut item.head,
            &mut item.patch,
            &mut item.trace,
        ];
        for operation in operations.into_iter().flatten() {
            let note = scope_note(operation.security.as_ref());
            // Test against the note itself, not against a trailing `)`: a
            // summary may legitimately end in a parenthetical of its own, and
            // `GET /stt/transcribe/realtime` ("… session (WebSocket)") is one — a
            // looser check silently left the one endpoint whose summary already
            // had a suffix as the only one without its scope.
            if let Some(summary) = operation.summary.as_mut()
                && !summary.ends_with(&note)
            {
                summary.push_str(&note);
            }
        }
    }
}

/// Prefix each feature route's `operationId` with its feature: `stt_` under
/// `/v1/stt`, `tts_` under `/v1/tts`.
///
/// utoipa takes the id from the handler's name, and both features have a
/// pipeline whose handlers are named alike — `get_stage` serves
/// `/v1/stt/pipeline/{stage}` and `/v1/tts/pipeline/{stage}` both. The path
/// already names the feature, so the id says it the same way, and the routes
/// the daemon has once keep their plain ids.
pub(crate) fn name_feature_operations(openapi: &mut utoipa::openapi::OpenApi) {
    for (path, item) in &mut openapi.paths.paths {
        let Some(feature) = ["stt", "tts"]
            .into_iter()
            .find(|f| path.starts_with(&format!("/v1/{f}/")))
        else {
            continue;
        };
        let operations = [
            &mut item.get,
            &mut item.put,
            &mut item.post,
            &mut item.delete,
            &mut item.options,
            &mut item.head,
            &mut item.patch,
            &mut item.trace,
        ];
        for operation in operations.into_iter().flatten() {
            if let Some(id) = operation.operation_id.as_mut() {
                *id = format!("{feature}_{id}");
            }
        }
    }
}

/// `other`'s document, ready to merge into `base`: each of its schemas that
/// shares a name with one of `base`'s but has another structure is renamed
/// `<prefix><name>`, and every reference to it follows.
///
/// A merge keeps the first schema of a name, so without this one feature's
/// endpoints would be documented with the other feature's body. Both features
/// define types named alike — `StageReport`, `ModelList`, `ErrorCode` — and
/// most differ in more than prose. Two that differ only in their descriptions
/// or examples are the same shape, and stay one.
pub(crate) fn name_colliding_schemas(
    base: &utoipa::openapi::OpenApi,
    other: utoipa::openapi::OpenApi,
    prefix: &str,
) -> utoipa::openapi::OpenApi {
    let base = serde_json::to_value(base).expect("the document serializes");
    let mut other = serde_json::to_value(other).expect("the document serializes");
    let base_schemas = base["components"]["schemas"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    // Until nothing collides: renaming a schema changes every schema that
    // refers to it, and one that matched `base` before may not any more.
    while let Some(schemas) = other["components"]["schemas"].as_object_mut() {
        let renamed: Vec<String> = schemas
            .iter()
            .filter(|(name, shape)| {
                base_schemas
                    .get(*name)
                    .is_some_and(|theirs| structure(theirs) != structure(shape))
            })
            .map(|(name, _)| name.clone())
            .collect();
        if renamed.is_empty() {
            break;
        }
        for name in &renamed {
            if let Some(shape) = schemas.remove(name) {
                schemas.insert(format!("{prefix}{name}"), shape);
            }
        }
        for name in &renamed {
            rename_refs(
                &mut other,
                &format!("#/components/schemas/{name}"),
                &format!("#/components/schemas/{prefix}{name}"),
            );
        }
    }
    serde_json::from_value(other).expect("the document deserializes")
}

/// A schema with its prose taken out: what a client parses, not what a reader
/// is told.
#[must_use]
pub(crate) fn structure(schema: &serde_json::Value) -> serde_json::Value {
    match schema {
        serde_json::Value::Object(fields) => serde_json::Value::Object(
            fields
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "description" | "example" | "examples"))
                .map(|(key, value)| (key.clone(), structure(value)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(structure).collect())
        }
        other => other.clone(),
    }
}

/// Point every `$ref` to `from` in `doc` at `to`.
fn rename_refs(doc: &mut serde_json::Value, from: &str, to: &str) {
    match doc {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields.iter_mut() {
                if key == "$ref" && value.as_str() == Some(from) {
                    *value = serde_json::Value::String(to.to_string());
                } else {
                    rename_refs(value, from, to);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                rename_refs(item, from, to);
            }
        }
        _ => {}
    }
}

/// How an operation's `security` reads in plain words.
///
/// `None` means the operation declared none, which is the genuinely
/// unauthenticated case: `POST /v1/auth/request`, the endpoint that mints the
/// token every other one needs. An empty scope list means any valid token will
/// do whatever its scopes — `/v1/events` is that, because its scopes are
/// per-topic and enforced inside the handler against the topics asked for.
fn scope_note(security: Option<&Vec<utoipa::openapi::security::SecurityRequirement>>) -> String {
    let Some(requirements) = security else {
        return " (no token needed)".to_string();
    };
    // `SecurityRequirement` keeps its map private, so read it back the way it is
    // written out. This runs once, in a generator, not on a request path.
    let scopes: Vec<String> = requirements
        .iter()
        .filter_map(|requirement| serde_json::to_value(requirement).ok())
        .filter_map(|value| value.get("session_token").cloned())
        .filter_map(|scopes| serde_json::from_value::<Vec<String>>(scopes).ok())
        .flatten()
        .collect();

    if scopes.is_empty() {
        " (any valid token)".to_string()
    } else {
        format!(" (scope: {})", scopes.join(", "))
    }
}
