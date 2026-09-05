// SPDX-License-Identifier: GPL-3.0-only
//! Contract: the URL surface is exactly this, and its namespaces mean something.
//!
//! `openapi_contract` checks each operation is *described* well. This checks the
//! set of paths itself — what a client can call, and where.
//!
//! The inventory below is the point. A path is a promise: rename one and every
//! client 404s at the moment it is upgraded, with nothing in the daemon's own
//! logs to say why. Generating the document from the router means the two can
//! never disagree, but it also means a rename regenerates cleanly and passes
//! every check — the mistake becomes the new truth. So the surface is written
//! out once, by hand, and any change to it has to be made here too, in a diff a
//! reviewer can read.
//!
//! Adding an endpoint is meant to fail this test. Add its path to the list.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// Every path the daemon serves, with the methods it answers on.
///
/// Sorted, and deliberately spelled out rather than derived: this is the list a
/// reviewer reads to see what the URL surface became.
const URL_SURFACE: &[(&str, &str)] = &[
    ("/v1/auth/request", "post"),
    ("/v1/auth/status", "get"),
    ("/v1/backend/list", "get"),
    ("/v1/backend/{backend_id}", "delete"),
    ("/v1/backend/{backend_id}/option/list", "get"),
    ("/v1/backend/{backend_id}/option/{name}", "delete,get,post"),
    ("/v1/backend/{backend_id}/secret/list", "get"),
    ("/v1/backend/{backend_id}/secret/{name}", "delete,get,post"),
    ("/v1/events", "get"),
    ("/v1/gpu_info", "get"),
    ("/v1/ping", "get"),
    ("/v1/pipeline", "get"),
    ("/v1/pipeline/{stage}", "delete,get,post"),
    ("/v1/pipeline/{stage}/backend/list", "get"),
    ("/v1/pipeline/{stage}/device/list", "get"),
    ("/v1/pipeline/{stage}/model", "delete,get,post"),
    ("/v1/pipeline/{stage}/model/cancel", "post"),
    ("/v1/pipeline/{stage}/model/list", "get"),
    ("/v1/pipeline/{stage}/model/reload", "post"),
    ("/v1/pipeline/{stage}/model/{model}/device", "get,post"),
    ("/v1/pipeline/{stage}/model/{model}/device/list", "get"),
    (
        "/v1/pipeline/{stage}/model/{model}/language",
        "delete,get,post",
    ),
    ("/v1/pipeline/{stage}/model/{model}/language/list", "get"),
    ("/v1/registry/backend/list", "get"),
    ("/v1/registry/backend/install", "post"),
    ("/v1/registry/backend/refresh", "post"),
    ("/v1/registry/backend/update", "post"),
    ("/v1/settings/audio_theme", "get,post"),
    ("/v1/settings/audio_theme/list", "get"),
    ("/v1/settings/audio_theme/test", "post"),
    ("/v1/settings/custom_models_dir", "get,post"),
    ("/v1/settings/language", "delete,get,post"),
    ("/v1/settings/language/list", "get"),
    ("/v1/settings/notification_method", "get,post"),
    ("/v1/settings/preview_typing", "get,post"),
    ("/v1/settings/recording_stop_mode", "get,post"),
    ("/v1/settings/update_beta_optin", "get,post"),
    ("/v1/settings/update_check_enabled", "get,post"),
    ("/v1/settings/volume", "get,post"),
    ("/v1/settings/write_method", "get,post"),
    ("/v1/settings/write_method/test", "post"),
    ("/v1/status", "get"),
    ("/v1/transcribe", "post"),
    ("/v1/transcribe/realtime", "get"),
    ("/v1/transcribe/stop", "post"),
    ("/v1/update", "get"),
    ("/v1/update/check", "post"),
];

/// The generated document, as a consumer receives it.
fn document() -> Value {
    serde_json::to_value(super::openapi_document()).expect("the document serializes")
}

