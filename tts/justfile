app_name := 'super-tts-app'
daemon_bin_name := 'super-tts-daemon'
# systemd unit name (matches super-tts-daemon/systemd/super-tts.service)
service_name := 'super-tts'
cli_name := 'super-tts-cli'
consent_name := 'super-tts-consent'
wrapper_name := 'tts'

# The COSMIC applet Super TTS shares with Super STT. It lives in its own repo,
# super-libre/super-cosmic-applet, whose releases the installer installs.
# `install-applet` builds it from a checkout of that repo: SHARED_APPLET_DIR,
# else ../super-cosmic-applet. It replaces Super TTS's own applet,
# super-tts-cosmic-applet, whose files `install-applet` removes.
shared_applet := 'super-cosmic-applet'
shared_applet_dir := env('SHARED_APPLET_DIR', '..' / 'super-cosmic-applet')
old_applet := 'super-tts-cosmic-applet'

# Installation paths — root-owned under /usr/local, matching the
# release installers, so install/uninstall recipes escalate with sudo
# for the copy/remove steps. `systemctl --user` calls stay unprivileged
# (the daemon runs as the user; only the files are root-owned).
home_dir := env('HOME')
install_prefix := '/usr/local'
bin_dir := install_prefix / 'bin'
# systemd *user* unit, but installed root-owned.
systemd_unit_dir := '/usr/lib/systemd/user'
run_dir := env('XDG_RUNTIME_DIR') / 'tts'
log_dir := home_dir / '.local' / 'share' / 'tts' / 'logs'
desktop_dir := install_prefix / 'share' / 'applications'
icons_dir := install_prefix / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps'
# Theme root, not the leaf dir: gtk-update-icon-cache indexes a whole theme.
icon_theme_dir := install_prefix / 'share' / 'icons' / 'hicolor'

# Binary paths
app_src := 'target' / 'release' / app_name
daemon_src := 'target' / 'release' / daemon_bin_name
cli_src := 'target' / 'release' / cli_name
consent_src := 'target' / 'release' / consent_name
app_dst := bin_dir / app_name
daemon_dst := bin_dir / daemon_bin_name
cli_dst := bin_dir / cli_name
consent_dst := bin_dir / consent_name
wrapper_dst := bin_dir / wrapper_name

# App files
app_desktop_file_name := 'super-tts-app.desktop'
app_desktop_file_src := 'super-tts-app' / 'resources' / app_desktop_file_name
app_icon_src := 'super-tts-app' / 'resources' / 'icons' / 'hicolor' / 'scalable' / 'apps' / 'super-tts-app.svg'
app_desktop_file_dst := desktop_dir / app_desktop_file_name
app_icon_dst := icons_dir / 'super-tts-app.svg'


# Service file
service_file := service_name + '.service'
service_dst := systemd_unit_dir / service_file

# Default recipe which runs `just build-release`
default: build-release

# Runs `cargo clean`
clean:
    cargo clean

# Removes vendored dependencies
clean-vendor:
    rm -rf .cargo vendor vendor.tar

# `cargo clean` and removes vendored dependencies
clean-dist: clean clean-vendor

# Compiles with debug profile
# Usage: just build-debug
build-debug *args:
    cargo build {{ args }}

# Compiles with release profile
# Usage: just build-release
build-release *args:
    cargo build --release {{ args }}

# Compiles release profile with vendored dependencies
# Usage: just build-vendored
build-vendored *args: vendor-extract
    just build-release --frozen --offline {{ args }}

# Runs a clippy check
check *args:
    cargo clippy --all-features --workspace {{ args }} -- -W clippy::pedantic -D warnings -D unused_must_use

# Runs a clippy check with JSON message format
check-json: (check '--message-format=json')

# Verify the daemon compiles under every backend-transport feature combination.
# The `check`/`test` gates use `--all-features` (both transports on), so a `#[cfg]`
# slip that only breaks a single-transport or no-backend build slips through
# otherwise (see audit Tier 1 #8). `-D warnings` also catches feature-conditional
# unused imports.
check-features:
    RUSTFLAGS="-D warnings" cargo check -p super-tts-daemon --no-default-features --features subprocess-backends
    RUSTFLAGS="-D warnings" cargo check -p super-tts-daemon --no-default-features --features wasm-backends
    RUSTFLAGS="-D warnings" cargo check -p super-tts-daemon --no-default-features
    # Compile (don't run) the subprocess transport integration test + its
    # mock_backend fixture so a refactor breaking SubprocessBackend can't pass CI
    # green. Running it needs a systemd --user session (SUPER_TTS_TEST_SUBPROCESS=1)
    # that hosted runners lack — unlike the WASM mock it can't run hermetically —
    # but compiling it keeps it from bit-rotting (audit 2 Tier 2 #13).
    RUSTFLAGS="-D warnings" cargo test -p super-tts-daemon --features test-fixtures --no-run --test subprocess_mock

# Check formatting without modifying files
fmt-check:
    cargo fmt --all -- --check

# Apply rustfmt to the whole workspace
fmt:
    cargo fmt --all

# Run the test suite. Usage: just test [--verbose]
test *args:
    cargo test {{ args }}

# Run the #[ignore]'d GUI smoke tests. These spawn real libcosmic surfaces
# against the live compositor, so they need a desktop session and can't run in
# CI — but they're the only thing that catches a surface the compositor
# rejects (e.g. a corner radius wider than an autosized layer surface, which
# kills the client mid-handshake and turns every consent prompt into a silent
# denial). Run them after touching any surface setup. Usage: just test-gui
[doc("Run the GUI smoke tests against the live compositor (needs a desktop session)")]
test-gui *args:
    cargo test -p super-tts-consent --test gui_smoke -- --ignored --nocapture {{ args }}

# Unit-test install.sh's pure logic (arch detection, channel validation, tag
# resolution from a JSON string) against fixture JSON. It's a bash script,
# not a Cargo target, so it isn't covered by `just test` — see
# scripts/test-install.sh for what it checks and why.
test-install:
    bash scripts/test-install.sh

