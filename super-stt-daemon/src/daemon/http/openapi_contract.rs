// SPDX-License-Identifier: GPL-3.0-only
//! Contract: the published document has to be worth publishing.
//!
//! `just openapi-check` already proves the committed `openapi.json` matches
//! what the router generates. That is a narrower claim than it sounds: it
//! catches a *stale* file, not a *wrong* one. Regenerating turns any mistake
//! into a committed mistake, and the check goes green.
//!
//! These tests are the other half. They assert the things a reader depends on
//! and nothing else enforces:
//!
//! - the scope an endpoint advertises is the scope the router enforces on it;
//! - every operation carries prose, not a placeholder;
//! - every success body names a real shape, not "some JSON";
//! - every guarded endpoint documents the failures a client will actually hit.
//!
//! They iterate the whole document, so adding an endpoint adds nothing here —
//! the new endpoint is simply held to the same standard as the rest.
//!
//! The document is inspected as JSON rather than through utoipa's types,
//! because JSON is what a client generator, a linter and a human actually
//! receive.

use serde_json::Value;

/// The generated document, as a consumer receives it.
fn document() -> Value {
    serde_json::to_value(super::openapi_document()).expect("the document serializes")
}

/// Every `(path, method, operation)` in the document.
fn operations(doc: &Value) -> Vec<(String, String, &Value)> {
    const METHODS: [&str; 7] = ["get", "put", "post", "delete", "options", "head", "patch"];
    let mut out = Vec::new();
    for (path, item) in doc["paths"].as_object().expect("paths is an object") {
        for (method, op) in item.as_object().expect("a path item is an object") {
            if METHODS.contains(&method.as_str()) {
                out.push((path.clone(), method.clone(), op));
            }
        }
    }
    assert!(!out.is_empty(), "the document describes no operations");
    out
}

/// The scopes an operation advertises under the one security scheme, or `None`
/// when it advertises no security at all.
fn advertised_scopes(op: &Value) -> Option<Vec<String>> {
    let requirements = op.get("security")?.as_array()?;
    let first = requirements.first()?;
    let scopes = first
        .get("session_token")
        .expect("the only security scheme is `session_token`")
        .as_array()
        .expect("scopes are an array");
    Some(
        scopes
            .iter()
            .map(|s| s.as_str().expect("a scope is a string").to_string())
            .collect(),
    )
}

/// Follow a `$ref` into `components/schemas`, or return the schema unchanged
/// when it is inline.
fn resolve<'a>(doc: &'a Value, schema: &'a Value) -> &'a Value {
    let Some(reference) = schema.get("$ref").and_then(Value::as_str) else {
        return schema;
    };
    let name = reference
        .strip_prefix("#/components/schemas/")
        .unwrap_or_else(|| panic!("unexpected $ref target: {reference}"));
    doc["components"]["schemas"]
        .get(name)
        .unwrap_or_else(|| panic!("$ref names a schema that is not in the document: {name}"))
}

