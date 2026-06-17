app_name := 'super-stt-app'
daemon_bin_name := 'super-stt-daemon'
# systemd unit name (matches super-stt-daemon/systemd/super-stt.service)
service_name := 'super-stt'
cli_name := 'super-stt-cli'
consent_name := 'super-stt-consent'
wrapper_name := 'stt'
applet_name := 'super-stt-cosmic-applet'

# Applet
applet_full_desktop_file_name := 'super-stt-cosmic-applet-full.desktop'
applet_left_desktop_file_name := 'super-stt-cosmic-applet-left.desktop'
applet_right_desktop_file_name := 'super-stt-cosmic-applet-right.desktop'

# Installation paths
home_dir := env('HOME')
user_bin_dir := home_dir / '.local' / 'bin'
user_systemd_dir := home_dir / '.config' / 'systemd' / 'user'
run_dir := env('XDG_RUNTIME_DIR') / 'stt'
log_dir := home_dir / '.local' / 'share' / 'stt' / 'logs'
user_desktop_dir := home_dir / '.local' / 'share' / 'applications'
user_icons_dir := home_dir / '.local' / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps'

# Binary paths
app_src := 'target' / 'release' / app_name
daemon_src := 'target' / 'release' / daemon_bin_name
cli_src := 'target' / 'release' / cli_name
consent_src := 'target' / 'release' / consent_name
applet_src := 'target' / 'release' / applet_name
debug_applet_src := 'target' / 'debug' / applet_name
app_dst := user_bin_dir / app_name
daemon_dst := user_bin_dir / daemon_bin_name
cli_dst := user_bin_dir / cli_name
consent_dst := user_bin_dir / consent_name
applet_dst := user_bin_dir / applet_name
wrapper_dst := user_bin_dir / wrapper_name

# App files
app_desktop_file_name := 'super-stt-app.desktop'
app_desktop_file_src := 'super-stt-app' / 'resources' / 'app.desktop'
app_icon_src := 'super-stt-app' / 'resources' / 'icons' / 'hicolor' / 'scalable' / 'apps' / 'super-stt-app.svg'
app_desktop_file_dst := user_desktop_dir / app_desktop_file_name
app_icon_dst := user_icons_dir / 'super-stt-app.svg'

# Applet files
applet_full_desktop_file_src := 'super-stt-cosmic-applet' / 'resources' / applet_full_desktop_file_name
applet_left_desktop_file_src := 'super-stt-cosmic-applet' / 'resources' / applet_left_desktop_file_name
applet_right_desktop_file_src := 'super-stt-cosmic-applet' / 'resources' / applet_right_desktop_file_name
applet_icon_src := 'super-stt-cosmic-applet' / 'resources' / 'icons' / 'hicolor' / 'scalable' / 'apps' / 'super-stt-cosmic-applet.svg'
applet_full_desktop_file_dst := user_desktop_dir / applet_full_desktop_file_name
applet_left_desktop_file_dst := user_desktop_dir / applet_left_desktop_file_name
applet_right_desktop_file_dst := user_desktop_dir / applet_right_desktop_file_name
applet_icon_dst := user_icons_dir / 'super-stt-cosmic-applet.svg'

# Service file
service_file := service_name + '.service'
service_dst := user_systemd_dir / service_file

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
# Usage: just build-debug [--cuda|--cudnn]
build-debug *args:
    cargo build {{ args }}

# Compiles with release profile
# Usage: just build-release [--cuda|--cudnn]
build-release *args:
    cargo build --release {{ args }}

# Compiles release profile with vendored dependencies
# Usage: just build-vendored [--cuda|--cudnn]
build-vendored *args: vendor-extract
    just build-release --frozen --offline {{ args }}

# Runs a clippy check
check *args:
    cargo clippy --all-features --workspace {{ args }} -- -W clippy::pedantic -D warnings -D unused_must_use

# Runs a clippy check with JSON message format
check-json: (check '--message-format=json')

# Check formatting without modifying files
fmt-check:
    cargo fmt --all -- --check

# Apply rustfmt to the whole workspace
fmt:
    cargo fmt --all

# Run the test suite. Usage: just test [--verbose]
test *args:
    cargo test {{ args }}

# Run doctests
doctest *args:
    cargo test --doc {{ args }}