# End-to-end install verification: installs a real published release, asserts
# the complete installed tree, then uninstalls and asserts nothing is left.
# DESTRUCTIVE — it writes to (and clears) the real /usr/local and
# /usr/lib/systemd/user, so it is deliberately NOT part of `just ci` and
# refuses to run without SUPER_TTS_INSTALL_E2E_YES=1. CI runs it on disposable
# runners (.github/workflows/install-e2e.yml); locally, run it in a container.
# The script is super-engine's, fetched at the rev install-e2e.yml pins, so a
# local run tests what CI does. It installs from the repo install.sh names.
# Set SUPER_TTS_INSTALLER_BIN to a built super-tts-install to run the local
# pass too.
# Usage: just test-install-e2e [stable|beta]
[doc("End-to-end install test (DESTRUCTIVE: real install into /usr/local)")]
test-install-e2e channel="stable":
    #!/usr/bin/env bash
    set -euo pipefail
    rev=$(grep -oP 'super-engine/\.github/actions/install-e2e@\K[0-9a-f]{40}' .github/workflows/install-e2e.yml)
    repo=$(grep -oP '^GITHUB_REPO="\K[^"]+' install.sh)
    script=$(mktemp)
    trap 'rm -f "$script"' EXIT
    curl -fsSL -o "$script" \
      "https://raw.githubusercontent.com/super-libre/super-engine/$rev/.github/actions/install-e2e/test-install-e2e.sh"
    GITHUB_REPOSITORY="$repo" bash "$script" super-tts tts {{ channel }}

# Load every committed old-config fixture against the current config types.
config-compat *args:
    cargo test -p super-tts-daemon --lib config {{ args }}

# Run doctests
doctest *args:
    cargo test --doc {{ args }}

# Verify the generated TOML schemas are current
schema-check:
    cargo test -p super-tts-registry-types --features schema

# Measure code coverage over the whole workspace (requires cargo-llvm-cov).
# --remap-path-prefix keeps report paths relative, and tests/ is excluded so
# only product code is counted. Build the mock WASM backends first
# (just build-mock-wasm-backend{,-realtime}) so the daemon transport tests run
# instead of self-skipping. Usage: just coverage [--html]
coverage *args:
    cargo llvm-cov --workspace --remap-path-prefix --ignore-filename-regex 'tests/' {{ args }}

# Coverage for CI: write lcov.info and print a summary.
coverage-lcov:
    cargo llvm-cov --workspace --remap-path-prefix --ignore-filename-regex 'tests/' --lcov --output-path lcov.info
    cargo llvm-cov report --summary-only --ignore-filename-regex 'tests/'

# Render a browsable HTML report to target/llvm-cov/html/. The index is a single
# flat file list (cargo-llvm-cov's default); per-file pages show line-level
# coverage. Usage: just coverage-html
coverage-html *args:
    cargo llvm-cov --workspace --remap-path-prefix --ignore-filename-regex 'tests/' --html {{ args }}

# Full local CI gate: format, lint, feature-combo compile, tests, install.sh
# tests, doctests, schemas
ci: fmt-check check check-features test test-install doctest schema-check openapi-check

# Run the app for testing purposes
run-app *args:
    env RUST_BACKTRACE=full RUST_LOG=super_tts_app=debug,super_tts_shared=debug cargo run --bin {{ app_name }} {{ args }}

# Run the daemon for testing purposes. Also builds super-tts-consent into
# the same target dir, since the daemon only looks for the consent helper
# alongside its own binary (auth_request popups fail without it).
# Usage: just run-daemon [cargo flags, e.g. --release]
run-daemon *args:
    cargo build --bin {{ consent_name }} --bin {{ daemon_bin_name }} {{ args }}
    env RUST_BACKTRACE=full RUST_LOG=super_tts_daemon=debug cargo run --bin {{ daemon_bin_name }} -v {{ args }}

# Run the CLI for testing purposes (talks to the running daemon over the HTTP socket)
# Usage: just run-cli [ping|status|record|stop|logout] [args]
run-cli *args:
    env RUST_BACKTRACE=full RUST_LOG=super_tts_cli=debug,super_tts_shared=debug cargo run --bin {{ cli_name }} -- {{ args }}

# Run the consent dialog on its own, without the daemon. The dialog is
# env-driven rather than argument-driven, so this fills in a plausible request;
# pass scope names to change which permission bullets render. The decision
# (allow / deny / dismissed) goes to stdout, same as the daemon would read.
#
# Heads up: the dialog is an overlay layer surface with an exclusive keyboard
# grab, so it holds the keyboard until you click Allow or Deny — the mouse
# still works. For a hands-free run, set SUPER_TTS_AUTH_AUTO_APPROVE_AFTER_MS and it
# approves itself after that many milliseconds. That env var is debug-only; a
# release build can never self-approve.
#
# For the automated version of this, see `just test-gui`.
#
# Usage: just run-consent [scope...]
#   just run-consent
#   just run-consent speak settings secrets
#   SUPER_TTS_AUTH_AUTO_APPROVE_AFTER_MS=4000 just run-consent
[doc("Run the consent dialog standalone. Usage: just run-consent [scope...]")]
run-consent *scopes:
    #!/usr/bin/env bash
    set -euo pipefail

    scopes="{{ scopes }}"
    # A spread of scopes so the dialog renders a representative bullet list.
    [ -n "$scopes" ] || scopes="speak status settings"

    echo "Scopes: $scopes"
    # Quiet RUST_LOG: the default pulls in thousands of wgpu/zbus/wayland lines
    # at startup and buries the dialog's own output.
    env RUST_BACKTRACE=full \
        RUST_LOG=super_tts_consent=debug,super_tts_shared=debug \
        SUPER_TTS_AUTH_APP_NAME="Test App" \
        SUPER_TTS_AUTH_SCOPES="$scopes" \
        SUPER_TTS_AUTH_EXE_PATH="/usr/bin/test-app" \
        cargo run --bin {{ consent_name }}

# Run security audit to check for vulnerabilities
audit:
    cargo audit

# Build only the app
build-app *args:
    cargo build --release --bin {{ app_name }} {{ args }}

# Build only the daemon
build-daemon *args:
    cargo build --release --bin {{ daemon_bin_name }} {{ args }}

