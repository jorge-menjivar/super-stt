app_name := 'super-stt-app'
daemon_name := 'super-stt'
wrapper_name := 'stt'
cosmic_name := 'cosmic-ext'
applet_name := 'super-stt-cosmic-applet'

# Applet
applet_full_desktop_file_name := 'super-stt-cosmic-applet-full.desktop'
applet_left_desktop_file_name := 'super-stt-cosmic-applet-left.desktop'
applet_right_desktop_file_name := 'super-stt-cosmic-applet-right.desktop'

# Installation paths
home_dir := env_var('HOME')
user_bin_dir := home_dir / '.local' / 'bin'
user_prefix := home_dir / '.local'
system_bin_dir := '/usr/local/bin'
user_systemd_dir := home_dir / '.config' / 'systemd' / 'user'
run_dir := env_var('XDG_RUNTIME_DIR') / 'stt'
log_dir := home_dir / '.local' / 'share' / 'stt' / 'logs'
user_desktop_dir := home_dir / '.local' / 'share' / 'applications'
user_icons_dir := home_dir / '.local' / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps'

# Binary paths
app_src := 'target' / 'release' / app_name
daemon_src := 'target' / 'release' / daemon_name
cosmic_src := 'target' / 'release' / cosmic_name
applet_src := 'target' / 'release' / applet_name
debug_applet_src := 'target' / 'debug' / applet_name
app_dst := user_bin_dir / app_name
daemon_dst := user_bin_dir / daemon_name
cosmic_dst := user_bin_dir / cosmic_name
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
service_file := daemon_name + '.service'
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
    cargo build {{args}}

# Compiles with release profile
# Usage: just build-release [--cuda|--cudnn]
build-release *args:
    cargo build --release {{args}}

# Compiles release profile with vendored dependencies
# Usage: just build-vendored [--cuda|--cudnn]
build-vendored *args: vendor-extract
    just build-release --frozen --offline {{args}}

# Runs a clippy check
check *args:
    cargo clippy --all-features --workspace {{args}} -- -W clippy::pedantic -D warnings

# Runs a clippy check with JSON message format
check-json: (check '--message-format=json')

# Run the app for testing purposes
run-app *args:
    env RUST_BACKTRACE=full RUST_LOG=super_stt_app=debug,super_stt_shared=debug cargo run --bin {{app_name}} {{args}}

# Run the daemon for testing purposes
# Usage: just run-daemon [--model MODEL] [other args]
run-daemon *args:
    env RUST_BACKTRACE=full RUST_LOG=super_stt=debug cargo run --bin {{daemon_name}} -v {{args}}

# Run security audit to check for vulnerabilities
audit:
    cargo audit

# Run the cosmic applet in the cosmic panel for testing purposes
run-applet *args:
    #!/usr/bin/env bash
    set -euo pipefail

    env RUST_BACKTRACE=full RUST_LOG=debug,super_stt_shared=debug,warn cargo build --bin {{applet_name}} {{args}}

    echo "Installing Debug Super STT COSMIC applet..."
    mkdir -p {{user_bin_dir}}
    install -m755 {{debug_applet_src}} {{applet_dst}}

    # Install the debug desktop entries for panel integration
    mkdir -p {{user_desktop_dir}}

    echo "Installing desktop entries for COSMIC panel integration..."
    install -Dm0644 {{applet_full_desktop_file_src}} {{applet_full_desktop_file_dst}}
    install -Dm0644 {{applet_left_desktop_file_src}} {{applet_left_desktop_file_dst}}
    install -Dm0644 {{applet_right_desktop_file_src}} {{applet_right_desktop_file_dst}}

    # Install the applet icon
    mkdir -p {{user_icons_dir}}
    echo "Installing applet icon..."
    install -Dm0644 {{applet_icon_src}} {{applet_icon_dst}}

    cosmic-panel

run-applet-windowed *args:
    env RUST_BACKTRACE=full RUST_LOG=debug,super_stt_shared=debug,warn cargo run --bin {{applet_name}} {{args}}