# Verify the generated TOML schemas are current
schema-check:
    cargo test -p super-stt-registry-types --features schema

# Full local CI gate: format, lint, tests, doctests, schemas
ci: fmt-check check test doctest schema-check

# Run the app for testing purposes
run-app *args:
    env RUST_BACKTRACE=full RUST_LOG=super_stt_app=debug,super_stt_shared=debug cargo run --bin {{ app_name }} {{ args }}

# Run the daemon for testing purposes. Also builds super-stt-consent into
# the same target dir, since the daemon only looks for the consent helper
# alongside its own binary (auth_request popups fail without it).
# Usage: just run-daemon [cargo flags, e.g. --release]
run-daemon *args:
    cargo build --bin {{ consent_name }} --bin {{ daemon_bin_name }} {{ args }}
    env RUST_BACKTRACE=full RUST_LOG=super_stt_daemon=debug cargo run --bin {{ daemon_bin_name }} -v {{ args }}

# Run the CLI for testing purposes (talks to the running daemon over the HTTP socket)
# Usage: just run-cli [ping|status|record|stop|logout] [args]
run-cli *args:
    env RUST_BACKTRACE=full RUST_LOG=super_stt_cli=debug,super_stt_shared=debug cargo run --bin {{ cli_name }} -- {{ args }}

# Run security audit to check for vulnerabilities
audit:
    cargo audit

# Run the cosmic applet in the cosmic panel for testing purposes
run-applet *args:
    #!/usr/bin/env bash
    set -euo pipefail

    env RUST_BACKTRACE=full RUST_LOG=debug,super_stt_shared=debug,warn cargo build --bin {{ applet_name }} {{ args }}

    echo "Installing Debug Super STT COSMIC applet..."
    mkdir -p {{ user_bin_dir }}
    install -m755 {{ debug_applet_src }} {{ applet_dst }}

    # Install the debug desktop entries for panel integration
    mkdir -p {{ user_desktop_dir }}

    echo "Installing desktop entries for COSMIC panel integration..."
    install -Dm0644 {{ applet_full_desktop_file_src }} {{ applet_full_desktop_file_dst }}
    install -Dm0644 {{ applet_left_desktop_file_src }} {{ applet_left_desktop_file_dst }}
    install -Dm0644 {{ applet_right_desktop_file_src }} {{ applet_right_desktop_file_dst }}

    # Install the applet icon
    mkdir -p {{ user_icons_dir }}
    echo "Installing applet icon..."
    install -Dm0644 {{ applet_icon_src }} {{ applet_icon_dst }}

    cosmic-panel

run-applet-windowed *args:
    env RUST_BACKTRACE=full RUST_LOG=debug,super_stt_shared=debug,warn cargo run --bin {{ applet_name }} {{ args }}

# Run the cosmic applet in the cosmic panel for testing purposes
run-applet-kill *args:
    #!/usr/bin/env bash
    set -euo pipefail

    env RUST_BACKTRACE=full RUST_LOG=debug,super_stt_shared=debug,warn cargo build --bin {{ applet_name }} {{ args }}

    echo "Installing Debug Super STT COSMIC applet..."
    mkdir -p {{ user_bin_dir }}
    install -m755 {{ debug_applet_src }} {{ applet_dst }}

    # Install the debug desktop entries for panel integration
    mkdir -p {{ user_desktop_dir }}

    echo "Installing desktop entries for COSMIC panel integration..."
    install -Dm0644 {{ applet_full_desktop_file_src }} {{ applet_full_desktop_file_dst }}
    install -Dm0644 {{ applet_left_desktop_file_src }} {{ applet_left_desktop_file_dst }}
    install -Dm0644 {{ applet_right_desktop_file_src }} {{ applet_right_desktop_file_dst }}

    # Install the applet icon
    mkdir -p {{ user_icons_dir }}
    echo "Installing applet icon..."
    install -Dm0644 {{ applet_icon_src }} {{ applet_icon_dst }}

    # Restart cosmic panel for changes to take effect
    pkill -f cosmic-panel || true

    echo "Running cosmic-panel in this terminal..."
    cosmic-panel

# Run the cosmic applet for testing purposes with different sides
run-applet-left *args:
    env RUST_BACKTRACE=full RUST_LOG=debug,super_stt_shared=debug,warn cargo run --bin {{ applet_name }} {{ args }} -- --side left

