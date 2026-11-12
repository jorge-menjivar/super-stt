// SPDX-License-Identifier: GPL-3.0-only
//! `/v1/shortcuts` through the `GlobalShortcuts` portal, end to end: a real
//! daemon on a desktop that is not COSMIC, and a fake portal on a session bus
//! of their own, which records each call and answers it the way the portal
//! does. Beside it is a fake of KDE Plasma's shortcut service, which the
//! daemon asks on KDE to forget a shortcut.
//!
//! The bus is a `dbus-daemon` with a config of the test's own and no service
//! directories, so no call can start a real service on it.

#![cfg(target_os = "linux")]

mod common;

use common::{Method, StatusCode, TestDaemon};
use serde_json::Value as Json;
use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_voice_daemon::shortcuts::portal::Session;
use super_voice_shared::daemon::http_client;
use super_voice_shared::models::shortcuts::ShortcutId;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";

// ---------- a session bus of the test's own ---------------------------------

/// A `dbus-daemon` that only these tests are on. Stopped when dropped.
struct Bus {
    child: std::process::Child,
    address: String,
}

impl Bus {
    /// Start one, listening in `dir`.
    fn start(dir: &Path) -> Self {
        std::fs::create_dir_all(dir).expect("bus dir");
        let config = dir.join("bus.conf");
        std::fs::write(
            &config,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
                dir.display()
            ),
        )
        .expect("bus config");
        let mut child = std::process::Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--print-address", "--nofork", "--nopidfile"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("dbus-daemon runs");
        let mut address = String::new();
        std::io::BufReader::new(child.stdout.take().expect("stdout"))
            .read_line(&mut address)
            .expect("dbus-daemon prints its address");
        Self {
            child,
            address: address.trim().to_string(),
        }
    }

    async fn connect(&self) -> zbus::Connection {
        zbus::connection::Builder::address(self.address.as_str())
            .expect("address")
            .build()
            .await
            .expect("connect to the test bus")
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------- the fake portal --------------------------------------------------

/// What the fake portal was asked, and how it answers.
#[derive(Default)]
struct Calls {
    /// The app ids registered.
    registered: Vec<String>,
    /// Each session opened, and closed.
    sessions: Vec<String>,
    closed: Vec<String>,
    /// Each bind: its session, and each shortcut's id and preferred keys.
    binds: Vec<(String, Vec<(String, String)>)>,
    /// What each session has bound.
    bound: HashMap<String, Vec<String>>,
    /// Answer a bind as the user saying no in the dialog, with this response
    /// code: the portal's own for a cancel is 1, and GNOME answers its Cancel
    /// with 2.
    refuse: Option<u32>,
    /// Answer a bind as KDE answers its dialog's Cancel: each shortcut the
    /// desktop did not have yet is bound, with no keys.
    cancel_binds_no_keys: bool,
    /// The shortcuts the desktop holds with no keys.
    no_keys: Vec<String>,
    /// Each shortcut KDE's shortcut service was told to forget: the app id
    /// it is under, and its id.
    forgotten: Vec<(String, String)>,
    /// The shortcuts the desktop keeps across sessions, as KDE Plasma and
    /// GNOME do: each one ever bound, until the user removes it in the
    /// desktop's settings.
    kept: Vec<String>,
}

type Shared = Arc<Mutex<Calls>>;

/// The keys the fake desktop puts each shortcut on.
fn keys_for(id: &str) -> &'static str {
    match id {
        "dictate" => "Meta+Space",
        _ => "Meta+Shift+Space",
    }
}

fn option(options: &HashMap<String, OwnedValue>, key: &str) -> String {
    options
        .get(key)
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
        .unwrap_or_else(|| panic!("no {key} in the options"))
}

/// The sender's part of a request or session path: `:1.42` is `1_42`.
fn sender_part(header: &Header<'_>) -> String {
    header
        .sender()
        .expect("a sender")
        .trim_start_matches(':')
        .replace('.', "_")
}

/// Answer the request at `path` with `Response`. Before the call returns,
/// which a portal can do too: the client listens before it calls. The
/// object server runs this on zbus's own executor, not on tokio, so there is
/// nothing to hand the answer to for later.
async fn respond(
    conn: &zbus::Connection,
    header: &Header<'_>,
    path: String,
    code: u32,
    results: HashMap<String, Value<'static>>,
) {
    conn.emit_signal(
        header.sender().map(|sender| sender.as_str()),
        path.as_str(),
        "org.freedesktop.portal.Request",
        "Response",
        &(code, results),
    )
    .await
    .expect("emit Response");
}