# Build only the CLI
build-cli *args:
    cargo build --release --bin {{ cli_name }} {{ args }}

# Build only the installer/self-updater
build-install:
    cargo build --release --bin super-tts-install

# Build only the consent helper (co-located with the daemon binary)
build-consent:
    cargo build --release --bin {{ consent_name }}

# Build the generic mock WASM backend fixture (wasm32-wasip2) that
# tests/wasm_mock.rs loads to exercise the daemon's WasmBackend orchestration.
# Requires: rustup target add wasm32-wasip2
build-mock-wasm-backend:
    cargo build --manifest-path super-tts-daemon/tests/fixtures/mock-wasm-backend/Cargo.toml --target wasm32-wasip2 --release

# Build the generic mock REALTIME WASM backend fixture (wasm32-wasip2) that
# tests/wasm_mock_realtime.rs loads to exercise the daemon's realtime
# orchestration (ws-server.handle). Requires: rustup target add wasm32-wasip2
build-mock-wasm-realtime-backend:
    cargo build --manifest-path super-tts-daemon/tests/fixtures/mock-wasm-realtime-backend/Cargo.toml --target wasm32-wasip2 --release

# Copy the canonical WIT (realtime.wit + deps) into every backend that bundles it.
sync-wit:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob # no in-tree backend bundles WIT after the relocations
    for dir in backends/*/wit; do
        cp docs/protocol/wit/realtime.wit "$dir/realtime.wit"
        rm -rf "$dir/deps"
        mkdir -p "$dir/deps"
        cp docs/protocol/wit/deps/*.wit "$dir/deps/"
        echo "synced $dir (realtime.wit + deps)"
    done

# CI check: every bundled WIT (realtime.wit + deps) must match the canonical.
check-wit-sync:
    #!/usr/bin/env bash
    set -euo pipefail
    fail=0
    shopt -s nullglob # no in-tree backend bundles WIT after the relocations
    for dir in backends/*/wit; do
        if ! diff -q "$dir/realtime.wit" docs/protocol/wit/realtime.wit >/dev/null; then
            echo "WIT drift: $dir/realtime.wit" >&2; fail=1
        fi
        if ! diff -rq "$dir/deps" docs/protocol/wit/deps >/dev/null 2>&1; then
            echo "WIT deps drift: $dir/deps" >&2; fail=1
        fi
    done
    [ "$fail" -eq 0 ]

# Regenerate the JSON Schemas for backend.toml and registry.toml from the
# canonical Rust types in super-tts-registry-types. CI fails when the
# committed schemas are stale, so run this after changing those types.
gen-schemas:
    cargo run -p super-tts-registry-types --features schema --bin gen_schemas

# Regenerate docs/protocol/openapi.json from the daemon's live /v1 router.
# Run it after changing anything under super-tts-daemon/src/daemon/http/v1/;
# `just openapi-check` (part of `just ci`) fails when the committed file is
# stale. Starts no daemon and touches no keyring — the document is built from
# the route registrations themselves.
openapi:
    cargo run -q -p super-tts-daemon --bin gen_openapi

# CI check: the committed OpenAPI document must match what the router produces.
# Regenerates into a temp file and diffs, so a failing check never leaves the
# working tree modified — the fix is `just openapi`.
openapi-check:
    #!/usr/bin/env bash
    set -euo pipefail
    spec=docs/protocol/openapi.json
    before=$(mktemp)
    cp "$spec" "$before"
    trap 'cp "$before" "$spec"; rm -f "$before"' EXIT
    cargo run -q -p super-tts-daemon --bin gen_openapi >/dev/null
    if ! diff -u "$before" "$spec"; then
        echo >&2
        echo "openapi.json is stale — the router and the published spec disagree." >&2
        echo "Run 'just openapi' and commit the result." >&2
        exit 1
    fi
    echo "openapi.json is current"

# Browse the protocol spec locally. Regenerates it first so what you read is
# what the router currently serves, then serves docs/protocol/ over HTTP —
# the pages fetch openapi.json, which a file:// origin would block.
#
# Two renderings of the same document, so neither taste has to win:
#   --swagger  (default) the familiar Swagger UI, richer per-response examples
#   --scalar   prose beside examples, reads better with long descriptions
# Either page links to the other, so the choice is not final.
#
# The Swagger view offers "Try it out", which reaches a running daemon over its
# TCP listener — a browser cannot dial the Unix socket. Nothing to configure:
# the listener is on by default and admits any origin.
#
# The port is chosen by the OS unless you name one, so this never collides with
# whatever else is already listening. The server binds before announcing, so the
# URL it prints is always the one actually being served. One consequence worth
# knowing: a token is bound to the origin that asked for it, and a new port is a
# new origin, so each run asks for consent again.
#
# Usage: just openapi-serve [--scalar|--swagger] [port]
openapi-serve *args:
    #!/usr/bin/env bash
    set -euo pipefail
    page=openapi.html
    port=0  # 0 asks the OS for any free port
    # Flag and port in either order, both optional.
    argv=({{ args }})
    for arg in ${argv[@]+"${argv[@]}"}; do
        case "$arg" in
            --swagger)   page=openapi.html ;;
            --scalar)    page=scalar.html ;;
            *[!0-9]*|'') echo "openapi-serve: unrecognized argument '$arg'" >&2
                         echo "usage: just openapi-serve [--scalar|--swagger] [port]" >&2
                         exit 2 ;;
            *)           port="$arg" ;;
        esac
    done
    just openapi
    exec python3 - "$port" "$page" <<'PYEOF'
    import functools
    import http.server
    import socketserver
    import sys
    import threading
    import webbrowser

    port, page = int(sys.argv[1]), sys.argv[2]

    class Server(socketserver.TCPServer):
        # Rebind straight after a previous run rather than waiting out TIME_WAIT.
        allow_reuse_address = True

    handler = functools.partial(
        http.server.SimpleHTTPRequestHandler, directory="docs/protocol"
    )
    try:
        httpd = Server(("127.0.0.1", port), handler)
    except OSError as e:
        # Only reachable for a port named on the command line; port 0 cannot
        # collide.
        sys.exit(f"openapi-serve: cannot bind port {port}: {e}\n"
                 f"Omit the port and one will be chosen for you.")

    with httpd:
        url = f"http://localhost:{httpd.server_address[1]}/{page}"
        print(f"Serving the daemon protocol at {url}  (ctrl-c to stop)", flush=True)
        # Open once the socket is listening, so the first request cannot race
        # the bind. Silent when there is no browser to open (headless, SSH).
        threading.Thread(target=webbrowser.open, args=(url,), daemon=True).start()
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print()
    PYEOF