/// The scope each endpoint advertises must be the scope the router puts it
/// behind.
///
/// These are written in two places that cannot see each other: the scope in a
/// `#[utoipa::path]` attribute on the handler, the guard in the group the
/// handler is registered into. Nothing else compares them, and a mismatch is
/// silent in the worst way — the endpoint works, the documentation is wrong,
/// and a client author requests a scope that earns them a `403` with no
/// explanation. `/auth/request` is the one endpoint with no security at all,
/// because it is how a caller obtains a token in the first place.
#[test]
fn every_operation_advertises_the_scope_its_router_enforces() {
    let doc = document();
    let enforced = super::v1::enforced_scopes();

    let mut wrong = Vec::new();
    for (path, method, op) in operations(&doc) {
        let guard = enforced
            .get(&path)
            .unwrap_or_else(|| panic!("{method} {path} is in the document but in no scope group"));
        let advertised = advertised_scopes(op);
        let expected = guard.as_ref().map(|s| {
            s.iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        });
        if advertised != expected {
            wrong.push(format!(
                "  {} {path}\n      router enforces: {expected:?}\n      document says:   {advertised:?}",
                method.to_uppercase()
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "these endpoints document a scope the router does not enforce:\n{}",
        wrong.join("\n")
    );
}

/// Every path the router serves must appear in the document, and vice versa.
///
/// `routes!` makes this true by construction today — the path comes off the
/// handler's own attribute. The test is here so that stays true if anyone
/// registers a route the older way, with `.route("/x", get(handler))`, which
/// compiles fine and publishes nothing.
#[test]
fn the_document_and_the_router_cover_the_same_paths() {
    let doc = document();
    let documented: std::collections::BTreeSet<String> = doc["paths"]
        .as_object()
        .expect("paths is an object")
        .keys()
        .cloned()
        .collect();
    let served: std::collections::BTreeSet<String> =
        super::v1::enforced_scopes().keys().cloned().collect();

    assert_eq!(
        documented, served,
        "the document and the router disagree about which paths exist"
    );
}

/// Every operation has to say what it is and what it does.
///
/// A summary is what a reader scans in the sidebar; a description is the part
/// that answers "should I be calling this?". An endpoint that ships with
/// neither is invisible in the reference even though it is in the document.
#[test]
fn every_operation_carries_prose() {
    let doc = document();

    let mut bare = Vec::new();
    for (path, method, op) in operations(&doc) {
        let summary = op.get("summary").and_then(Value::as_str).unwrap_or("");
        let description = op.get("description").and_then(Value::as_str).unwrap_or("");
        // A one-word summary is a placeholder, not a summary. The shortest real
        // one in the document is "Liveness probe".
        if summary.len() < 8 || description.len() < 40 {
            bare.push(format!(
                "  {} {path}  (summary {} chars, description {} chars)",
                method.to_uppercase(),
                summary.len(),
                description.len()
            ));
        }
    }
    assert!(
        bare.is_empty(),
        "these operations are missing prose a reader needs:\n{}",
        bare.join("\n")
    );
}

/// Every response has to name the shape it carries.
///
/// This is the property the narrow response types exist for. The daemon passes
/// one wide `DaemonResponse` around internally, and a schema generated from
/// *that* would tell a client `GET /volume` may return forty-two fields when it
/// returns two. A body that resolves to a schema with no properties means an
/// endpoint has slipped back to publishing "some JSON".
///
/// Two documented exceptions, both because the payload is not a JSON body at
/// all: the event stream and the `101` of the realtime upgrade.
#[test]
fn every_response_names_a_real_shape() {
    let doc = document();

    let mut untyped = Vec::new();
    for (path, method, op) in operations(&doc) {
        for (code, response) in op["responses"]
            .as_object()
            .expect("responses is an object")
            .iter()
        {
            let Some(content) = response.get("content") else {
                // A response with no body at all: only the protocol-upgrade
                // `101`, which hands the connection to the WebSocket session.
                assert_eq!(
                    code, "101",
                    "{method} {path} {code} carries no body; only the upgrade may"
                );
                continue;
            };
            let Some(json) = content.get("application/json") else {
                // `text/event-stream`: the frames are documented in prose,
                // since SSE has no schema language.
                assert!(
                    content.get("text/event-stream").is_some(),
                    "{method} {path} {code} has a body in neither JSON nor SSE"
                );
                continue;
            };
            let schema = resolve(&doc, &json["schema"]);
            let named = schema
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|p| !p.is_empty());
            if !named {
                untyped.push(format!("  {} {path} → {code}", method.to_uppercase()));
            }
        }
    }
    assert!(
        untyped.is_empty(),
        "these responses publish an unnamed shape:\n{}",
        untyped.join("\n")
    );
}

/// Every guarded endpoint documents the two failures its guard produces.
///
/// A client hits `401` the moment its token expires and `403` the moment it
/// asks for something outside its scopes. Both are certain to happen, both are
/// recoverable, and a client that has not been told about them handles neither.
/// The scope-less group is exempt from `403`: a guard that checks no scope
/// cannot deny one.
#[test]
fn every_guarded_endpoint_documents_its_auth_failures() {
    let doc = document();

    let mut missing = Vec::new();
    for (path, method, op) in operations(&doc) {
        let Some(scopes) = advertised_scopes(op) else {
            continue; // unauthenticated: nothing to expire, nothing to deny
        };
        let responses = op["responses"].as_object().expect("responses is an object");

        let mut wanted = vec!["401"];
        if !scopes.is_empty() {
            wanted.push("403");
        }
        for code in wanted {
            if !responses.contains_key(code) {
                missing.push(format!(
                    "  {} {path} does not document {code}",
                    method.to_uppercase()
                ));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "these endpoints leave a client unable to handle an auth failure:\n{}",
        missing.join("\n")
    );
}

/// The document declares the one security scheme the whole surface uses.
///
/// Every operation's `security` references `session_token` by name. If the
/// scheme itself went missing the references would dangle, and a generated
/// client would have no way to send the token at all.
#[test]
fn the_bearer_scheme_is_defined() {
    let doc = document();

    assert_eq!(
        doc["openapi"].as_str().map(|v| v.starts_with("3.1")),
        Some(true),
        "the document should be OpenAPI 3.1 — Swagger UI 4.x cannot read it, \
         and docs/protocol/openapi.html pins 5.x on that basis"
    );

    let scheme = &doc["components"]["securitySchemes"]["session_token"];
    assert_eq!(scheme["type"], "http", "the scheme is HTTP auth");
    assert_eq!(scheme["scheme"], "bearer", "presented as a bearer token");
}

/// The documented error shapes must match the envelopes the daemon really
/// builds.
///
/// The daemon has several error constructors, each with a slightly different
/// shape, and each endpoint's `#[utoipa::path]` names one of two schemas for
/// its failures. Those are hand-paired: nothing checks that the constructor an
/// endpoint reaches for emits the keys the schema it points at declares. A key
/// the daemon sends but the document omits is worse than an undocumented
/// endpoint — the client author reads a complete-looking shape and writes a
/// parser that discards the field carrying the failure's identity.
///
/// A sibling of `error_envelope_contract`, which checks the same envelopes
/// survive the trip to the client. This one checks they were described.
///
/// Adding an error constructor? Add it here too.
#[tokio::test]
async fn documented_error_shapes_match_the_envelopes_the_daemon_builds() {
    use super::internal::helpers::responses::{
        invalid_session, model_not_loaded_response, rate_limited, reason,
        recording_in_progress_response, scope_denied,
    };
    use super::v1::backends::{json_error, json_error_msg};
    use super::v1::registry::{registry_error, registry_error_msg};
    use axum::http::StatusCode;

    async fn keys_of(resp: axum::response::Response) -> Vec<String> {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("collect body");
        let value: Value = serde_json::from_slice(&bytes).expect("an error body is JSON");
        value
            .as_object()
            .expect("an error body is an object")
            .keys()
            .cloned()
            .collect()
    }

    // Each constructor, paired with the schema the endpoints using it name.
    let cases: Vec<(&str, &str, axum::response::Response)> = vec![
        (
            "json_error",
            "ErrorEnvelope",
            json_error(StatusCode::NOT_FOUND, "unknown_backend"),
        ),
        (
            "json_error_msg",
            "ErrorEnvelope",
            json_error_msg(StatusCode::BAD_REQUEST, "invalid_option", "out of range"),
        ),
        ("scope_denied", "ErrorEnvelope", scope_denied()),
        ("rate_limited", "ErrorEnvelope", rate_limited()),
        (
            "recording_in_progress",
            "ErrorEnvelope",
            recording_in_progress_response(),
        ),
        (
            "model_not_loaded",
            "ErrorEnvelope",
            model_not_loaded_response(),
        ),
        (
            "registry_error",
            "RegistryError",
            registry_error(StatusCode::NOT_FOUND, "not_found"),
        ),
        (
            "registry_error_msg",
            "RegistryError",
            registry_error_msg(StatusCode::INTERNAL_SERVER_ERROR, "remove_failed", "denied"),
        ),
        (
            "invalid_session",
            "ReasonEnvelope",
            invalid_session(reason::UNKNOWN),
        ),
    ];

    let doc = document();
    let mut undocumented = Vec::new();
    for (constructor, schema_name, response) in cases {
        let schema = doc["components"]["schemas"]
            .get(schema_name)
            .unwrap_or_else(|| panic!("{schema_name} is not published in the document"));
        let declared: Vec<&String> = schema["properties"]
            .as_object()
            .expect("an envelope declares properties")
            .keys()
            .collect();

        for key in keys_of(response).await {
            if !declared.iter().any(|d| **d == key) {
                undocumented.push(format!(
                    "  {constructor} sends `{key}`, which {schema_name} does not declare"
                ));
            }
        }
    }
    assert!(
        undocumented.is_empty(),
        "the daemon sends error fields the document does not describe:\n{}",
        undocumented.join("\n")
    );
}