/// A `shortcuts` result: each id, with the keys the fake desktop gave it,
/// none for the ones in `no_keys`.
fn described(ids: &[String], no_keys: &[String]) -> Value<'static> {
    let list: Vec<(String, HashMap<String, Value<'static>>)> = ids
        .iter()
        .map(|id| {
            let keys = if no_keys.contains(id) {
                ""
            } else {
                keys_for(id)
            };
            let mut properties = HashMap::new();
            properties.insert(
                "trigger_description".to_string(),
                Value::from(keys.to_string()),
            );
            (id.clone(), properties)
        })
        .collect();
    Value::from(list)
}

struct Registry(Shared);

#[zbus::interface(name = "org.freedesktop.host.portal.Registry")]
impl Registry {
    fn register(&self, app_id: String, _options: HashMap<String, OwnedValue>) {
        self.0.lock().unwrap().registered.push(app_id);
    }
}

struct GlobalShortcuts(Shared);

#[zbus::interface(name = "org.freedesktop.portal.GlobalShortcuts")]
impl GlobalShortcuts {
    async fn create_session(
        &self,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let sender = sender_part(&header);
        let request = format!(
            "{PORTAL_PATH}/request/{sender}/{}",
            option(&options, "handle_token")
        );
        let session = format!(
            "{PORTAL_PATH}/session/{sender}/{}",
            option(&options, "session_handle_token")
        );
        conn.object_server()
            .at(
                session.as_str(),
                FakeSession {
                    calls: Arc::clone(&self.0),
                    path: session.clone(),
                },
            )
            .await?;
        self.0.lock().unwrap().sessions.push(session.clone());
        let mut results = HashMap::new();
        results.insert("session_handle".to_string(), Value::from(session));
        respond(conn, &header, request.clone(), 0, results).await;
        Ok(OwnedObjectPath::try_from(request).expect("path"))
    }

    async fn bind_shortcuts(
        &self,
        session_handle: OwnedObjectPath,
        shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
        _parent_window: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let request = format!(
            "{PORTAL_PATH}/request/{}/{}",
            sender_part(&header),
            option(&options, "handle_token")
        );
        let session = session_handle.to_string();
        let asked: Vec<(String, String)> = shortcuts
            .iter()
            .map(|(id, properties)| (id.clone(), option(properties, "preferred_trigger")))
            .collect();
        let ids: Vec<String> = asked.iter().map(|(id, _)| id.clone()).collect();
        let (refuse, no_keys) = {
            let mut calls = self.0.lock().unwrap();
            calls.binds.push((session.clone(), asked));
            if calls.refuse.is_none() {
                calls.bound.insert(session, ids.clone());
                for id in &ids {
                    if !calls.kept.contains(id) {
                        calls.kept.push(id.clone());
                        if calls.cancel_binds_no_keys {
                            calls.no_keys.push(id.clone());
                        }
                    }
                }
            }
            (calls.refuse, calls.no_keys.clone())
        };
        if let Some(code) = refuse {
            respond(conn, &header, request.clone(), code, HashMap::new()).await;
        } else {
            let mut results = HashMap::new();
            results.insert("shortcuts".to_string(), described(&ids, &no_keys));
            respond(conn, &header, request.clone(), 0, results).await;
        }
        Ok(OwnedObjectPath::try_from(request).expect("path"))
    }

    async fn list_shortcuts(
        &self,
        session_handle: OwnedObjectPath,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let request = format!(
            "{PORTAL_PATH}/request/{}/{}",
            sender_part(&header),
            option(&options, "handle_token")
        );
        // A session lists what it bound, or, before it binds anything, what
        // the desktop kept from sessions before it.
        let (ids, no_keys) = {
            let calls = self.0.lock().unwrap();
            let ids = calls
                .bound
                .get(session_handle.as_str())
                .cloned()
                .unwrap_or_else(|| calls.kept.clone());
            (ids, calls.no_keys.clone())
        };
        let mut results = HashMap::new();
        results.insert("shortcuts".to_string(), described(&ids, &no_keys));
        respond(conn, &header, request.clone(), 0, results).await;
        Ok(OwnedObjectPath::try_from(request).expect("path"))
    }