# Install the app (system installation under /usr/local)
install-app:
    #!/usr/bin/env bash
    # Ask for sudo up front and keep the timestamp alive in the
    # background: the build can outlast sudo's credential cache, and a
    # password prompt buried in build output is easy to miss.
    sudo -v
    ( while sudo -n -v 2>/dev/null; do sleep 60; done ) &
    sudo_keepalive=$!
    trap 'kill "$sudo_keepalive" 2>/dev/null' EXIT

    # Build the app first
    echo "Building app..."
    if ! just build-app; then
        echo "❌ App build failed or was interrupted"
        exit 1
    fi

    # Check if binary exists
    if [ ! -f "{{ app_src }}" ]; then
        echo "❌ App binary not found at {{ app_src }}"
        exit 1
    fi

    echo "Installing Super TTS app to {{ app_dst }}"
    sudo mkdir -p {{ bin_dir }}
    sudo install -m755 {{ app_src }} {{ app_dst }}

    # Install the desktop entry for application menu
    echo "Installing desktop entry..."
    sudo install -Dm0644 {{ app_desktop_file_src }} {{ app_desktop_file_dst }}

    # Install the app icon
    echo "Installing app icon..."
    sudo install -Dm0644 {{ app_icon_src }} {{ app_icon_dst }}

    # Update desktop database
    if command -v update-desktop-database &> /dev/null; then
        sudo update-desktop-database {{ desktop_dir }} 2>/dev/null || true
    fi

    # Update icon cache
    if command -v gtk-update-icon-cache &> /dev/null; then
        sudo gtk-update-icon-cache -f -t {{ icon_theme_dir }} 2>/dev/null || true
    fi

    # Nudge COSMIC's launcher caches (app grid + search backend) so the
    # entry appears without a relogin; both respawn on demand and rescan.
    pkill -f '^cosmic-app-library$' 2>/dev/null || true
    pkill -f '^cosmic-launcher$' 2>/dev/null || true
    pkill -f '^pop-launcher( |$)' 2>/dev/null || true

    echo "✓ Super TTS app installed: {{ app_dst }}"
    echo "✓ Desktop entry installed: {{ app_desktop_file_dst }}"
    echo "✓ App icon installed: {{ app_icon_dst }}"

# Build the shared COSMIC applet from its checkout and install it under
# /usr/local, with that repo's own `just install`, then remove Super TTS's own
# applet it replaces. The release installer installs the applet's newest
# release instead.
install-applet:
    #!/usr/bin/env bash
    set -euo pipefail
    dir='{{ shared_applet_dir }}'
    if [ ! -f "$dir/justfile" ]; then
        echo "No super-cosmic-applet checkout at $dir." >&2
        echo "Clone https://github.com/super-libre/super-cosmic-applet there, or set SHARED_APPLET_DIR to one." >&2
        exit 1
    fi

    # Ask for sudo up front and keep the timestamp alive in the
    # background: the build can outlast sudo's credential cache, and a
    # password prompt buried in build output is easy to miss.
    sudo -v
    ( while sudo -n -v 2>/dev/null; do sleep 60; done ) &
    sudo_keepalive=$!
    trap 'kill "$sudo_keepalive" 2>/dev/null' EXIT

    echo "Building and installing the COSMIC applet from $dir..."
    just --justfile "$dir/justfile" --working-directory "$dir" install

    # The shared applet replaces Super TTS's own.
    sudo rm -f '{{ bin_dir }}/{{ old_applet }}' '{{ icons_dir }}/{{ old_applet }}.svg'
    for side in full left right; do
        sudo rm -f "{{ desktop_dir }}/{{ old_applet }}-$side.desktop"
    done

    # Refresh the desktop/icon caches so the panel picks up the new applet
    # entries without a relogin (mirrors install-app).
    if command -v update-desktop-database &> /dev/null; then
        sudo update-desktop-database {{ desktop_dir }} 2>/dev/null || true
    fi

    if command -v gtk-update-icon-cache &> /dev/null; then
        sudo gtk-update-icon-cache -f -t {{ icon_theme_dir }} 2>/dev/null || true
    fi

    # Nudge COSMIC's launcher caches (app grid + search backend) so the
    # entries appear without a relogin; both respawn on demand and rescan.
    pkill -f '^cosmic-app-library$' 2>/dev/null || true
    pkill -f '^cosmic-launcher$' 2>/dev/null || true
    pkill -f '^pop-launcher( |$)' 2>/dev/null || true

    echo "✓ COSMIC applet installed: {{ bin_dir }}/{{ shared_applet }}"
    echo "Add it to a panel in COSMIC Settings > Desktop > Panel > Configure panel applets:"
    echo "  Super Applet (Full), (Left Side) or (Right Side)."