# Run the cosmic applet in the cosmic panel for testing purposes
run-applet-kill *args:
    #!/usr/bin/env bash
    set -euo pipefail

    env RUST_BACKTRACE=full RUST_LOG=debug,super_stt_shared=debug,warn cargo build --bin {{applet_name}} {{args}}

    echo "Installing Debug Super STT COSMIC applet..."
    mkdir -p {{user_bin_dir}}
    install -m755 {{debug_applet_src}} {{applet_dst}}

    # Install the debug desktop entries for panel integration
    mkdir -p {{user_desktop_dir}}

    echo "Installing desktop entries for COSMIC panel integration..."
    install -Dm0644 {{applet_full_desktop_file_src}} {{applet_full_desktop_file_dst}}
    install -Dm0644 {{applet_left_desktop_file_src}} {{applet_left_desktop_file_dst}}
    install -Dm0644 {{applet_right_desktop_file_src}} {{applet_right_desktop_file_dst}}

    # Install the applet icon
    mkdir -p {{user_icons_dir}}
    echo "Installing applet icon..."
    install -Dm0644 {{applet_icon_src}} {{applet_icon_dst}}

    # Restart cosmic panel for changes to take effect
    pkill -f cosmic-panel || true

    echo "Running cosmic-panel in this terminal..."
    cosmic-panel

# Run the cosmic applet for testing purposes with different sides
run-applet-left *args:
    env RUST_BACKTRACE=full RUST_LOG=debug,super_stt_shared=debug,warn cargo run --bin {{applet_name}} {{args}} -- --side left

run-applet-right *args:
    env RUST_BACKTRACE=full RUST_LOG=debug,super_stt_shared=debug,warn cargo run --bin {{applet_name}} {{args}} -- --side right

run-applet-full *args:
    env RUST_BACKTRACE=full RUST_LOG=debug,super_stt_shared=debug,warn cargo run --bin {{applet_name}} {{args}} -- --side full

# Build only the app
build-app *args:
    cargo build --release --bin {{app_name}} {{args}}

# Build only the daemon
build-daemon *args:
    cargo build --release --bin {{daemon_name}} {{args}}