    /// Lower-case, as the portal names it, not zbus's `Version`.
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }

    #[zbus(signal)]
    async fn activated(
        emitter: &SignalEmitter<'_>,
        session_handle: ObjectPath<'_>,
        shortcut_id: &str,
        timestamp: u64,
        options: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<()>;
}

struct FakeSession {
    calls: Shared,
    path: String,
}

#[zbus::interface(name = "org.freedesktop.portal.Session")]
impl FakeSession {
    fn close(&self) {
        let mut calls = self.calls.lock().unwrap();
        calls.closed.push(self.path.clone());
        calls.bound.remove(&self.path);
    }
}

/// KDE Plasma's shortcut service, which keeps every app's shortcuts.
struct KGlobalAccel(Shared);

#[zbus::interface(name = "org.kde.KGlobalAccel")]
impl KGlobalAccel {
    /// Lower-case, as KDE names it, not zbus's `Unregister`.
    #[zbus(name = "unregister")]
    fn unregister(&self, component: String, shortcut: String) -> bool {
        let mut calls = self.0.lock().unwrap();
        let had = calls.kept.contains(&shortcut);
        calls.kept.retain(|id| *id != shortcut);
        calls.no_keys.retain(|id| *id != shortcut);
        calls.forgotten.push((component, shortcut));
        had
    }
}

/// The fake portal, serving on `bus` as the portal's own name, beside KDE's
/// shortcut service.
async fn fake_portal(bus: &Bus) -> (zbus::Connection, Shared) {
    let calls = Shared::default();
    let conn = zbus::connection::Builder::address(bus.address.as_str())
        .expect("address")
        .name("org.freedesktop.portal.Desktop")
        .expect("name")
        .name("org.kde.kglobalaccel")
        .expect("kglobalaccel name")
        .serve_at(PORTAL_PATH, Registry(Arc::clone(&calls)))
        .expect("registry")
        .serve_at(PORTAL_PATH, GlobalShortcuts(Arc::clone(&calls)))
        .expect("global shortcuts")
        .serve_at("/kglobalaccel", KGlobalAccel(Arc::clone(&calls)))
        .expect("kglobalaccel")
        .build()
        .await
        .expect("serve the fake portal");
    (conn, calls)
}

// ---------- the daemon -------------------------------------------------------

/// A daemon on KDE, on `bus`, with a token for it with the `settings` scope.
/// `config`, when given, is its config file before it starts.
async fn start(label: &str, bus: &Bus, config: Option<&str>) -> (TestDaemon, String) {
    start_on("KDE", label, bus, config).await
}

/// [`start`], on `desktop`.
async fn start_on(
    desktop: &str,
    label: &str,
    bus: &Bus,
    config: Option<&str>,
) -> (TestDaemon, String) {
    let builder = common::daemon(label)
        .env("XDG_CURRENT_DESKTOP", desktop)
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address);
    if let Some(content) = config {
        let path = config_file(&builder.home().config);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, content).expect("write config");
    }
    let daemon = builder.start().await;
    let token = http_client::auth_request(
        daemon.socket().to_path_buf(),
        "shortcuts portal test",
        &["settings"],
    )
    .await
    .expect("auth_request")
    .session_token;
    (daemon, token)
}

/// The daemon's config file under a test home's config.
fn config_file(config: &Path) -> PathBuf {
    config.join("super-voice/daemon.ron")
}

async fn call(daemon: &TestDaemon, token: &str, method: Method, path: &str) -> (StatusCode, Json) {
    common::request(daemon.socket(), method, path, Some(token), None).await
}

/// Each shortcut's `state` and `keys`, by id.
fn states(list: &Json) -> Vec<(String, String, String)> {
    list["shortcuts"]
        .as_array()
        .expect("shortcuts")
        .iter()
        .map(|s| {
            (
                s["id"].as_str().expect("id").to_string(),
                s["state"].as_str().expect("state").to_string(),
                s["keys"].as_str().expect("keys").to_string(),
            )
        })
        .collect()
}

fn row(id: &str, state: &str, keys: &str) -> (String, String, String) {
    (id.to_string(), state.to_string(), keys.to_string())
}

/// The shortcuts and preferred keys of the last bind.
fn last_bind(calls: &Shared) -> Vec<(String, String)> {
    calls
        .lock()
        .unwrap()
        .binds
        .last()
        .map(|(_, asked)| asked.clone())
        .unwrap_or_default()
}

