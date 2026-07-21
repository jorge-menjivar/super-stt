#!/bin/bash

# Super STT Beta Installation Script
#
# Downloads and installs pre-built BETA binaries from a GitHub
# pre-release tag. This is the channel new users land on while the
# rewritten daemon protocol is still stabilizing; install-stable.sh
# remains responsible for the legacy single-`super-stt` layout the
# pre-protocol releases shipped.
#
# Differences vs the stable channel:
#   - Workspace was split into separate binaries
#     (super-stt-daemon, super-stt-cli, super-stt-consent,
#     super-stt-app, super-stt-cosmic-applet).
#   - The daemon now spawns a libcosmic consent helper for
#     /auth/request prompts, so super-stt-consent MUST be installed
#     next to super-stt-daemon. Missing helper = every auth fails
#     with popup_failed.
#   - The `stt` wrapper invokes super-stt-daemon directly. The
#     legacy `sg stt -c` indirection is gone because it broke
#     /proc/<pid>/exe readlinks (kernel ptrace_may_access requires
#     UID+GID match) and that broke peer identification in the new
#     protocol.
#   - The `stt` group is no longer required for socket ACLs — the
#     unit binds the socket as the user's primary group with 0660.
#
# This script is invoked indirectly via the top-level install.sh
# dispatcher when the user passes `--beta`.

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
BLINK='\033[5m'
BG_YELLOW='\033[43m'
BG_RED='\033[41m'
NC='\033[0m'

# Default values
INSTALL_PREFIX="$HOME/.local"
GITHUB_REPO="jorge-menjivar/super-stt"
VERSION="latest"
INSTALL_OPTION="all"

# Parse arguments
INTERACTIVE=true

TEMP_DIR=$(mktemp -d)
# Clean up the tarball + extracted binaries on any exit, including early
# failures under `set -e`; otherwise each run leaks a multi-hundred-MB
# /tmp/tmp.XXXX until reboot.
trap 'rm -rf "$TEMP_DIR"' EXIT

echo "Temp directory: $TEMP_DIR"

DESKTOP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons"
METAINFO_DIR="$HOME/.local/share/metainfo"
USER_SYSTEMD_DIR="$HOME/.config/systemd/user"

# Print functions
print_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
print_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Detect system architecture
detect_arch() {
    local arch=$(uname -m)
    case "$arch" in
        x86_64) echo "x86_64-unknown-linux-gnu" ;;
        aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
        *) print_error "Unsupported architecture: $arch"; exit 1 ;;
    esac
}

# Install daemon, CLI, and the consent helper.
#
# The consent helper MUST live alongside the daemon binary —
# super-stt-daemon's locate_consent_helper looks only in
# `current_exe().parent()`, no PATH fallback. Without it the daemon
# logs "super-stt-consent not found alongside daemon binary" and
# every /auth/request returns 403 auth_denied / popup_failed.
install_daemon() {
    mkdir -p "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/stt"
    mkdir -p "$HOME/.local/share/stt/logs"
    mkdir -p "$HOME/.config/systemd/user"

    print_info "Installing daemon, CLI, and consent helper..."
    mkdir -p "$INSTALL_PREFIX/bin"

    install -m 755 "$TEMP_DIR/super-stt-daemon" "$INSTALL_PREFIX/bin/"
    install -m 755 "$TEMP_DIR/super-stt-cli" "$INSTALL_PREFIX/bin/"
    install -m 755 "$TEMP_DIR/super-stt-consent" "$INSTALL_PREFIX/bin/"

    # The `stt` convenience wrapper used by keyboard shortcuts (e.g.
    # Super+Space → `stt record --write`). Invokes the daemon binary
    # directly — no `sg stt -c` indirection. Changing the GID of the
    # daemon's CLI peers breaks `/proc/<pid>/exe` readlinks across
    # the daemon ↔ client boundary because the kernel's
    # __ptrace_may_access check requires both UID AND GID to match.
    cat > "$INSTALL_PREFIX/bin/stt" << EOF
#!/bin/bash
# Super STT convenience wrapper — invokes super-stt-cli directly.
# Used by keyboard shortcuts (e.g. Super+Space → "stt record --write").
exec "$INSTALL_PREFIX/bin/super-stt-cli" "\$@"
EOF
    chmod 755 "$INSTALL_PREFIX/bin/stt"
}