# Install the daemon (system installation under /usr/local; runs as a
# systemd --user service)
# Usage: just install-daemon
install-daemon:
    #!/usr/bin/env bash
    # Ask for sudo up front and keep the timestamp alive in the
    # background: the builds (daemon, consent, CLI) can outlast sudo's
    # credential cache, and a password prompt buried in build output is
    # easy to miss.
    sudo -v
    ( while sudo -n -v 2>/dev/null; do sleep 60; done ) &
    sudo_keepalive=$!
    trap 'kill "$sudo_keepalive" 2>/dev/null' EXIT

    # Build the daemon first
    echo "Building daemon..."

    if ! just build-daemon; then
        echo "❌ Daemon build failed or was interrupted"
        exit 1
    fi

    # Check if binary exists
    if [ ! -f "{{ daemon_src }}" ]; then
        echo "❌ Daemon binary not found at {{ daemon_src }}"
        exit 1
    fi

    echo "Installing Super TTS daemon as user service..."

    # Install binary
    echo "Installing daemon binary to {{ daemon_dst }}"
    sudo mkdir -p {{ bin_dir }}
    sudo install -m755 {{ daemon_src }} {{ daemon_dst }}

    # Build + install the consent helper alongside the daemon. The
    # daemon's `locate_consent_helper` only looks for it next to its
    # own binary (no PATH fallback, for security), so without this
    # step every /auth/request fails with `popup_failed`.
    echo "Building consent helper..."
    if ! just build-consent; then
        echo "❌ Consent helper build failed"
        exit 1
    fi
    if [ ! -f "{{ consent_src }}" ]; then
        echo "❌ Consent helper binary not found at {{ consent_src }}"
        exit 1
    fi
    echo "Installing consent helper to {{ consent_dst }}"
    sudo install -m755 {{ consent_src }} {{ consent_dst }}

    # Install the CLI alongside the daemon. The `tts` wrapper created below
    # execs the CLI (client commands like `record`/`stop` live there, not in
    # the daemon binary), so the wrapper is broken without it.
    if ! just install-cli; then
        echo "❌ CLI installation failed"
        exit 1
    fi

    # Create user directories
    echo "Creating user directories..."
    mkdir -p {{ run_dir }}
    mkdir -p {{ log_dir }}

    # Install the unit root-owned. Its bare ExecStart name resolves via
    # systemd's fixed search path, which covers {{ bin_dir }}.
    #
    # Installed verbatim, and it must stay that way: the daemon takes no
    # configuration on its command line (the model, its device, and the audio
    # theme are all config / `POST /v1` state), so a flag appended to
    # ExecStart is rejected by clap before the listener binds and the unit
    # crash-loops under Restart=always. `every_shipped_execstart_parses`
    # (super-tts-daemon/src/cli_tests.rs) fails if one is reintroduced here.
    echo "Installing systemd user unit..."
    sudo install -Dm0644 super-tts-daemon/systemd/{{ service_file }} {{ service_dst }}

    # A unit left in ~/.config/systemd/user by an older install takes
    # precedence over the packaged one — remove it or systemd keeps
    # launching the stale ~/.local/bin binary.
    rm -f "$HOME/.config/systemd/user/{{ service_file }}"

    # Create the `tts` convenience wrapper (for keyboard shortcuts like
    # `tts record --write`). Note: we deliberately do NOT use `sg tts
    # -c "..."` here. Changing the GID of the daemon (or its CLI
    # peers) breaks `/proc/<pid>/exe` readlinks across the daemon ↔
    # client boundary — the kernel's `__ptrace_may_access` check
    # requires *both* matching UID and matching GID, and clients run
    # with the user's primary GID. The `tts` group is no longer
    # required for socket ACLs in the user-mode systemd unit; the
    # socket file is owned `user:user-primary-group` with 0660, so the
    # owner (same user) can access regardless of group membership.
    echo "Creating wrapper script at {{ wrapper_dst }}"
    wrapper_tmp=$(mktemp)
    echo '#!/bin/bash' > "$wrapper_tmp"
    echo '# Super TTS convenience wrapper — invokes super-tts-cli directly.' >> "$wrapper_tmp"
    echo '# Used by keyboard shortcuts (e.g. Super+Space → "tts record --write").' >> "$wrapper_tmp"
    echo '' >> "$wrapper_tmp"
    echo 'exec {{ cli_dst }} "$@"' >> "$wrapper_tmp"
    sudo install -m755 "$wrapper_tmp" {{ wrapper_dst }}
    rm -f "$wrapper_tmp"

    # Setup COSMIC keyboard shortcut if available
    # Setup COSMIC keyboard shortcut
    if command -v cosmic-panel &> /dev/null; then
        COSMIC_SHORTCUTS_DIR="$HOME/.config/cosmic/com.system76.CosmicSettings.Shortcuts/v1"
        COSMIC_SHORTCUTS_FILE="$COSMIC_SHORTCUTS_DIR/custom"

        echo -n "Add COSMIC keyboard shortcut (Super+Space)? [Y/n]: "
        read -r add_shortcut

        if [[ ! "$add_shortcut" =~ ^[Nn]$ ]]; then
            mkdir -p "$COSMIC_SHORTCUTS_DIR"
            tts_command="{{ bin_dir }}/tts record --write"

            if [ -f "$COSMIC_SHORTCUTS_FILE" ] && [ -s "$COSMIC_SHORTCUTS_FILE" ]; then
                if ! grep -q "Super TTS" "$COSMIC_SHORTCUTS_FILE"; then
                    if ! (grep -q 'key: "space"' "$COSMIC_SHORTCUTS_FILE" && grep -A5 -B5 'key: "space"' "$COSMIC_SHORTCUTS_FILE" | grep -q 'Super'); then
                        cp "$COSMIC_SHORTCUTS_FILE" "$COSMIC_SHORTCUTS_FILE.backup"
                        temp_file=$(mktemp)
                        if grep -q '^{}$' "$COSMIC_SHORTCUTS_FILE"; then
                            echo '{' > "$temp_file"
                        else
                            head -n -1 "$COSMIC_SHORTCUTS_FILE" > "$temp_file"
                        fi
                        echo '    (' >> "$temp_file"
                        echo '        modifiers: [' >> "$temp_file"
                        echo '            Super,' >> "$temp_file"
                        echo '        ],' >> "$temp_file"
                        echo '        key: "space",' >> "$temp_file"
                        echo '        description: Some("Super TTS"),' >> "$temp_file"
                        echo "    ): Spawn(\"$tts_command\")," >> "$temp_file"
                        echo '}' >> "$temp_file"
                        mv "$temp_file" "$COSMIC_SHORTCUTS_FILE"
                        rm -f "$COSMIC_SHORTCUTS_FILE.backup"
                    fi
                fi
            else
                echo '{' > "$COSMIC_SHORTCUTS_FILE"
                echo '    (' >> "$COSMIC_SHORTCUTS_FILE"
                echo '        modifiers: [' >> "$COSMIC_SHORTCUTS_FILE"
                echo '            Super,' >> "$COSMIC_SHORTCUTS_FILE"
                echo '        ],' >> "$COSMIC_SHORTCUTS_FILE"
                echo '        key: "space",' >> "$COSMIC_SHORTCUTS_FILE"
                echo '        description: Some("Super TTS"),' >> "$COSMIC_SHORTCUTS_FILE"
                echo "    ): Spawn(\"$tts_command\")," >> "$COSMIC_SHORTCUTS_FILE"
                echo '}' >> "$COSMIC_SHORTCUTS_FILE"
            fi
        fi
    fi || true

    echo "✓ Super TTS installed to {{ daemon_dst }}"
    echo "✓ Wrapper script created at {{ wrapper_dst }}"
    echo "✓ Convenience shortcut 'tts' created"
    echo ""
    echo "🚀 Ready to use!"
    echo "-- tts speak \"hello\"          # Speak some text"

    # Reload user systemd and enable service
    echo "Reloading user systemd..."
    systemctl --user daemon-reload

    echo "✓ Daemon installed successfully as user service!"
    echo ""
    # `restart`, not `start`: start is a no-op on an already-running
    # unit, which would leave a freshly installed binary unused (the old
    # process keeps running from the deleted inode).
    systemctl --user restart {{ service_name }}
    systemctl --user enable {{ service_name }}