/// `path -> "delete,get,post"` for what the router actually serves.
fn served() -> BTreeMap<String, String> {
    const METHODS: [&str; 7] = ["get", "put", "post", "delete", "options", "head", "patch"];
    let doc = document();
    let mut out = BTreeMap::new();
    for (path, item) in doc["paths"].as_object().expect("paths is an object") {
        let mut methods: Vec<String> = item
            .as_object()
            .expect("a path item is an object")
            .keys()
            .filter(|m| METHODS.contains(&m.as_str()))
            .cloned()
            .collect();
        methods.sort();
        out.insert(path.clone(), methods.join(","));
    }
    out
}

/// The whole URL surface, spelled out.
///
/// Fails on any added, removed or renamed path, and on any method added to or
/// dropped from one. The message names what moved, because "the surface
/// changed" is not something a reviewer can act on.
#[test]
fn the_url_surface_is_exactly_this() {
    let served = served();
    let expected: BTreeMap<String, String> = URL_SURFACE
        .iter()
        .map(|(p, m)| ((*p).to_string(), (*m).to_string()))
        .collect();

    assert_eq!(
        URL_SURFACE.len(),
        expected.len(),
        "URL_SURFACE lists a path twice"
    );

    let added: Vec<_> = served
        .keys()
        .filter(|p| !expected.contains_key(*p))
        .collect();
    let removed: Vec<_> = expected
        .keys()
        .filter(|p| !served.contains_key(*p))
        .collect();
    assert!(
        added.is_empty() && removed.is_empty(),
        "the URL surface moved.\n  served but not listed: {added:?}\n  listed but not served: {removed:?}\n\
         Both empty? Then a path was renamed — it will show as one of each."
    );

    let changed: Vec<String> = expected
        .iter()
        .filter_map(|(p, want)| {
            let got = served.get(p)?;
            (got != want).then(|| format!("{p}: listed {want}, serves {got}"))
        })
        .collect();
    assert!(
        changed.is_empty(),
        "methods changed:\n  {}",
        changed.join("\n  ")
    );
}

/// Everything under `/v1/settings/` is a setting, and every setting is there.
///
/// This is the invariant the namespace exists to carry. It is easy to lose in
/// the obvious way: `settings` is also a *scope*, and the scope guards far more
/// than the settings — `/backends`, `/pipeline` and `/registry` all sit behind
/// it. Tagging by scope rather than by subject is what once filed a live GPU
/// probe and a model catalog as "settings", which is how a reader ends up
/// looking for `/v1/settings/gpu_info`.
///
/// So: the namespace and the tag have to agree, in both directions.
#[test]
fn the_settings_namespace_and_the_settings_tag_agree() {
    let doc = document();
    const METHODS: [&str; 7] = ["get", "put", "post", "delete", "options", "head", "patch"];

    let mut namespaced_but_untagged = Vec::new();
    let mut tagged_but_not_namespaced = Vec::new();

    for (path, item) in doc["paths"].as_object().expect("paths is an object") {
        for (method, op) in item.as_object().expect("a path item is an object") {
            if !METHODS.contains(&method.as_str()) {
                continue;
            }
            let tagged = op["tags"]
                .as_array()
                .expect("every operation carries tags")
                .iter()
                .any(|t| t == "settings");
            let namespaced = path.starts_with("/v1/settings/");
            match (namespaced, tagged) {
                (true, false) => namespaced_but_untagged.push(format!("{method} {path}")),
                (false, true) => tagged_but_not_namespaced.push(format!("{method} {path}")),
                _ => {}
            }
        }
    }

    assert!(
        namespaced_but_untagged.is_empty(),
        "under /v1/settings/ but not tagged `settings`: {namespaced_but_untagged:?}"
    );
    assert!(
        tagged_but_not_namespaced.is_empty(),
        "tagged `settings` but not under /v1/settings/: {tagged_but_not_namespaced:?}\n\
         Either move it into the namespace, or give it the tag its subject deserves — \
         sharing the `settings` scope is not the same as being a setting."
    );
}