# Install desktop app
install_app() {
    print_info "Installing desktop app..."
    mkdir -p "$INSTALL_PREFIX/bin"

    # Install binary
    install -m 755 "$TEMP_DIR/super-stt-app" "$INSTALL_PREFIX/bin/"

    print_info "Installing desktop integration files..."

    # Install desktop file
    mkdir -p "$DESKTOP_DIR"
    install -m 644 "$TEMP_DIR/resources/super-stt-app.desktop" "$DESKTOP_DIR/super-stt-app.desktop"

    # Install icon
    mkdir -p "$ICON_DIR"
    cp -r "$TEMP_DIR/resources/icons/hicolor/scalable/apps/super-stt-app.svg" "$ICON_DIR/"

    # Install metainfo
    mkdir -p "$METAINFO_DIR"
    if [ -f "$TEMP_DIR/resources/super-stt-app.metainfo.xml" ]; then
        install -m 644 "$TEMP_DIR/resources/super-stt-app.metainfo.xml" "$METAINFO_DIR/super-stt-app.metainfo.xml"
    fi

    # Update icon cache
    if command -v gtk-update-icon-cache &> /dev/null; then
        gtk-update-icon-cache -f -t "$ICON_DIR" 2>/dev/null || true
    fi
}

# Install COSMIC applet
install_applet() {
    if ! command -v cosmic-panel &> /dev/null; then
        print_warn "COSMIC panel not found - skipping applet installation"
        return 0
    fi

    print_info "Installing COSMIC applet..."
    mkdir -p "$INSTALL_PREFIX/bin"

    # Check if this is an update (binary already exists)
    local is_update=false
    if [ -f "$INSTALL_PREFIX/bin/super-stt-cosmic-applet" ]; then
        is_update=true
    fi

    # Install binary
    install -m 755 "$TEMP_DIR/super-stt-cosmic-applet" "$INSTALL_PREFIX/bin/"

    print_info "Installing COSMIC applet integration files..."

    # Install desktop files
    mkdir -p "$DESKTOP_DIR"
    for desktop_file in "$TEMP_DIR/resources/super-stt-cosmic-applet-"*.desktop; do
        local filename=$(basename "$desktop_file")
        install -m 644 "$desktop_file" "$DESKTOP_DIR/$filename"
    done

    # Install icon
    mkdir -p "$ICON_DIR"
    cp -r "$TEMP_DIR/resources/icons/hicolor/scalable/apps/super-stt-cosmic-applet.svg" "$ICON_DIR/"

    # Update icon cache
    if command -v gtk-update-icon-cache &> /dev/null; then
        gtk-update-icon-cache -f -t "$ICON_DIR" 2>/dev/null || true
    fi

    # Restart COSMIC panel so the updated applet binary is loaded
    if [ "$is_update" = true ] && pgrep -f cosmic-panel > /dev/null 2>&1; then
        print_info "Restarting COSMIC panel to load updated applet..."
        pkill -f cosmic-panel || true
    fi
}

# Install systemd service
install_service() {
    if ! command -v systemctl &> /dev/null; then
        print_warn "Systemd not detected - skipping service installation"
        return 0
    fi

    print_info "Installing systemd service..."

    mkdir -p "$USER_SYSTEMD_DIR"
    cp "$TEMP_DIR/systemd/super-stt.service" "$USER_SYSTEMD_DIR/"
    systemctl --user daemon-reload

    # Enable and start (or restart) the service
    systemctl --user enable super-stt
    if systemctl --user is-active --quiet super-stt; then
        print_info "Restarting Super STT daemon service to apply update..."
        if systemctl --user restart super-stt; then
            print_info "Super STT daemon service restarted successfully"
        else
            print_warn "Failed to restart Super STT daemon service"
            print_warn "You can restart it manually with: systemctl --user restart super-stt"
        fi
    else
        print_info "Starting Super STT daemon service..."
        if systemctl --user start super-stt; then
            print_info "Super STT daemon service started successfully"
        else
            print_warn "Failed to start Super STT daemon service"
            print_warn "You can start it manually with: systemctl --user start super-stt"
        fi
    fi
}