fn pair(id: &str, keys: &str) -> (String, String) {
    (id.to_string(), keys.to_string())
}

/// The shortcuts KDE's shortcut service was told to forget.
fn forgotten(calls: &Shared) -> Vec<(String, String)> {
    calls.lock().unwrap().forgotten.clone()
}

// ---------- the tests --------------------------------------------------------

/// Each shortcut added is bound in a new session with the ones before it,
/// under the app's id, and one removed is left out of the next. What is bound
/// is kept in the config. On KDE, KDE's shortcut service forgets one removed.
#[tokio::test]
async fn the_portal_binds_each_shortcut_added_and_drops_one_removed() {
    let home = tempfile::tempdir().expect("tempdir");
    let bus = Bus::start(home.path());
    let (_portal, calls) = fake_portal(&bus).await;
    let (daemon, token) = start("shortcuts-portal", &bus, None).await;

    let (status, list) = call(&daemon, &token, Method::GET, "/shortcuts").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["method"], "portal");
    assert_eq!(list["removable"], true, "KDE forgets one when asked");
    assert_eq!(
        states(&list),
        [
            row("dictate", "missing", "Super+Space"),
            row("read_aloud", "missing", "Super+Shift+Space")
        ]
    );
    assert!(
        calls.lock().unwrap().sessions.is_empty(),
        "nothing bound yet"
    );

    let (status, list) = call(&daemon, &token, Method::POST, "/shortcuts/dictate").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(
        states(&list),
        [
            row("dictate", "installed", "Meta+Space"),
            row("read_aloud", "missing", "Super+Shift+Space")
        ]
    );
    assert_eq!(calls.lock().unwrap().registered, ["super-voice-app"]);
    assert_eq!(last_bind(&calls), [pair("dictate", "LOGO+space")]);

    let (status, list) = call(&daemon, &token, Method::POST, "/shortcuts/read_aloud").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(
        states(&list),
        [
            row("dictate", "installed", "Meta+Space"),
            row("read_aloud", "installed", "Meta+Shift+Space")
        ]
    );
    assert_eq!(
        last_bind(&calls),
        [
            pair("dictate", "LOGO+space"),
            pair("read_aloud", "LOGO+SHIFT+space")
        ]
    );
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.sessions.len(), 2);
        assert_eq!(
            calls.closed,
            calls.sessions[..1],
            "the first session is closed"
        );
    }

    let (status, list) = call(&daemon, &token, Method::DELETE, "/shortcuts/dictate").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(
        states(&list),
        [
            row("dictate", "missing", "Super+Space"),
            row("read_aloud", "installed", "Meta+Shift+Space")
        ]
    );
    assert_eq!(last_bind(&calls), [pair("read_aloud", "LOGO+SHIFT+space")]);
    assert_eq!(forgotten(&calls), [pair("super-voice-app", "dictate")]);
    assert_eq!(calls.lock().unwrap().kept, ["read_aloud"]);
    let config = std::fs::read_to_string(config_file(&daemon.home().config)).expect("config");
    assert!(config.contains(r#""read_aloud""#), "{config}");
    assert!(!config.contains(r#""dictate""#), "{config}");

    let (status, list) = call(&daemon, &token, Method::DELETE, "/shortcuts/read_aloud").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let calls = calls.lock().unwrap();
    assert!(calls.kept.is_empty(), "KDE has none left: {:?}", calls.kept);
    assert_eq!(
        calls.closed.len(),
        calls.sessions.len(),
        "no session is left open"
    );
    assert_eq!(calls.sessions.len(), 3, "and none is opened for nothing");
}

/// How a desktop answers its dialog's Cancel.
#[derive(Debug, Clone, Copy)]
enum Cancel {
    /// The bind ends with this response code: the portal's own for a cancel
    /// is 1, and GNOME answers 2.
    Code(u32),
    /// KDE binds the shortcut, with no keys.
    BindsNoKeys,
}

/// A shortcut the user turns down in the desktop's dialog is refused, and not
/// kept, however the desktop answers the Cancel. KDE, which binds it with no
/// keys, is told to forget it, so adding it again asks again.
#[tokio::test]
async fn a_shortcut_the_user_turns_down_is_not_kept() {
    for cancel in [Cancel::Code(1), Cancel::Code(2), Cancel::BindsNoKeys] {
        let home = tempfile::tempdir().expect("tempdir");
        let bus = Bus::start(home.path());
        let (_portal, calls) = fake_portal(&bus).await;
        match cancel {
            Cancel::Code(code) => calls.lock().unwrap().refuse = Some(code),
            Cancel::BindsNoKeys => calls.lock().unwrap().cancel_binds_no_keys = true,
        }
        let label = match cancel {
            Cancel::Code(code) => format!("shortcuts-refused-{code}"),
            Cancel::BindsNoKeys => "shortcuts-refused-no-keys".to_string(),
        };
        let (daemon, token) = start(&label, &bus, None).await;

        let (status, body) = call(&daemon, &token, Method::POST, "/shortcuts/dictate").await;
        assert_eq!(status, StatusCode::CONFLICT, "{cancel:?}: {body}");
        assert_eq!(body["error_code"], "shortcut_refused", "{cancel:?}: {body}");

        let (_, list) = call(&daemon, &token, Method::GET, "/shortcuts").await;
        assert_eq!(
            states(&list)[0],
            row("dictate", "missing", "Super+Space"),
            "{cancel:?}"
        );
        let config =
            std::fs::read_to_string(config_file(&daemon.home().config)).unwrap_or_default();
        assert!(!config.contains(r#""dictate""#), "{cancel:?}: {config}");

        if let Cancel::BindsNoKeys = cancel {
            let calls = calls.lock().unwrap();
            assert_eq!(calls.forgotten, [pair("super-voice-app", "dictate")]);
            assert!(calls.kept.is_empty(), "KDE forgot it: {:?}", calls.kept);
            assert_eq!(
                calls.closed.len(),
                calls.sessions.len(),
                "the session that bound it with no keys is closed"
            );
        }
    }
}

/// Off KDE, the daemon cannot take a shortcut off the desktop: the list says
/// so, and removing one only stops Super Voice binding it.
#[tokio::test]
async fn off_kde_removing_is_left_to_the_desktop() {
    let home = tempfile::tempdir().expect("tempdir");
    let bus = Bus::start(home.path());
    let (_portal, calls) = fake_portal(&bus).await;
    let (daemon, token) = start_on("GNOME", "shortcuts-portal-gnome", &bus, None).await;

    let (status, list) = call(&daemon, &token, Method::POST, "/shortcuts/dictate").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["removable"], false, "{list}");

    let (status, list) = call(&daemon, &token, Method::DELETE, "/shortcuts/dictate").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(states(&list)[0], row("dictate", "missing", "Super+Space"));
    let calls = calls.lock().unwrap();
    assert!(calls.forgotten.is_empty(), "{:?}", calls.forgotten);
    assert_eq!(calls.kept, ["dictate"], "the desktop keeps it");
}

/// A daemon binds the shortcuts in its config as it starts, with no client
/// asking: the portal kept them only while the last one held them.
#[tokio::test]
async fn saved_shortcuts_are_bound_again_at_start() {
    let home = tempfile::tempdir().expect("tempdir");
    let bus = Bus::start(home.path());
    let (_portal, calls) = fake_portal(&bus).await;

    // A config with read aloud saved, as a daemon before this one left it.
    let (first, token) = start("shortcuts-portal-saved", &bus, None).await;
    let (status, body) = call(&first, &token, Method::POST, "/shortcuts/read_aloud").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let config = std::fs::read_to_string(config_file(&first.home().config)).expect("config");
    drop(first);
    let binds_before = calls.lock().unwrap().binds.len();

    let (second, token) = start("shortcuts-portal-restart", &bus, Some(&config)).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while calls.lock().unwrap().binds.len() == binds_before {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the daemon never bound its saved shortcuts"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(last_bind(&calls), [pair("read_aloud", "LOGO+SHIFT+space")]);

    let (_, list) = call(&second, &token, Method::GET, "/shortcuts").await;
    assert_eq!(
        states(&list)[1],
        row("read_aloud", "installed", "Meta+Shift+Space")
    );
}

/// Shortcuts added on one desktop are that desktop's. A daemon starting on
/// another binds none of them, so that desktop asks the user nothing, and
/// they are kept for the first one.
#[tokio::test]
async fn shortcuts_added_on_one_desktop_are_not_bound_on_another() {
    let home = tempfile::tempdir().expect("tempdir");
    let bus = Bus::start(home.path());
    let (_portal, calls) = fake_portal(&bus).await;

    let (first, token) = start("shortcuts-portal-on-kde", &bus, None).await;
    let (status, body) = call(&first, &token, Method::POST, "/shortcuts/dictate").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let config = std::fs::read_to_string(config_file(&first.home().config)).expect("config");
    drop(first);
    let sessions_before = calls.lock().unwrap().sessions.len();

    let (second, token) = start_on("GNOME", "shortcuts-portal-on-gnome", &bus, Some(&config)).await;
    let (_, list) = call(&second, &token, Method::GET, "/shortcuts").await;
    assert_eq!(states(&list)[0], row("dictate", "missing", "Super+Space"));
    // The bind at a start runs in the background: give it its chance.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        calls.lock().unwrap().sessions.len(),
        sessions_before,
        "nothing is bound on GNOME, so GNOME asks nothing"
    );
    let config = std::fs::read_to_string(config_file(&second.home().config)).expect("config");
    assert!(config.contains(r#""KDE""#), "{config}");
    assert!(config.contains(r#""dictate""#), "{config}");
}

/// A shortcut the user removed in the desktop's settings while the daemon
/// was off is not bound again at the next start: it is forgotten, and the
/// page offers to add it. One the desktop still has is bound again.
#[tokio::test]
async fn a_shortcut_removed_in_the_desktop_is_offered_again() {
    let home = tempfile::tempdir().expect("tempdir");
    let bus = Bus::start(home.path());
    let (_portal, calls) = fake_portal(&bus).await;

    let (first, token) = start("shortcuts-portal-removed", &bus, None).await;
    for id in ["dictate", "read_aloud"] {
        let (status, body) = call(&first, &token, Method::POST, &format!("/shortcuts/{id}")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let config = std::fs::read_to_string(config_file(&first.home().config)).expect("config");
    drop(first);
    // The user removes dictation in the desktop's settings.
    calls.lock().unwrap().kept.retain(|id| id != "dictate");
    let binds_before = calls.lock().unwrap().binds.len();

    let (second, token) = start("shortcuts-portal-removed-restart", &bus, Some(&config)).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while calls.lock().unwrap().binds.len() == binds_before {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the daemon never bound the shortcut the desktop kept"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(last_bind(&calls), [pair("read_aloud", "LOGO+SHIFT+space")]);

    let (_, list) = call(&second, &token, Method::GET, "/shortcuts").await;
    assert_eq!(
        states(&list),
        [
            row("dictate", "missing", "Super+Space"),
            row("read_aloud", "installed", "Meta+Shift+Space")
        ]
    );
    let config = std::fs::read_to_string(config_file(&second.home().config)).expect("config");
    assert!(!config.contains(r#""dictate""#), "{config}");
}

/// A press reaches the session's caller as the shortcut's id, and only a
/// press on its own session does.
#[tokio::test]
async fn a_press_on_the_session_is_handed_on() {
    let home = tempfile::tempdir().expect("tempdir");
    let bus = Bus::start(home.path());
    let (portal, calls) = fake_portal(&bus).await;

    let (pressed, mut presses) = tokio::sync::mpsc::unbounded_channel();
    let session = Session::open(bus.connect().await, move |id| {
        let _ = pressed.send(id);
    })
    .await
    .expect("open");
    let bound = session.bind(&[ShortcutId::ReadAloud]).await.expect("bind");
    assert_eq!(bound.len(), 1);
    let ours = calls.lock().unwrap().sessions[0].clone();

    let emitter = SignalEmitter::new(&portal, PORTAL_PATH).expect("emitter");
    let other = ObjectPath::try_from("/org/freedesktop/portal/desktop/session/1_1/other").unwrap();
    GlobalShortcuts::activated(&emitter, other, "dictate", 0, HashMap::new())
        .await
        .expect("emit");
    GlobalShortcuts::activated(
        &emitter,
        ObjectPath::try_from(ours.as_str()).unwrap(),
        "read_aloud",
        0,
        HashMap::new(),
    )
    .await
    .expect("emit");

    let id = tokio::time::timeout(Duration::from_secs(5), presses.recv())
        .await
        .expect("a press arrives")
        .expect("the channel is open");
    assert_eq!(
        id,
        ShortcutId::ReadAloud,
        "the other session's press is not ours"
    );
}