/// Every `/v1/settings/` path is guarded by the `settings` scope.
///
/// The namespace is a promise about access as much as about subject. A settings
/// path reachable with a `status` token — or with no token — would be a hole a
/// reader has no reason to go looking for, precisely because the prefix says
/// what it says.
#[test]
fn the_settings_namespace_is_settings_scoped() {
    let wrong: Vec<String> = super::v1::enforced_scopes()
        .into_iter()
        .filter(|(path, _)| path.starts_with("/v1/settings/"))
        .filter(|(_, scopes)| scopes.as_deref() != Some(&["settings"]))
        .map(|(path, scopes)| format!("{path}: guarded by {scopes:?}"))
        .collect();
    assert!(
        wrong.is_empty(),
        "settings paths not behind the `settings` scope:\n  {}",
        wrong.join("\n  ")
    );
}

/// No path is served under two spellings.
///
/// Registering a handler twice under different paths is silent: both answer,
/// the document lists both, and clients split between them until one is
/// removed. The `summary` is the cheapest per-operation identity available.
#[test]
fn no_operation_is_served_at_two_paths() {
    let doc = document();
    const METHODS: [&str; 7] = ["get", "put", "post", "delete", "options", "head", "patch"];
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut dupes = Vec::new();

    for (path, item) in doc["paths"].as_object().expect("paths is an object") {
        for (method, op) in item.as_object().expect("a path item is an object") {
            if !METHODS.contains(&method.as_str()) {
                continue;
            }
            let summary = op["summary"]
                .as_str()
                .expect("every operation has a summary");
            if let Some(first) = seen.insert(summary.to_string(), path.clone())
                && &first != path
            {
                dupes.push(format!("{summary:?}: {first} and {path}"));
            }
        }
    }
    assert!(
        dupes.is_empty(),
        "the same operation is served at two paths:\n  {}",
        dupes.join("\n  ")
    );
}

/// The declared tags and the tags in use are the same set.
///
/// Both directions drift silently. A tag declared and no longer carried by any
/// operation renders as an empty section in the published docs — which is how
/// `models` was left behind when `GET /models` became
/// `GET /pipeline/{stage}/model/list`. A tag used but never declared renders
/// with no description at all, under a heading the reader has to guess at.
#[test]
fn every_declared_tag_is_used_and_every_used_tag_is_declared() {
    let doc = document();
    const METHODS: [&str; 7] = ["get", "put", "post", "delete", "options", "head", "patch"];

    let declared: BTreeSet<String> = doc["tags"]
        .as_array()
        .expect("the document declares tags")
        .iter()
        .map(|t| t["name"].as_str().expect("a tag has a name").to_string())
        .collect();

    let mut used = BTreeSet::new();
    for (_, item) in doc["paths"].as_object().expect("paths is an object") {
        for (method, op) in item.as_object().expect("a path item is an object") {
            if !METHODS.contains(&method.as_str()) {
                continue;
            }
            for t in op["tags"].as_array().expect("every operation carries tags") {
                used.insert(t.as_str().expect("a tag is a string").to_string());
            }
        }
    }

    let orphaned: Vec<_> = declared.difference(&used).collect();
    let undeclared: Vec<_> = used.difference(&declared).collect();
    assert!(
        orphaned.is_empty(),
        "declared but carried by no operation: {orphaned:?}"
    );
    assert!(
        undeclared.is_empty(),
        "used but never declared, so they publish with no description: {undeclared:?}"
    );
}

/// Every path the document lists is one the guards know about.
///
/// [`super::v1::enforced_scopes`] is built from the scope grouping, the document
/// from the route registrations. A path in one and not the other means a route
/// was registered outside every group — reachable, and guarded by nothing.
#[test]
fn every_path_sits_in_a_scope_group() {
    let guarded: BTreeSet<String> = super::v1::enforced_scopes().into_keys().collect();
    let served = served();
    let ungrouped: Vec<&String> = served.keys().filter(|p| !guarded.contains(*p)).collect();
    assert!(
        ungrouped.is_empty(),
        "served but in no scope group, so behind no guard: {ungrouped:?}"
    );
}