# Warn if the install dir isn't on PATH. We deliberately do NOT edit the
# user's shell profile: rc files are the user's to manage, and an auto-append
# has no clean uninstall counterpart. Assume `~/.local/bin` is already on PATH
# (the XDG default) and just tell the user how to add it if it isn't.
update_path() {
    local bin_dir="$1"

    if [[ ":$PATH:" != *":$bin_dir:"* ]]; then
        print_warn "$bin_dir is not on your PATH."
        print_warn "Add this line to your shell profile (e.g. ~/.bashrc or ~/.zshrc):"
        print_warn "  export PATH=\"$bin_dir:\$PATH\""
    fi
}

# Configure COSMIC keyboard shortcut
configure_cosmic_shortcut() {
    # Check if we're on COSMIC desktop
    if ! command -v cosmic-panel &> /dev/null; then
        return 0
    fi

    COSMIC_SHORTCUTS_DIR="$HOME/.config/cosmic/com.system76.CosmicSettings.Shortcuts/v1"
    COSMIC_SHORTCUTS_FILE="$COSMIC_SHORTCUTS_DIR/custom"

    # Ask user if they want to add the shortcut
    echo -n "Add COSMIC keyboard shortcut (Super+Space)? [Y/n]: "
    if [ -t 2 ]; then
        read -r add_shortcut < /dev/tty
    else
        # Non-interactive fallback - default to yes
        add_shortcut="y"
        echo "y"
    fi

    if [[ "$add_shortcut" =~ ^[Nn]$ ]]; then
        return 0
    fi

    # Create the shortcuts directory if it doesn't exist
    mkdir -p "$COSMIC_SHORTCUTS_DIR"

    # Use the full path to the stt wrapper for reliability
    local stt_command="$INSTALL_PREFIX/bin/stt record --write"

    # Check if shortcuts file exists and has content
    if [ -f "$COSMIC_SHORTCUTS_FILE" ] && [ -s "$COSMIC_SHORTCUTS_FILE" ]; then
        # File exists with content, check if our shortcut is already there
        if grep -q "Super STT" "$COSMIC_SHORTCUTS_FILE"; then
            return 0
        fi

        # Check if Super+Space is already used
        if grep -q 'key: "space"' "$COSMIC_SHORTCUTS_FILE" && grep -A5 -B5 'key: "space"' "$COSMIC_SHORTCUTS_FILE" | grep -q 'Super'; then
            return 0
        fi

        # Create a backup
        cp "$COSMIC_SHORTCUTS_FILE" "$COSMIC_SHORTCUTS_FILE.backup"

        # Create a temporary file with the new shortcut entry
        local temp_file=$(mktemp)

        # Check if the file is empty (just {}) and handle accordingly
        if grep -q '^{}$' "$COSMIC_SHORTCUTS_FILE"; then
            # File is empty, replace entirely
            echo '{' > "$temp_file"
        else
            # File has content, remove the closing brace and add our shortcut
            head -n -1 "$COSMIC_SHORTCUTS_FILE" > "$temp_file"
        fi

        cat >> "$temp_file" << EOFSHORTCUT
    (
        modifiers: [
            Super,
        ],
        key: "space",
        description: Some("Super STT"),
    ): Spawn("$stt_command"),
}
EOFSHORTCUT

        # Replace the original file
        mv "$temp_file" "$COSMIC_SHORTCUTS_FILE"

        # Verify the file is valid by checking it has proper structure
        if ! grep -q '^{' "$COSMIC_SHORTCUTS_FILE" || ! grep -q '^}' "$COSMIC_SHORTCUTS_FILE"; then
            mv "$COSMIC_SHORTCUTS_FILE.backup" "$COSMIC_SHORTCUTS_FILE"
            return 1
        fi

        # Remove backup if successful
        rm -f "$COSMIC_SHORTCUTS_FILE.backup"
    else
        # Create new shortcuts file with our shortcut
        cat > "$COSMIC_SHORTCUTS_FILE" << EOF
{
    (
        modifiers: [
            Super,
        ],
        key: "space",
        description: Some("Super STT"),
    ): Spawn("$stt_command"),
}
EOF
    fi
}

for arg in "$@"; do
    case $arg in
        --non-interactive)
            INTERACTIVE=false
            shift
            ;;
        --version=*)
            VERSION="${arg#*=}"
            shift
            ;;
        --version)
            VERSION="$2"
            shift 2
            ;;
    esac