run-applet-right *args:
    env RUST_BACKTRACE=full RUST_LOG=debug,super_stt_shared=debug,warn cargo run --bin {{ applet_name }} {{ args }} -- --side right

run-applet-full *args:
    env RUST_BACKTRACE=full RUST_LOG=debug,super_stt_shared=debug,warn cargo run --bin {{ applet_name }} {{ args }} -- --side full

# Build only the app
build-app *args:
    cargo build --release --bin {{ app_name }} {{ args }}

# Build only the daemon
build-daemon *args:
    cargo build --release --bin {{ daemon_bin_name }} {{ args }}

# Build only the CLI
build-cli *args:
    cargo build --release --bin {{ cli_name }} {{ args }}

# Build only the consent helper (co-located with the daemon binary)
build-consent:
    cargo build --release --bin {{ consent_name }}

# Build only the cosmic applet
build-applet:
    echo "🔧 Building COSMIC applet..."
    cargo build --release --bin {{ applet_name }}

# Build the OpenAI WASM backend component (wasm32-wasip2).
# Requires: rustup target add wasm32-wasip2
build-openai-backend:
    cargo build --manifest-path backends/openai/Cargo.toml --target wasm32-wasip2 --release

# Build the Mistral WASM backend component (wasm32-wasip2).
# Requires: rustup target add wasm32-wasip2
build-mistral-backend:
    cargo build --manifest-path backends/mistral/Cargo.toml --target wasm32-wasip2 --release

# Build the Deepgram WASM backend component (wasm32-wasip2).
# Requires: rustup target add wasm32-wasip2
build-deepgram-backend:
    cargo build --manifest-path backends/deepgram/Cargo.toml --target wasm32-wasip2 --release