# Install daemon, settings app, and CLI
# Usage: just install
install:
    #!/usr/bin/env bash
    if ! just install-daemon; then
        echo "❌ Daemon installation failed"
        exit 1
    fi

    if ! just install-app; then
        echo "❌ App installation failed"
        exit 1
    fi

# Configure COSMIC keyboard shortcut for Super TTS
setup-cosmic-shortcut:
    #!/usr/bin/env bash
    # Check if we're on COSMIC desktop
    if ! command -v cosmic-panel &> /dev/null; then
        echo "COSMIC desktop not detected"
        exit 0
    fi

    COSMIC_SHORTCUTS_DIR="$HOME/.config/cosmic/com.system76.CosmicSettings.Shortcuts/v1"
    COSMIC_SHORTCUTS_FILE="$COSMIC_SHORTCUTS_DIR/custom"

    # Ask user if they want to add the shortcut
    echo -n "Add keyboard shortcut (Super+Space) for Super TTS? [Y/n]: "
    read -r add_shortcut

    if [[ "$add_shortcut" =~ ^[Nn]$ ]]; then
        exit 0
    fi

    # Create the shortcuts directory if it doesn't exist
    mkdir -p "$COSMIC_SHORTCUTS_DIR"

    # Use the full path to the tts wrapper for reliability
    tts_command="{{ bin_dir }}/tts record --write"

    # Check if shortcuts file exists and has content
    if [ -f "$COSMIC_SHORTCUTS_FILE" ] && [ -s "$COSMIC_SHORTCUTS_FILE" ]; then
        # File exists with content, check if our shortcut is already there
        if grep -q "Super TTS" "$COSMIC_SHORTCUTS_FILE"; then
            exit 0
        fi

        # Check if Super+Space is already used
        if grep -q 'key: "space"' "$COSMIC_SHORTCUTS_FILE" && grep -A5 -B5 'key: "space"' "$COSMIC_SHORTCUTS_FILE" | grep -q 'Super'; then
            echo "Super+Space already in use"
            exit 0
        fi

        # Create a backup
        cp "$COSMIC_SHORTCUTS_FILE" "$COSMIC_SHORTCUTS_FILE.backup"

        # Create a temporary file with the new shortcut entry
        temp_file=$(mktemp)

        # Check if the file is empty (just {}) and handle accordingly
        if grep -q '^{}$' "$COSMIC_SHORTCUTS_FILE"; then
            # File is empty, replace entirely
            echo '{' > "$temp_file"
            echo '    (' >> "$temp_file"
            echo '        modifiers: [' >> "$temp_file"
            echo '            Super,' >> "$temp_file"
            echo '        ],' >> "$temp_file"
            echo '        key: "space",' >> "$temp_file"
            echo '        description: Some("Super TTS"),' >> "$temp_file"
            echo "    ): Spawn(\"$tts_command\")," >> "$temp_file"
            echo '}' >> "$temp_file"
        else
            # File has content, remove the closing brace and add our shortcut
            head -n -1 "$COSMIC_SHORTCUTS_FILE" > "$temp_file"
            echo '    (' >> "$temp_file"
            echo '        modifiers: [' >> "$temp_file"
            echo '            Super,' >> "$temp_file"
            echo '        ],' >> "$temp_file"
            echo '        key: "space",' >> "$temp_file"
            echo '        description: Some("Super TTS"),' >> "$temp_file"
            echo "    ): Spawn(\"$tts_command\")," >> "$temp_file"
            echo '}' >> "$temp_file"
        fi

        # Replace the original file
        mv "$temp_file" "$COSMIC_SHORTCUTS_FILE"

        # Verify the file is valid by checking it has proper structure
        if ! grep -q '^{' "$COSMIC_SHORTCUTS_FILE" || ! grep -q '^}' "$COSMIC_SHORTCUTS_FILE"; then
            mv "$COSMIC_SHORTCUTS_FILE.backup" "$COSMIC_SHORTCUTS_FILE"
            exit 1
        fi

        # Remove backup if successful
        rm -f "$COSMIC_SHORTCUTS_FILE.backup"
    else

        echo '{' > "$COSMIC_SHORTCUTS_FILE"
        echo '    (' >> "$COSMIC_SHORTCUTS_FILE"
        echo '        modifiers: [' >> "$COSMIC_SHORTCUTS_FILE"
        echo '            Super,' >> "$COSMIC_SHORTCUTS_FILE"
        echo '        ],' >> "$COSMIC_SHORTCUTS_FILE"
        echo '        key: "space",' >> "$COSMIC_SHORTCUTS_FILE"
        echo '        description: Some("Super TTS"),' >> "$COSMIC_SHORTCUTS_FILE"
        echo "    ): Spawn(\"$tts_command\")," >> "$COSMIC_SHORTCUTS_FILE"
        echo '}' >> "$COSMIC_SHORTCUTS_FILE"
    fi