# Build only the cosmic applet
build-applet:
    echo "🔧 Building COSMIC applet..."
    cargo build --release --bin {{applet_name}}

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
    if [ ! -f "{{app_src}}" ]; then
        echo "❌ App binary not found at {{app_src}}"
        exit 1
    fi

    echo "Installing Super STT app to {{app_dst}}"
    mkdir -p {{user_bin_dir}}
    install -m755 {{app_src}} {{app_dst}}

    # Install the desktop entry for application menu
    mkdir -p {{user_desktop_dir}}
    echo "Installing desktop entry..."
    install -Dm0644 {{app_desktop_file_src}} {{app_desktop_file_dst}}

    # Install the app icon
    mkdir -p {{user_icons_dir}}
    echo "Installing app icon..."
    install -Dm0644 {{app_icon_src}} {{app_icon_dst}}

    # Update desktop database
    if command -v update-desktop-database &> /dev/null; then
        update-desktop-database {{user_desktop_dir}} 2>/dev/null || true
    fi

    # Update icon cache
    if command -v gtk-update-icon-cache &> /dev/null; then
        gtk-update-icon-cache {{user_icons_dir}} 2>/dev/null || true
    fi

    echo "✓ Super STT app installed: {{app_dst}}"
    echo "✓ Desktop entry installed: {{app_desktop_file_dst}}"
    echo "✓ App icon installed: {{app_icon_dst}}"


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
    if [ ! -f "{{applet_src}}" ]; then
        echo "❌ COSMIC applet binary not found at {{applet_src}}"
        exit 1
    fi

    echo "Installing Super STT COSMIC applet..."
    mkdir -p {{user_bin_dir}}
    install -m755 {{applet_src}} {{applet_dst}}

    # Install the desktop entries for panel integration
    mkdir -p {{user_desktop_dir}}

    echo "Installing desktop entries for COSMIC panel integration..."
    install -Dm0644 {{applet_full_desktop_file_src}} {{applet_full_desktop_file_dst}}
    install -Dm0644 {{applet_left_desktop_file_src}} {{applet_left_desktop_file_dst}}
    install -Dm0644 {{applet_right_desktop_file_src}} {{applet_right_desktop_file_dst}}

    # Install the applet icon
    mkdir -p {{user_icons_dir}}
    echo "Installing applet icon..."
    install -Dm0644 {{applet_icon_src}} {{applet_icon_dst}}

    echo "✓ COSMIC applet installed: {{applet_dst}}"
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
    args_array=({{args}})
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

    if [[ "{{args}}" == *"--cudnn"* ]]; then
        if ! just build-daemon --features "cuda,cudnn"; then
            echo "❌ Daemon build failed or was interrupted"
            exit 1
        fi
    elif [[ "{{args}}" == *"--cuda"* ]]; then
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
    if [ ! -f "{{daemon_src}}" ]; then
        echo "❌ Daemon binary not found at {{daemon_src}}"
        exit 1
    fi

    echo "Installing Super STT daemon as user service..."

    # Setup stt group for secure socket access
    if [ -f "scripts/setup-stt-group.sh" ]; then
        echo "Setting up stt group for secure access..."
        bash scripts/setup-stt-group.sh || true
    fi

    # Install binary
    echo "Installing daemon binary to {{daemon_dst}}"
    mkdir -p {{user_bin_dir}}
    install -m755 {{daemon_src}} {{daemon_dst}}

    # Create user directories
    echo "Creating user directories..."
    mkdir -p {{run_dir}}
    mkdir -p {{log_dir}}
    mkdir -p {{user_systemd_dir}}

    # Copy the service file from the repo
    echo "Installing user systemd service file..."
    cp super-stt/systemd/super-stt.service {{service_dst}}

    # Add model parameter to ExecStart if specified
    if [[ -n "$model" ]]; then
        echo "Configuring daemon to use model: $model"
        sed -i "s|--socket %t/stt/super-stt.sock|--socket %t/stt/super-stt.sock --model $model|" {{service_dst}}
    fi

    # Create wrapper script for automatic group switching
    echo "Creating wrapper script at {{wrapper_dst}}"
    echo '#!/bin/bash' > {{wrapper_dst}}
    echo '# Super STT Daemon Wrapper' >> {{wrapper_dst}}
    echo '# This wrapper automatically uses the stt group for socket access' >> {{wrapper_dst}}
    echo '' >> {{wrapper_dst}}
    echo 'exec sg stt -c "{{daemon_dst}} $*"' >> {{wrapper_dst}}
    chmod +x {{wrapper_dst}}

    # Setup COSMIC keyboard shortcut if available
    # Setup COSMIC keyboard shortcut
    if command -v cosmic-panel &> /dev/null; then
        COSMIC_SHORTCUTS_DIR="$HOME/.config/cosmic/com.system76.CosmicSettings.Shortcuts/v1"
        COSMIC_SHORTCUTS_FILE="$COSMIC_SHORTCUTS_DIR/custom"

        echo -n "Add COSMIC keyboard shortcut (Super+Space)? [Y/n]: "
        read -r add_shortcut

        if [[ ! "$add_shortcut" =~ ^[Nn]$ ]]; then
            mkdir -p "$COSMIC_SHORTCUTS_DIR"
            stt_command="{{user_bin_dir}}/stt record --write"

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

    if ! grep -q "{{user_bin_dir}}" "$shell_config" 2>/dev/null; then
        echo "Adding {{user_bin_dir}} to PATH in $shell_config"
        echo 'export PATH="{{user_bin_dir}}:$PATH"' >> "$shell_config"
        echo "⚠️  Restart your shell or run: source $shell_config"
    fi

    echo "✓ Super STT installed to {{daemon_dst}}"
    echo "✓ Wrapper script created at {{wrapper_dst}}"
    echo "✓ Convenience shortcut 'stt' created"
    echo ""
    echo "🚀 Ready to use!"
    echo "-- stt record --write         # Record, transcribe, and type result"

    # Reload user systemd and enable service
    echo "Reloading user systemd..."
    systemctl --user daemon-reload

    echo "✓ Daemon installed successfully as user service!"
    echo ""
    systemctl --user start {{daemon_name}}
    systemctl --user enable {{daemon_name}}