done

# Interactive menu function
show_menu() {
    echo "============================================="
    echo "      Super STT BETA Installation Menu"
    echo "============================================="
    echo ""
    echo "Detected system:"
    echo "  Architecture: $ARCH"
    echo ""
    echo "What would you like to install?"
    echo ""
    echo "1. All $([ ! command -v cosmic-panel &> /dev/null ] && echo "(skip COSMIC applet)" || echo "(includes COSMIC applet)") [DEFAULT]"
    echo "2. Daemon + CLI only"
    echo "3. Desktop App only"
    echo "4. COSMIC Applet only $([ ! command -v cosmic-panel &> /dev/null ] && echo "(not available)")"
    echo ""
    echo "q. Quit"
    echo ""
    echo "============================================="
}

handle_menu() {
    while true; do
        show_menu
        echo -n "Select option [1-4, q] (default: 1): "
        read -r choice

        # Default to option 1 if empty
        if [ -z "$choice" ]; then
            choice="1"
        fi

        case $choice in
            1)
                INSTALL_OPTION="all"
                break
                ;;
            2)
                INSTALL_OPTION="daemon"
                break
                ;;
            3)
                INSTALL_OPTION="app"
                break
                ;;
            4)
                if ! command -v cosmic-panel &> /dev/null; then
                    print_warn "COSMIC panel not found - applet not available"
                    sleep 1
                else
                    INSTALL_OPTION="applet"
                    break
                fi
                ;;
            q|Q)
                print_info "Installation cancelled"
                exit 0
                ;;
            *)
                print_warn "Invalid option. Please try again."
                sleep 1
                ;;
        esac
    done
}

# Show interactive menu if in interactive mode
# Check if we have a controlling terminal (works better with piped input)
if [ "$INTERACTIVE" = true ] && [ -t 2 ]; then
    # Minimal detection for menu display
    ARCH=$(detect_arch)
    VARIANT="cpu"

    # Save current stdin and redirect to terminal for menu
    exec 3<&0
    exec < /dev/tty
    handle_menu
    clear
    # Restore original stdin
    exec 0<&3
    exec 3<&-
fi

# Detect what we need based on install option
ARCH=$(detect_arch)
print_info "Detected architecture: $ARCH"


# GPU residency lives in out-of-tree backends, so there is a single
# CPU build for every install option.
VARIANT="cpu"