# Copy the canonical WIT (realtime.wit + deps) into every backend that bundles it.
sync-wit:
    #!/usr/bin/env bash
    set -euo pipefail
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
    for dir in backends/*/wit; do
        if ! diff -q "$dir/realtime.wit" docs/protocol/wit/realtime.wit >/dev/null; then
            echo "WIT drift: $dir/realtime.wit" >&2; fail=1
        fi
        if ! diff -rq "$dir/deps" docs/protocol/wit/deps >/dev/null 2>&1; then
            echo "WIT deps drift: $dir/deps" >&2; fail=1
        fi
    done
    [ "$fail" -eq 0 ]

# Build the standalone Qwen3-ASR Python subprocess backend bundle.
# Pure wheel assembly (no compilation); produces a self-contained relocatable
# tarball under backends/qwen3-asr/target/. The cuda13 bundle is large.
# Usage: just build-qwen3-asr-backend [cpu|cuda13]
build-qwen3-asr-backend accel="cpu":
    backends/qwen3-asr/scripts/build_bundle.sh {{ accel }} backends/qwen3-asr/target

# Regenerate the JSON Schemas for backend.toml and registry.toml from the
# canonical Rust types in super-stt-registry-types. CI fails when the
# committed schemas are stale, so run this after changing those types.
gen-schemas:
    cargo run -p super-stt-registry-types --features schema --bin gen_schemas

# Serve a local offline registry for testing the Download/Install flow without
# any GitHub release or Pages setup. Builds the OpenAI + Mistral wasm
# components, stages them with their asset filenames, generates an index.json
# with real SHA-256 hashes, and serves the directory over HTTP. In the daemon's
# environment set `SUPER_STT_REGISTRY_URL=http://localhost:8787/index.json`
# before starting it, then open the app's Models > Download tab.
# Requires: `rustup target add wasm32-wasip2` (Python 3 is used only as the
# static file server at the end).
serve-test-registry port="8787": build-openai-backend build-mistral-backend
    #!/usr/bin/env bash
    set -euo pipefail
    out="target/test-registry"
    base="http://localhost:{{ port }}"
    mkdir -p "$out"
    cp backends/openai/target/wasm32-wasip2/release/super_stt_backend_openai.wasm "$out/openai.wasm"
    cp backends/mistral/target/wasm32-wasip2/release/super_stt_backend_mistral.wasm "$out/mistral.wasm"
    cargo run -q -p super-stt-indexer -- local --out "$out" --base-url "$base" \
        backends/openai/backend.toml backends/mistral/backend.toml
    echo ""
    echo "Test registry ready. In the daemon's environment, run:"
    echo "    export SUPER_STT_REGISTRY_URL=$base/index.json"
    echo "then restart the daemon and open Models > Download."
    echo ""
    echo "Serving $out at $base (Ctrl-C to stop)…"
    cd "$out" && exec python3 -m http.server {{ port }}

# Install built backends into the daemon's discovery directory
# (<XDG_DATA_HOME or ~/.local/share>/super-stt/backends/). Builds the OpenAI,
# Mistral, and Deepgram WASM components; the Qwen3-ASR Python bundle is
# installed only if already built (run
# `just build-qwen3-asr-backend [cpu|cuda13]` first).
install-backends: build-openai-backend build-mistral-backend build-deepgram-backend
    #!/usr/bin/env bash
    set -euo pipefail
    backends_dir="${XDG_DATA_HOME:-$HOME/.local/share}/super-stt/backends"

    # OpenAI (WASM component). backend.toml's entrypoint is "openai.wasm".
    openai_dir="$backends_dir/openai"
    mkdir -p "$openai_dir"
    cp backends/openai/backend.toml "$openai_dir/backend.toml"
    cp backends/openai/target/wasm32-wasip2/release/super_stt_backend_openai.wasm \
        "$openai_dir/openai.wasm"
    echo "Installed OpenAI backend -> $openai_dir"

    # Mistral (WASM component). backend.toml's entrypoint is "mistral.wasm".
    mistral_dir="$backends_dir/mistral"
    mkdir -p "$mistral_dir"
    cp backends/mistral/backend.toml "$mistral_dir/backend.toml"
    cp backends/mistral/target/wasm32-wasip2/release/super_stt_backend_mistral.wasm \
        "$mistral_dir/mistral.wasm"
    echo "Installed Mistral backend -> $mistral_dir"

    # Deepgram (WASM component). backend.toml's entrypoint is "deepgram.wasm".
    deepgram_dir="$backends_dir/deepgram"
    mkdir -p "$deepgram_dir"
    cp backends/deepgram/backend.toml "$deepgram_dir/backend.toml"
    cp backends/deepgram/target/wasm32-wasip2/release/super_stt_backend_deepgram.wasm \
        "$deepgram_dir/deepgram.wasm"
    echo "Installed Deepgram backend -> $deepgram_dir"

    # Qwen3-ASR (Python subprocess bundle). Installed only if a bundle has been
    # built; prefers the cuda13 bundle when present. Extracts over any existing
    # install so downloaded model weights under models/ are preserved.
    qwen_tarball=""
    for accel in cuda13 cpu; do
        cand="backends/qwen3-asr/target/qwen3-asr-x86_64-unknown-linux-gnu-$accel.tar.gz"
        if [ -f "$cand" ]; then qwen_tarball="$cand"; break; fi
    done
    if [ -n "$qwen_tarball" ]; then
        qwen_dir="$backends_dir/qwen3-asr"
        mkdir -p "$qwen_dir"
        # The tarball provides the heavy, relocatable runtime/. The launcher,
        # app/, and manifest are small source files — copy the current ones over
        # the extracted copies so a launcher/app tweak needs no bundle rebuild.
        # `install -m 0755` sets the executable bit the daemon requires to exec
        # the launcher; it is part of the recipe, never a manual step.
        tar -C "$qwen_dir" -xzf "$qwen_tarball"
        rm -rf "$qwen_dir/app"
        cp -r backends/qwen3-asr/app "$qwen_dir/app"
        mkdir -p "$qwen_dir/bin"
        install -m 0755 backends/qwen3-asr/bin/qwen3-asr "$qwen_dir/bin/qwen3-asr"
        cp backends/qwen3-asr/backend.toml "$qwen_dir/backend.toml"
        echo "Installed Qwen3-ASR backend ($(basename "$qwen_tarball")) -> $qwen_dir"
    else
        echo "Qwen3-ASR backend not built; run 'just build-qwen3-asr-backend [cpu|cuda13]' to enable it." >&2
    fi
    echo "Done. Restart the daemon (systemctl --user restart super-stt) to discover backends."

# Install the app (user-local installation)
install-app:
    #!/usr/bin/env bash
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

    echo "Installing Super STT app to {{ app_dst }}"
    mkdir -p {{ user_bin_dir }}
    install -m755 {{ app_src }} {{ app_dst }}

    # Install the desktop entry for application menu
    mkdir -p {{ user_desktop_dir }}
    echo "Installing desktop entry..."
    install -Dm0644 {{ app_desktop_file_src }} {{ app_desktop_file_dst }}

    # Install the app icon
    mkdir -p {{ user_icons_dir }}
    echo "Installing app icon..."
    install -Dm0644 {{ app_icon_src }} {{ app_icon_dst }}

    # Update desktop database
    if command -v update-desktop-database &> /dev/null; then
        update-desktop-database {{ user_desktop_dir }} 2>/dev/null || true
    fi

    # Update icon cache
    if command -v gtk-update-icon-cache &> /dev/null; then
        gtk-update-icon-cache {{ user_icons_dir }} 2>/dev/null || true
    fi

    echo "✓ Super STT app installed: {{ app_dst }}"
    echo "✓ Desktop entry installed: {{ app_desktop_file_dst }}"
    echo "✓ App icon installed: {{ app_icon_dst }}"

# Install the cosmic applet (user-local installation)
install-applet:
    #!/usr/bin/env bash
    # Build the cosmic applet first
    echo "Building COSMIC applet..."
    if ! just build-applet; then
        echo "❌ COSMIC applet build failed or was interrupted"
        exit 1
    fi

    # Check if binary exists
    if [ ! -f "{{ applet_src }}" ]; then
        echo "❌ COSMIC applet binary not found at {{ applet_src }}"
        exit 1
    fi

    echo "Installing Super STT COSMIC applet..."
    mkdir -p {{ user_bin_dir }}
    install -m755 {{ applet_src }} {{ applet_dst }}

    # Install the desktop entries for panel integration
    mkdir -p {{ user_desktop_dir }}

    echo "Installing desktop entries for COSMIC panel integration..."
    install -Dm0644 {{ applet_full_desktop_file_src }} {{ applet_full_desktop_file_dst }}
    install -Dm0644 {{ applet_left_desktop_file_src }} {{ applet_left_desktop_file_dst }}
    install -Dm0644 {{ applet_right_desktop_file_src }} {{ applet_right_desktop_file_dst }}

    # Install the applet icon
    mkdir -p {{ user_icons_dir }}
    echo "Installing applet icon..."
    install -Dm0644 {{ applet_icon_src }} {{ applet_icon_dst }}

    echo "✓ COSMIC applet installed: {{ applet_dst }}"
    echo "✓ Desktop entries installed for panel integration:"
    echo "  - Super STT Applet (Full)"
    echo "  - Super STT Applet (Left Side)"
    echo "  - Super STT Applet (Right Side)"
    echo ""
    echo "🚀 Ready to use! The applet can now be added to your COSMIC panel through:"
    echo "-- COSMIC Settings > Desktop > Panel > Configure panel applets > Add Applet"

# Install the daemon (user installation)
# Usage: just install-daemon [--cuda|--cudnn] [--model MODEL]
install-daemon *args:
    #!/usr/bin/env bash
    # Build the daemon first
    echo "Building daemon..."

    # Extract model parameter
    model=""
    args_array=({{ args }})
    for i in "${!args_array[@]}"; do
        if [[ "${args_array[$i]}" == "--model" ]]; then
            # Next argument is the model name
            if [[ $((i+1)) -lt ${#args_array[@]} ]]; then
                model="${args_array[$((i+1))]}"
            fi
            break
        elif [[ "${args_array[$i]}" == --model=* ]]; then
            model="${args_array[$i]#--model=}"
            break
        fi
    done

    if [[ "{{ args }}" == *"--cudnn"* ]]; then
        if ! just build-daemon --features "cuda,cudnn"; then
            echo "❌ Daemon build failed or was interrupted"
            exit 1
        fi
    elif [[ "{{ args }}" == *"--cuda"* ]]; then
        if ! just build-daemon --features "cuda"; then
            echo "❌ Daemon build failed or was interrupted"
            exit 1
        fi
    else
        if ! just build-daemon; then
            echo "❌ Daemon build failed or was interrupted"
            exit 1
        fi
    fi

    # Check if binary exists
    if [ ! -f "{{ daemon_src }}" ]; then
        echo "❌ Daemon binary not found at {{ daemon_src }}"
        exit 1
    fi

    echo "Installing Super STT daemon as user service..."

    # Setup stt group for secure socket access
    if [ -f "scripts/setup-stt-group.sh" ]; then
        echo "Setting up stt group for secure access..."
        bash scripts/setup-stt-group.sh || true
    fi

    # Install binary
    echo "Installing daemon binary to {{ daemon_dst }}"
    mkdir -p {{ user_bin_dir }}
    install -m755 {{ daemon_src }} {{ daemon_dst }}

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
    install -m755 {{ consent_src }} {{ consent_dst }}

    # Create user directories
    echo "Creating user directories..."
    mkdir -p {{ run_dir }}
    mkdir -p {{ log_dir }}
    mkdir -p {{ user_systemd_dir }}

    # Copy the service file from the repo
    echo "Installing user systemd service file..."
    cp super-stt-daemon/systemd/{{ service_file }} {{ service_dst }}

    # Add model parameter to ExecStart if specified
    if [[ -n "$model" ]]; then
        echo "Configuring daemon to use model: $model"
        sed -i "s|--socket %t/stt/super-stt.sock|--socket %t/stt/super-stt.sock --model $model|" {{ service_dst }}
    fi

    # Create the `stt` convenience wrapper (for keyboard shortcuts like
    # `stt record --write`). Note: we deliberately do NOT use `sg stt
    # -c "..."` here. Changing the GID of the daemon (or its CLI
    # peers) breaks `/proc/<pid>/exe` readlinks across the daemon ↔
    # client boundary — the kernel's `__ptrace_may_access` check
    # requires *both* matching UID and matching GID, and clients run
    # with the user's primary GID. The `stt` group is no longer
    # required for socket ACLs in the user-mode systemd unit; the
    # socket file is owned `user:user-primary-group` with 0660, so the
    # owner (same user) can access regardless of group membership.
    echo "Creating wrapper script at {{ wrapper_dst }}"
    echo '#!/bin/bash' > {{ wrapper_dst }}
    echo '# Super STT convenience wrapper — invokes super-stt-daemon directly.' >> {{ wrapper_dst }}
    echo '# Used by keyboard shortcuts (e.g. Super+Space → "stt record --write").' >> {{ wrapper_dst }}
    echo '' >> {{ wrapper_dst }}
    echo 'exec {{ daemon_dst }} "$@"' >> {{ wrapper_dst }}
    chmod +x {{ wrapper_dst }}

    # Setup COSMIC keyboard shortcut if available
    # Setup COSMIC keyboard shortcut
    if command -v cosmic-panel &> /dev/null; then
        COSMIC_SHORTCUTS_DIR="$HOME/.config/cosmic/com.system76.CosmicSettings.Shortcuts/v1"
        COSMIC_SHORTCUTS_FILE="$COSMIC_SHORTCUTS_DIR/custom"

        echo -n "Add COSMIC keyboard shortcut (Super+Space)? [Y/n]: "
        read -r add_shortcut

        if [[ ! "$add_shortcut" =~ ^[Nn]$ ]]; then
            mkdir -p "$COSMIC_SHORTCUTS_DIR"
            stt_command="{{ user_bin_dir }}/stt record --write"

            if [ -f "$COSMIC_SHORTCUTS_FILE" ] && [ -s "$COSMIC_SHORTCUTS_FILE" ]; then
                if ! grep -q "Super STT" "$COSMIC_SHORTCUTS_FILE"; then
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
                        echo '        description: Some("Super STT"),' >> "$temp_file"
                        echo "    ): Spawn(\"$stt_command\")," >> "$temp_file"
                        echo '}' >> "$temp_file"
                        mv "$temp_file" "$COSMIC_SHORTCUTS_FILE"
                        rm -f "$COSMIC_SHORTCUTS_FILE.backup"
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
                echo '        description: Some("Super STT"),' >> "$COSMIC_SHORTCUTS_FILE"
                echo "    ): Spawn(\"$stt_command\")," >> "$COSMIC_SHORTCUTS_FILE"
                echo '}' >> "$COSMIC_SHORTCUTS_FILE"
            fi
        fi
    fi || true

    # Update PATH in user's shell config
    shell_config="$HOME/.bashrc"
    if [[ "$SHELL" == *"zsh"* ]]; then
        shell_config="$HOME/.zshrc"
    fi

    if ! grep -q "{{ user_bin_dir }}" "$shell_config" 2>/dev/null; then
        echo "Adding {{ user_bin_dir }} to PATH in $shell_config"
        echo 'export PATH="{{ user_bin_dir }}:$PATH"' >> "$shell_config"
        echo "⚠️  Restart your shell or run: source $shell_config"
    fi

    echo "✓ Super STT installed to {{ daemon_dst }}"
    echo "✓ Wrapper script created at {{ wrapper_dst }}"
    echo "✓ Convenience shortcut 'stt' created"
    echo ""
    echo "🚀 Ready to use!"
    echo "-- stt record --write         # Record, transcribe, and type result"

    # Reload user systemd and enable service
    echo "Reloading user systemd..."
    systemctl --user daemon-reload

    echo "✓ Daemon installed successfully as user service!"
    echo ""
    systemctl --user start {{ service_name }}
    systemctl --user enable {{ service_name }}

# Install daemon, settings app, and CLI
# Usage: just install [--cuda|--cudnn] [--model MODEL]
install *args:
    #!/usr/bin/env bash
    # Check if cuDNN or CUDA is requested and call commands with the right args
    if ! just install-daemon {{ args }}; then
        echo "❌ Daemon installation failed"
        exit 1
    fi

    if ! just install-app; then
        echo "❌ App installation failed"
        exit 1
    fi

    if ! just install-cli; then
        echo "❌ CLI installation failed"
        exit 1
    fi

# Configure COSMIC keyboard shortcut for Super STT
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
    echo -n "Add keyboard shortcut (Super+Space) for Super STT? [Y/n]: "
    read -r add_shortcut

    if [[ "$add_shortcut" =~ ^[Nn]$ ]]; then
        exit 0
    fi

    # Create the shortcuts directory if it doesn't exist
    mkdir -p "$COSMIC_SHORTCUTS_DIR"

    # Use the full path to the stt wrapper for reliability
    stt_command="{{ user_bin_dir }}/stt record --write"

    # Check if shortcuts file exists and has content
    if [ -f "$COSMIC_SHORTCUTS_FILE" ] && [ -s "$COSMIC_SHORTCUTS_FILE" ]; then
        # File exists with content, check if our shortcut is already there
        if grep -q "Super STT" "$COSMIC_SHORTCUTS_FILE"; then
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
            echo '        description: Some("Super STT"),' >> "$temp_file"
            echo "    ): Spawn(\"$stt_command\")," >> "$temp_file"
            echo '}' >> "$temp_file"
        else
            # File has content, remove the closing brace and add our shortcut
            head -n -1 "$COSMIC_SHORTCUTS_FILE" > "$temp_file"
            echo '    (' >> "$temp_file"
            echo '        modifiers: [' >> "$temp_file"
            echo '            Super,' >> "$temp_file"
            echo '        ],' >> "$temp_file"
            echo '        key: "space",' >> "$temp_file"
            echo '        description: Some("Super STT"),' >> "$temp_file"
            echo "    ): Spawn(\"$stt_command\")," >> "$temp_file"
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
        echo '        description: Some("Super STT"),' >> "$COSMIC_SHORTCUTS_FILE"
        echo "    ): Spawn(\"$stt_command\")," >> "$COSMIC_SHORTCUTS_FILE"
        echo '}' >> "$COSMIC_SHORTCUTS_FILE"
    fi

# Install everything (daemon, app, and COSMIC applet)
# Usage: just install-all [--cuda|--cudnn] [--model MODEL]
install-all *args:
    #!/usr/bin/env bash
    if ! just install {{ args }}; then
        echo "❌ Core installation failed"
        exit 1
    fi

    if ! just install-cosmic-all; then
        echo "❌ COSMIC applet installation failed"
        exit 1
    fi

    echo ""
    echo "🎉 Complete Super STT installation finished!"
    echo ""
    echo "⚙️  Quick Setup Tips:"
    echo "-- If you're on COSMIC, the daemon installer already offered to set up Super+Space shortcut"
    echo "-- For other desktop environments, add a keyboard shortcut for: stt record --write"
    echo "-- Recommended shortcuts: Super+Space, Ctrl+Alt+S, or F12"

# Uninstall the app
uninstall-app:
    #!/usr/bin/env bash
    echo "Uninstalling Super STT App..."
    rm -f {{ app_dst }}
    rm -f {{ app_desktop_file_dst }}
    rm -f {{ app_icon_dst }}

    # Update desktop database
    if command -v update-desktop-database &> /dev/null; then
        update-desktop-database {{ user_desktop_dir }} 2>/dev/null || true
    fi

    # Update icon cache
    if command -v gtk-update-icon-cache &> /dev/null; then
        gtk-update-icon-cache {{ user_icons_dir }} 2>/dev/null || true
    fi

    echo "✓ Super STT App uninstalled"
    echo "✓ Desktop entry removed"
    echo "✓ App icon removed"

# Uninstall the cosmic applet
uninstall-applet:
    #!/usr/bin/env bash
    echo "Uninstalling Super STT COSMIC applet..."
    rm -f {{ applet_dst }}
    rm -f {{ applet_full_desktop_file_dst }}
    rm -f {{ applet_left_desktop_file_dst }}
    rm -f {{ applet_right_desktop_file_dst }}
    # Remove the applet icon
    rm -f {{ applet_icon_dst }}
    echo "✓ COSMIC applet uninstalled"
    echo "✓ Desktop entries removed"
    echo "✓ Applet icon removed"

# Uninstall the daemon
uninstall-daemon:
    #!/usr/bin/env bash
    echo "Uninstalling Super STT daemon user service..."

    # Stop and disable user service
    systemctl --user stop {{ service_name }} || true
    systemctl --user disable {{ service_name }} || true

    # Remove service file
    rm -f {{ service_dst }}

    # Remove binary
    rm -f {{ daemon_dst }}

    # Remove the co-located consent helper — without it the daemon
    # would deny every auth_request, so it's part of the daemon's
    # install contract.
    rm -f {{ consent_dst }}

    rm -f "{{ user_bin_dir }}/stt"

    # Remove directories (but preserve logs)
    rm -rf {{ run_dir }}
    echo "Log directory {{ log_dir }} preserved"

    # Reload user systemd
    systemctl --user daemon-reload

    echo "✓ Super STT Daemon user service uninstalled"

# Install just the consent helper (normally bundled with install-daemon)
install-consent:
    #!/usr/bin/env bash
    if ! just build-consent; then
        echo "❌ Consent helper build failed"
        exit 1
    fi
    if [ ! -f "{{ consent_src }}" ]; then
        echo "❌ Consent helper binary not found at {{ consent_src }}"
        exit 1
    fi
    mkdir -p {{ user_bin_dir }}
    install -m755 {{ consent_src }} {{ consent_dst }}
    echo "✓ Consent helper installed: {{ consent_dst }}"

# Uninstall the consent helper.
uninstall-consent:
    #!/usr/bin/env bash
    echo "Uninstalling Super STT consent helper..."
    rm -f {{ consent_dst }}
    echo "✓ Consent helper uninstalled"

# Install the CLI binary (user-local installation)
install-cli:
    #!/usr/bin/env bash
    echo "Building CLI..."
    if ! just build-cli; then
        echo "❌ CLI build failed or was interrupted"
        exit 1
    fi

    if [ ! -f "{{ cli_src }}" ]; then
        echo "❌ CLI binary not found at {{ cli_src }}"
        exit 1
    fi

    mkdir -p {{ user_bin_dir }}
    install -m755 {{ cli_src }} {{ cli_dst }}
    echo "✓ Super STT CLI installed: {{ cli_dst }}"

# Uninstall the CLI binary
uninstall-cli:
    #!/usr/bin/env bash
    echo "Uninstalling Super STT CLI..."
    rm -f {{ cli_dst }}
    echo "✓ Super STT CLI uninstalled"

# Uninstall daemon, app, applet, and CLI
uninstall: uninstall-daemon uninstall-app uninstall-applet uninstall-cli

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
    echo "🔍 Super STT System Status"
    echo "=========================="
    echo ""

    # Check if app is installed
    if command -v stt &> /dev/null; then
        echo "✅ App tools: Installed (stt command available)"
    elif [ -f "{{ app_dst }}" ]; then
        echo "✅ Super STT App: Installed"
    else
        echo "❌ Super STT App: Not installed"
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
    if [ -f "{{ applet_dst }}" ]; then
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