# Install both client and daemon
# Usage: just install [--cuda|--cudnn] [--model MODEL]
install *args:
    #!/usr/bin/env bash
    # Check if cuDNN or CUDA is requested and call commands with the right args
    if ! just install-daemon {{args}}; then
        echo "❌ Daemon installation failed"
        exit 1
    fi

    if ! just install-app; then
        echo "❌ App installation failed"
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
    stt_command="{{user_bin_dir}}/stt record --write"

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
    if ! just install {{args}}; then
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
    rm -f {{app_dst}}
    rm -f {{app_desktop_file_dst}}
    rm -f {{app_icon_dst}}

    # Update desktop database
    if command -v update-desktop-database &> /dev/null; then
        update-desktop-database {{user_desktop_dir}} 2>/dev/null || true
    fi

    # Update icon cache
    if command -v gtk-update-icon-cache &> /dev/null; then
        gtk-update-icon-cache {{user_icons_dir}} 2>/dev/null || true
    fi

    echo "✓ Super STT App uninstalled"
    echo "✓ Desktop entry removed"
    echo "✓ App icon removed"

# Uninstall the cosmic applet
uninstall-applet:
    #!/usr/bin/env bash
    echo "Uninstalling Super STT COSMIC applet..."
    rm -f {{applet_dst}}
    rm -f {{applet_full_desktop_file_dst}}
    rm -f {{applet_left_desktop_file_dst}}
    rm -f {{applet_right_desktop_file_dst}}
    # Remove the applet icon
    rm -f {{applet_icon_dst}}
    echo "✓ COSMIC applet uninstalled"
    echo "✓ Desktop entries removed"
    echo "✓ Applet icon removed"

# Uninstall the daemon
uninstall-daemon:
    #!/usr/bin/env bash
    echo "Uninstalling Super STT daemon user service..."

    # Stop and disable user service
    systemctl --user stop {{daemon_name}} || true
    systemctl --user disable {{daemon_name}} || true

    # Remove service file
    rm -f {{service_dst}}

    # Remove binary
    rm -f {{daemon_dst}}

    rm -f "{{user_bin_dir}}/stt"

    # Remove directories (but preserve logs)
    rm -rf {{run_dir}}
    echo "Log directory {{log_dir}} preserved"

    # Reload user systemd
    systemctl --user daemon-reload

    echo "✓ Super STT Daemon user service uninstalled"

# Uninstall both app and daemon
uninstall: uninstall-daemon uninstall-app uninstall-applet

# Start the daemon user service
start-daemon:
    systemctl --user start {{daemon_name}}

# Stop the daemon user service
stop-daemon:
    systemctl --user stop {{daemon_name}}

# Enable daemon to start with user session
enable-daemon:
    systemctl --user enable {{daemon_name}}

# Disable daemon from starting with user session
disable-daemon:
    systemctl --user disable {{daemon_name}}

# Check daemon status
status-daemon:
    systemctl --user status {{daemon_name}}

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
    elif [ -f "{{app_dst}}" ]; then
        echo "✅ Super STT App: Installed"
    else
        echo "❌ Super STT App: Not installed"
    fi

    # Check if daemon binary exists
    if [ -f "{{daemon_dst}}" ]; then
        echo "✅ Daemon binary: Installed"
    else
        echo "❌ Daemon binary: Not installed"
    fi

    # Check if cosmic applet is installed
    if [ -f "{{applet_dst}}" ]; then
        echo "✅ COSMIC applet: Installed"
    else
        echo "❌ COSMIC applet: Not installed"
    fi

# View daemon logs
logs-daemon:
    journalctl --user -u {{daemon_name}} -f

# View recent daemon logs
logs-daemon-recent:
    journalctl --user -u {{daemon_name}} -n 50

# Restart the daemon user service
restart-daemon:
    systemctl --user restart {{daemon_name}}

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