# Get the latest pre-release (beta) version if not specified.
#
# GitHub's /releases/latest endpoint excludes prereleases — that's
# what install-stable.sh uses to pin to the most recent stable. For
# beta we walk the full /releases list and pick the newest one whose
# `prerelease` flag is true. jq isn't a dependency, so do this in
# pure grep/sed; the response shape is stable enough.
if [ "$VERSION" = "latest" ]; then
    print_info "Fetching latest beta (pre-release) version..."
    RELEASES_JSON=$(curl -s "https://api.github.com/repos/$GITHUB_REPO/releases")
    if [ -z "$RELEASES_JSON" ]; then
        print_error "Failed to fetch release list"
        exit 1
    fi

    # Walk the JSON object-by-object: each release starts with
    # `"tag_name":`, and the matching `"prerelease":` follows within
    # the same object. We emit `<tag>:<prerelease>` pairs and pick
    # the first one whose flag is true.
    VERSION=$(echo "$RELEASES_JSON" \
        | grep -E '"tag_name"|"prerelease"' \
        | paste - - \
        | awk -F'[:,]' '
            {
                # $2 = tag_name value (with quotes), $4 = prerelease bool
                gsub(/[ "]/, "", $2);
                gsub(/[ "]/, "", $4);
                if ($4 == "true") { print $2; exit }
            }')

    if [ -z "$VERSION" ]; then
        print_error "No beta (pre-release) found in the GitHub releases for $GITHUB_REPO"
        print_error "If you want the latest stable, rerun the installer without --beta."
        exit 1
    fi
fi

print_info "Installing Super STT BETA $VERSION"

# Tarball naming: stable releases use `super-stt-<arch>-<variant>.tar.gz`
# Beta releases use `super-stt-<arch>-<variant>-beta.tar.gz` so the
# two channels can coexist on the same GitHub releases page without
# either side accidentally pulling the other's artifacts.
TARBALL_NAME="super-stt-${ARCH}-${VARIANT}-beta.tar.gz"
DOWNLOAD_URL="https://github.com/$GITHUB_REPO/releases/download/$VERSION/$TARBALL_NAME"

print_info "Downloading from: $DOWNLOAD_URL"

# Download the tarball
download_with_fallback() {
    local variant="$1"
    local arch="$2"
    local tarball_name="super-stt-${arch}-${variant}-beta.tar.gz"
    local download_url="https://github.com/$GITHUB_REPO/releases/download/$VERSION/$tarball_name"

    print_info "Trying to download: $tarball_name" >&2

    if curl -L -f -o "$TEMP_DIR/$tarball_name" "$download_url" 2>/dev/null; then
        echo "$tarball_name"
        return 0
    fi

    return 1
}

# Try to download with fallback support
DOWNLOADED_TARBALL=$(download_with_fallback "$VARIANT" "$ARCH")

if [ -z "$DOWNLOADED_TARBALL" ]; then
    print_error "Failed to download the beta tarball"
    print_error "Tried: https://github.com/$GITHUB_REPO/releases/download/$VERSION/super-stt-${ARCH}-${VARIANT}-beta.tar.gz"
    exit 1
fi

print_info "Successfully downloaded: $DOWNLOADED_TARBALL"

# Extract the tarball
print_info "Extracting binaries..."
tar -xzf "$TEMP_DIR/$DOWNLOADED_TARBALL" -C "$TEMP_DIR"

# Install components based on selection
case $INSTALL_OPTION in
    "all")
        # Install everything (skip applet if COSMIC not available)
        install_daemon
        install_app
        if command -v cosmic-panel &> /dev/null; then
            install_applet
        fi
        install_service
        ;;
    "daemon")
        # Install daemon + CLI + consent helper + service
        install_daemon
        install_service
        ;;
    "app")
        # Install app only
        install_app
        ;;
    "applet")
        # Install applet only
        install_applet
        ;;
esac

# Add to PATH if installing to non-standard location
if [ "$INSTALL_PREFIX" != "/usr/local" ] && [ "$INSTALL_PREFIX" != "/usr" ]; then
    update_path "$INSTALL_PREFIX/bin"
fi

# Configure COSMIC shortcut if daemon was installed and in interactive mode
if [ "$INTERACTIVE" = true ] && ([ "$INSTALL_OPTION" = "all" ] || [ "$INSTALL_OPTION" = "daemon" ]); then
    configure_cosmic_shortcut "$INSTALL_PREFIX"
fi

print_info ""
print_info "Installation complete!"
print_info ""
print_info "Installed components:"

case $INSTALL_OPTION in
    "all")
        print_info "  ✅ super-stt-daemon"
        print_info "  ✅ super-stt-cli"
        print_info "  ✅ super-stt-consent (auth popup helper)"
        print_info "  ✅ stt (convenience wrapper)"
        [ -f "$INSTALL_PREFIX/bin/super-stt-app" ] && print_info "  ✅ super-stt-app (desktop app)"
        [ -f "$INSTALL_PREFIX/bin/super-stt-cosmic-applet" ] && print_info "  ✅ super-stt-cosmic-applet (COSMIC applet)"
        print_info "  ✅ systemd user service"
        print_info ""
        print_info "Run 'stt record --write' to get started"
        ;;
    "daemon")
        print_info "  ✅ super-stt-daemon"
        print_info "  ✅ super-stt-cli"
        print_info "  ✅ super-stt-consent (auth popup helper)"
        print_info "  ✅ stt (convenience wrapper)"
        print_info "  ✅ systemd user service"
        print_info ""
        print_info "Run 'stt record --write' to get started"
        ;;
    "app")
        [ -f "$INSTALL_PREFIX/bin/super-stt-app" ] && print_info "  ✅ super-stt-app (desktop app)"
        print_info ""
        print_info "Desktop app installed. Note: You'll need the daemon to use Super STT functionality."
        ;;
    "applet")
        [ -f "$INSTALL_PREFIX/bin/super-stt-cosmic-applet" ] && print_info "  ✅ super-stt-cosmic-applet (COSMIC applet)"
        print_info ""
        print_info "COSMIC applet installed. Note: You'll need the daemon to use Super STT functionality."
        ;;
esac