# Install everything (daemon, app, and the shared COSMIC applet)
# Usage: just install-all
install-all:
    #!/usr/bin/env bash
    if ! just install; then
        echo "❌ Core installation failed"
        exit 1
    fi

    if ! just install-applet; then
        echo "❌ COSMIC applet installation failed"
        exit 1
    fi

    echo ""
    echo "🎉 Complete Super TTS installation finished!"
    echo ""
    echo "⚙️  Quick Setup Tips:"
    echo "-- If you're on COSMIC, the daemon installer already offered to set up Super+Space shortcut"
    echo "-- For other desktop environments, add a keyboard shortcut for: tts record --write"
    echo "-- Recommended shortcuts: Super+Space, Ctrl+Alt+S, or F12"

# Uninstall the app
uninstall-app:
    #!/usr/bin/env bash
    echo "Uninstalling Super TTS App..."
    sudo rm -f {{ app_dst }}
    sudo rm -f {{ app_desktop_file_dst }}
    sudo rm -f {{ app_icon_dst }}

    # Update desktop database
    if command -v update-desktop-database &> /dev/null; then
        sudo update-desktop-database {{ desktop_dir }} 2>/dev/null || true
    fi

    # Update icon cache
    if command -v gtk-update-icon-cache &> /dev/null; then
        sudo gtk-update-icon-cache -f -t {{ icon_theme_dir }} 2>/dev/null || true
    fi

    # Drop the entry from COSMIC's launcher caches without a relogin.
    pkill -f '^cosmic-app-library$' 2>/dev/null || true
    pkill -f '^cosmic-launcher$' 2>/dev/null || true
    pkill -f '^pop-launcher( |$)' 2>/dev/null || true

    echo "✓ Super TTS App uninstalled"
    echo "✓ Desktop entry removed"
    echo "✓ App icon removed"

# Uninstall the COSMIC applet: Super TTS's own, and the shared one unless
# Super STT, which uses it too, is still installed.
uninstall-applet:
    #!/usr/bin/env bash
    echo "Uninstalling the COSMIC applet..."
    sudo rm -f '{{ bin_dir }}/{{ old_applet }}' '{{ icons_dir }}/{{ old_applet }}.svg'
    for side in full left right; do
        sudo rm -f "{{ desktop_dir }}/{{ old_applet }}-$side.desktop"
    done
    if [ -e '{{ bin_dir }}/super-stt-daemon' ]; then
        echo "Super STT is installed and uses the shared applet, so it stays."
    else
        sudo rm -f '{{ bin_dir }}/{{ shared_applet }}' '{{ icons_dir }}/{{ shared_applet }}.svg'
        for side in full left right; do
            sudo rm -f "{{ desktop_dir }}/{{ shared_applet }}-$side.desktop"
        done
    fi

    # Drop the entries from COSMIC's launcher caches without a relogin.
    pkill -f '^cosmic-app-library$' 2>/dev/null || true
    pkill -f '^cosmic-launcher$' 2>/dev/null || true
    pkill -f '^pop-launcher( |$)' 2>/dev/null || true

    echo "✓ COSMIC applet uninstalled"

# Uninstall the daemon
uninstall-daemon:
    #!/usr/bin/env bash
    echo "Uninstalling Super TTS daemon user service..."

    # Stop and disable user service
    systemctl --user stop {{ service_name }} || true
    systemctl --user disable {{ service_name }} || true

    # Remove service file (also any legacy user-local copy)
    sudo rm -f {{ service_dst }}
    rm -f "$HOME/.config/systemd/user/{{ service_file }}"

    # Remove binary
    sudo rm -f {{ daemon_dst }}

    # Remove the co-located consent helper — without it the daemon
    # would deny every auth_request, so it's part of the daemon's
    # install contract.
    sudo rm -f {{ consent_dst }}

    sudo rm -f {{ wrapper_dst }}

    # Remove directories (but preserve logs)
    rm -rf {{ run_dir }}
    echo "Log directory {{ log_dir }} preserved"

    # Reload user systemd
    systemctl --user daemon-reload

    echo "✓ Super TTS Daemon user service uninstalled"

# Install just the consent helper (normally bundled with install-daemon)
install-consent:
    #!/usr/bin/env bash
    # Ask for sudo up front and keep the timestamp alive in the
    # background: the build can outlast sudo's credential cache, and a
    # password prompt buried in build output is easy to miss.
    sudo -v
    ( while sudo -n -v 2>/dev/null; do sleep 60; done ) &
    sudo_keepalive=$!
    trap 'kill "$sudo_keepalive" 2>/dev/null' EXIT

    if ! just build-consent; then
        echo "❌ Consent helper build failed"
        exit 1
    fi
    if [ ! -f "{{ consent_src }}" ]; then
        echo "❌ Consent helper binary not found at {{ consent_src }}"
        exit 1
    fi
    sudo mkdir -p {{ bin_dir }}
    sudo install -m755 {{ consent_src }} {{ consent_dst }}
    echo "✓ Consent helper installed: {{ consent_dst }}"

# Uninstall the consent helper.
uninstall-consent:
    #!/usr/bin/env bash
    echo "Uninstalling Super TTS consent helper..."
    sudo rm -f {{ consent_dst }}
    echo "✓ Consent helper uninstalled"

# Install the CLI binary (system installation under /usr/local)
install-cli:
    #!/usr/bin/env bash
    # Ask for sudo up front and keep the timestamp alive in the
    # background: the build can outlast sudo's credential cache, and a
    # password prompt buried in build output is easy to miss.
    sudo -v
    ( while sudo -n -v 2>/dev/null; do sleep 60; done ) &
    sudo_keepalive=$!
    trap 'kill "$sudo_keepalive" 2>/dev/null' EXIT

    echo "Building CLI..."
    if ! just build-cli; then
        echo "❌ CLI build failed or was interrupted"
        exit 1
    fi

    if [ ! -f "{{ cli_src }}" ]; then
        echo "❌ CLI binary not found at {{ cli_src }}"
        exit 1
    fi

    sudo mkdir -p {{ bin_dir }}
    sudo install -m755 {{ cli_src }} {{ cli_dst }}
    echo "✓ Super TTS CLI installed: {{ cli_dst }}"

# Uninstall the CLI binary
uninstall-cli:
    #!/usr/bin/env bash
    echo "Uninstalling Super TTS CLI..."
    sudo rm -f {{ cli_dst }}

    # The `tts` wrapper is a thin exec of the CLI, so it is dead weight
    # without it — left in place it stays on PATH and fails at exec time
    # instead of reporting as uninstalled.
    sudo rm -f {{ wrapper_dst }}
    echo "✓ Super TTS CLI uninstalled"

# Uninstall daemon, app, applet, CLI, and consent helper
uninstall: uninstall-daemon uninstall-app uninstall-applet uninstall-cli uninstall-consent

# Start the daemon user service
start-daemon:
    systemctl --user start {{ service_name }}

# Stop the daemon user service
stop-daemon:
    systemctl --user stop {{ service_name }}

# Enable daemon to start with user session
enable-daemon:
    systemctl --user enable {{ service_name }}

# Disable daemon from starting with user session
disable-daemon:
    systemctl --user disable {{ service_name }}

# Check daemon status
status-daemon:
    systemctl --user status {{ service_name }}

# Show overall system status and test connectivity
status: status-daemon
    #!/usr/bin/env bash
    echo ""
    echo "🔍 Super TTS System Status"
    echo "=========================="
    echo ""

    # Check if app is installed
    if command -v tts &> /dev/null; then
        echo "✅ App tools: Installed (tts command available)"
    elif [ -f "{{ app_dst }}" ]; then
        echo "✅ Super TTS App: Installed"
    else
        echo "❌ Super TTS App: Not installed"
    fi

    # Check if daemon binary exists
    if [ -f "{{ daemon_dst }}" ]; then
        echo "✅ Daemon binary: Installed"
    else
        echo "❌ Daemon binary: Not installed"
    fi

    # Check if CLI binary exists
    if [ -f "{{ cli_dst }}" ]; then
        echo "✅ CLI binary: Installed"
    else
        echo "❌ CLI binary: Not installed"
    fi

    # Check if cosmic applet is installed
    if [ -f "{{ bin_dir }}/{{ shared_applet }}" ]; then
        echo "✅ COSMIC applet: Installed"
    else
        echo "❌ COSMIC applet: Not installed"
    fi

# View daemon logs
logs-daemon:
    journalctl --user -u {{ service_name }} -f

# View recent daemon logs
logs-daemon-recent:
    journalctl --user -u {{ service_name }} -n 50

# Restart the daemon user service
restart-daemon:
    systemctl --user restart {{ service_name }}

# Pin super-engine at one commit: every super-engine crate in Cargo.toml, and
# the shared CI jobs the workflows in .github/workflows run from it, so the
# installer and the script that tests it come from the same engine commit.
# Then updates Cargo.lock. `rev` is anything GitHub resolves to a commit (a
# full or short hash, or `main`). It must be pushed: CI fetches the jobs from
# GitHub.
# Usage: just pin-engine <rev>
[doc("Pin super-engine's crates and shared CI jobs at one commit")]
pin-engine rev:
    #!/usr/bin/env bash
    set -euo pipefail
    sha=$(curl -fsSL -H 'Accept: application/vnd.github.sha' \
      "https://api.github.com/repos/super-libre/super-engine/commits/{{ rev }}") || {
      echo "pin-engine: GitHub has no super-engine commit '{{ rev }}'. Push it first." >&2
      exit 1
    }
    python3 - "$sha" <<'PYEOF'
    import glob
    import re
    import sys

    sha = sys.argv[1]
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        sys.exit(f"pin-engine: GitHub answered {sha!r}, not a commit hash")
    git = r'git\s*=\s*"https://github\.com/super-libre/super-engine"'
    crate = re.compile(rf'({git},\s*rev\s*=\s*")[^"]*')
    job = re.compile(r"(super-libre/super-engine/\.github/[\w./-]+@)[^\s#]+")

    def pinned(path, pattern):
        with open(path) as f:
            return pattern.subn(lambda m: m.group(1) + sha, f.read())

    # Every file is checked before any is written, so a refusal changes nothing.
    manifest, crates = pinned("Cargo.toml", crate)
    engine_deps = len(re.findall(git, manifest))
    if crates == 0 or crates != engine_deps:
        sys.exit(
            f"pin-engine: found a rev on {crates} of the {engine_deps} super-engine "
            "crates in Cargo.toml. Each one needs `git = ...` followed by `rev = ...`."
        )
    workflows = {p: pinned(p, job) for p in sorted(glob.glob(".github/workflows/*.yml"))}

    def write(path, text):
        with open(path, "w") as f:
            f.write(text)

    write("Cargo.toml", manifest)
    for path, (text, _) in workflows.items():
        write(path, text)
    jobs = sum(count for _, count in workflows.values())
    print(f"Pinned {crates} crates and {jobs} CI jobs at {sha}")
    PYEOF
    cargo metadata --format-version 1 > /dev/null

# Vendor dependencies locally
vendor:
    #!/usr/bin/env bash
    mkdir -p .cargo
    cargo vendor --sync Cargo.toml | head -n -1 > .cargo/config.toml
    echo 'directory = "vendor"' >> .cargo/config.toml
    echo >> .cargo/config.toml
    echo '[env]' >> .cargo/config.toml
    if [ -n "${SOURCE_DATE_EPOCH}" ]
    then
        source_date="$(date -d "@${SOURCE_DATE_EPOCH}" "+%Y-%m-%d")"
        echo "VERGEN_GIT_COMMIT_DATE = \"${source_date}\"" >> .cargo/config.toml
    fi
    if [ -n "${SOURCE_GIT_HASH}" ]
    then
        echo "VERGEN_GIT_SHA = \"${SOURCE_GIT_HASH}\"" >> .cargo/config.toml
    fi
    tar pcf vendor.tar .cargo vendor
    rm -rf .cargo vendor

# Extracts vendored dependencies
vendor-extract:
    rm -rf vendor
    tar pxf vendor.tar
