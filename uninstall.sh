#!/bin/bash

# Super Voice Uninstall Script
#
# Removes Super Voice, and whatever Super STT or Super TTS installs it
# replaced left behind, regardless of which channel installed them.
#
# Usage:
#   curl -sSL https://raw.githubusercontent.com/jorge-menjivar/super-voice/main/uninstall.sh | bash
#   bash uninstall.sh
#
# What gets removed:
#   - Super Voice's binaries in /usr/local/bin: the daemon, the `voice` CLI
#     and wrapper, the consent helper, the installer, both settings apps and
#     the COSMIC applet
#   - What Super STT and Super TTS installed: their daemons, CLIs, wrappers
#     (`stt`, `tts`), consent helpers, installers and applets, in
#     /usr/local/bin and the legacy ~/.local/bin
#   - Desktop entries, icons, metainfo (system + per-user)
#   - Runtime socket dirs under $XDG_RUNTIME_DIR
#   - systemd user units (/usr/lib/systemd/user + ~/.config/systemd/user)
#   - The COSMIC keyboard shortcut (only if it's the lone entry)
#
# The COSMIC panel is restarted when an installed applet was removed, so the
# tray drops it without a relogin.
#
# Root-owned files are removed via sudo; it is only invoked when such
# files are actually present.
#
# What is PRESERVED:
#   - Logs
#   - Config: ~/.config/super-voice/
#   - Secrets: ~/.local/state/super-voice/ (session tokens, API keys)
#   - Models, installed backends and cloned voices: ~/.local/share/super-voice/
#   - Whatever Super STT and Super TTS kept in their own config, state and
#     data folders
#
# The daemons are stopped as the final step so any in-flight transcription
# or speech gets a chance to finish before the process exits.

set -u

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'
print_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
print_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Current (system) install locations.
SYSTEM_PREFIX="${INSTALL_PREFIX:-/usr/local}"
SYSTEM_BIN_DIR="$SYSTEM_PREFIX/bin"
SYSTEM_DESKTOP_DIR="$SYSTEM_PREFIX/share/applications"
SYSTEM_ICON_DIR="$SYSTEM_PREFIX/share/icons/hicolor/scalable/apps"
SYSTEM_ICON_THEME_DIR="$SYSTEM_PREFIX/share/icons/hicolor"
SYSTEM_METAINFO_DIR="$SYSTEM_PREFIX/share/metainfo"
SYSTEM_SYSTEMD_DIR="/usr/lib/systemd/user"

# Legacy per-user install locations.
LEGACY_BIN_DIR="$HOME/.local/bin"
DESKTOP_DIR="$HOME/.local/share/applications"
ICON_DIR_HICOLOR="$HOME/.local/share/icons/hicolor/scalable/apps"
ICON_DIR_FLAT="$HOME/.local/share/icons"
METAINFO_DIR="$HOME/.local/share/metainfo"
USER_SYSTEMD_DIR="$HOME/.config/systemd/user"

RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
CONFIG_DIR="$HOME/.config/super-voice"
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/super-voice"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/super-voice"
COSMIC_SHORTCUTS="$HOME/.config/cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom"

# Super Voice's unit, and the ones it replaced.
SERVICES=(super-voice super-stt super-tts)

# Every binary any of them installed, current names first.
BINARIES=(
    super-voice-daemon super-voice-cli super-voice-consent super-voice-install voice
    super-voice-app super-tts-app super-stt-app
    super-cosmic-applet
    super-stt super-stt-daemon super-stt-cli super-stt-consent super-stt-install stt
    super-stt-cosmic-applet super-stt-applet-full super-stt-applet-left super-stt-applet-right
    super-tts-daemon super-tts-cli super-tts-consent super-tts-install tts
    super-tts-cosmic-applet
)
APPS=(super-voice-app super-tts-app super-stt-app)
APPLETS=(super-cosmic-applet super-stt-cosmic-applet super-tts-cosmic-applet)
# Processes to stop at the end, for a daemon started outside systemd.
DAEMONS=(super-voice-daemon super-stt-daemon super-tts-daemon super-stt)

# This script undoes what install.sh did, and install.sh only installs on
# Linux (see its `detect_triple`). Running it on a Mac would walk a tree of
# paths that do not exist there and shell out to a `systemctl` that is not
# there either — a wall of harmless errors that reads like a broken
# uninstall. Say what to run instead.
if [ "$(uname -s)" = "Darwin" ]; then
    print_error "This uninstaller is for Linux installs made by install.sh."
    print_error "On macOS Super Voice is installed from source, so uninstall it the same way:"
    print_error "  cd /path/to/super-stt && just uninstall"
    exit 1
fi

SUDO=""
if [ "$(id -u)" -ne 0 ]; then
    SUDO="sudo"
fi

# Remove a path that may be root-owned. Tries a plain rm first so sudo
# is only invoked when actually needed. Returns 1 if the path did not
# exist (so callers can skip their "removed" message).
remove_path() {
    local target="$1"
    if [ ! -e "$target" ] && [ ! -L "$target" ]; then
        return 1
    fi
    rm -f "$target" 2>/dev/null || $SUDO rm -f "$target"
}

DESKTOPS=()
ICONS=()
for app in "${APPS[@]}"; do
    DESKTOPS+=("$app.desktop")
    ICONS+=("$app.svg")
done
for applet in "${APPLETS[@]}"; do
    for side in full left right; do
        DESKTOPS+=("$applet-$side.desktop")
    done
    ICONS+=("$applet.svg")
done

# Detect whether a system install is present at all, so legacy-only
# setups never see a sudo prompt.
SYSTEM_INSTALL_PRESENT=false
for bin in "${BINARIES[@]}"; do
    [ -e "$SYSTEM_BIN_DIR/$bin" ] && SYSTEM_INSTALL_PRESENT=true
done
for service in "${SERVICES[@]}"; do
    [ -e "$SYSTEM_SYSTEMD_DIR/$service.service" ] && SYSTEM_INSTALL_PRESENT=true
done

# Whether a COSMIC applet was installed, captured here for the same reason:
# it decides whether the panel needs restarting once the files are gone
# (step 6), and by then there is nothing left to probe. Both the system and
# the legacy per-user layouts count, and the desktop entries are probed as
# well as the binary so an install missing one of the two still registers.
APPLET_INSTALLED=false
for applet in "${APPLETS[@]}"; do
    for probe in \
        "$SYSTEM_BIN_DIR/$applet" \
        "$LEGACY_BIN_DIR/$applet" \
        "$SYSTEM_DESKTOP_DIR/$applet-full.desktop" \
        "$DESKTOP_DIR/$applet-full.desktop"
    do
        [ -e "$probe" ] && APPLET_INSTALLED=true
    done
done

print_info "Uninstalling Super Voice..."

# 1. Disable the units so they don't auto-start after the next reboot,
#    BUT don't stop them yet — the daemon keeps running while its on-disk
#    artifacts are cleared, and stops at the end.
if command -v systemctl &> /dev/null; then
    for service in "${SERVICES[@]}"; do
        if systemctl --user is-enabled --quiet "$service" 2>/dev/null; then
            print_info "Disabling user systemd unit '$service'..."
            systemctl --user disable "$service" 2>/dev/null || true
        fi
    done
fi

# 2. Remove binaries — system layout plus the legacy layout. Missing
#    files are not an error (the user may have only installed a subset).
print_info "Removing binaries from $SYSTEM_BIN_DIR and $LEGACY_BIN_DIR..."
for bin in "${BINARIES[@]}"; do
    for dir in "$SYSTEM_BIN_DIR" "$LEGACY_BIN_DIR"; do
        remove_path "$dir/$bin" && print_info "  removed $dir/$bin"
    done
done

# 3. Desktop entries (system + legacy per-user).
print_info "Removing desktop entries..."
for name in "${DESKTOPS[@]}"; do
    for dir in "$SYSTEM_DESKTOP_DIR" "$DESKTOP_DIR"; do
        remove_path "$dir/$name" && print_info "  removed $dir/$name"
    done
done

# 4. Icons (system hicolor, legacy hicolor-scalable, and the flat
#    layout some old versions of the script used).
print_info "Removing icons..."
for name in "${ICONS[@]}"; do
    for dir in "$SYSTEM_ICON_DIR" "$ICON_DIR_HICOLOR" "$ICON_DIR_FLAT"; do
        remove_path "$dir/$name" && print_info "  removed $dir/$name"
    done
done

# 5. metainfo (system + legacy per-user)
for app in "${APPS[@]}"; do
    for dir in "$SYSTEM_METAINFO_DIR" "$METAINFO_DIR"; do
        remove_path "$dir/$app.metainfo.xml" && print_info "Removed $dir/$app.metainfo.xml"
    done
done

# 6. Refresh icon / desktop caches so the system reflects the removal.
if command -v gtk-update-icon-cache &> /dev/null; then
    gtk-update-icon-cache -f "$ICON_DIR_FLAT/hicolor" 2>/dev/null || true
    if [ "$SYSTEM_INSTALL_PRESENT" = true ]; then
        $SUDO gtk-update-icon-cache -f "$SYSTEM_ICON_THEME_DIR" 2>/dev/null || true
    fi
fi
if command -v update-desktop-database &> /dev/null; then
    update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
    if [ "$SYSTEM_INSTALL_PRESENT" = true ]; then
        $SUDO update-desktop-database "$SYSTEM_DESKTOP_DIR" 2>/dev/null || true
    fi
fi
# The panel keeps a loaded applet in the tray until it restarts, so a removed
# applet lingers there until the next relogin. Restart it when an applet was
# actually installed (captured before removal, above) and a panel is actually
# running; cosmic-session respawns it. Unanchored `-f`, matching the
# installer's own post-install panel restart, because the panel runs from an
# absolute path rather than a bare name like the launchers below.
if [ "$APPLET_INSTALLED" = true ] && pgrep -f cosmic-panel > /dev/null 2>&1; then
    print_info "Restarting cosmic-panel to drop the removed applet..."
    pkill -f cosmic-panel 2>/dev/null || true
fi

# Nudge COSMIC's launcher caches so the removed entries disappear without
# a relogin; both processes respawn on demand and rescan.
pkill -f '^cosmic-app-library$' 2>/dev/null || true
pkill -f '^cosmic-launcher$' 2>/dev/null || true
pkill -f '^pop-launcher( |$)' 2>/dev/null || true

# 7. COSMIC custom keyboard shortcut. Remove only if ours is the only
#    entry; otherwise the user has other custom bindings we shouldn't
#    disturb. They can hand-edit if they want a finer surgical removal.
if [ -f "$COSMIC_SHORTCUTS" ] && grep -Eq 'description: Some\("Super (Voice|STT)"\)' "$COSMIC_SHORTCUTS"; then
    # Each binding starts with `    (` in column 0. Count them.
    entry_count=$(grep -c '^    (' "$COSMIC_SHORTCUTS" || echo 0)
    if [ "$entry_count" = "1" ]; then
        rm -f "$COSMIC_SHORTCUTS"
        print_info "Removed COSMIC keyboard shortcut"
    else
        print_warn "COSMIC custom shortcuts file has other entries — not touching."
        print_warn "  Edit by hand to remove the Super Voice entry:"
        print_warn "  $COSMIC_SHORTCUTS"
    fi
fi

# 8. Runtime socket dirs (sockets will be cleared when the daemons stop in
#    step 10).
for name in voice stt tts; do
    if [ -d "$RUNTIME_DIR/$name" ]; then
        rm -rf "${RUNTIME_DIR:?}/$name"
        print_info "Removed runtime dir $RUNTIME_DIR/$name"
    fi
done

# 9. systemd unit files (system + legacy per-user) + reload so systemd
#    forgets the units.
for service in "${SERVICES[@]}"; do
    for unit in \
        "$SYSTEM_SYSTEMD_DIR/$service.service" \
        "$USER_SYSTEMD_DIR/$service.service"
    do
        remove_path "$unit" && print_info "Removed systemd unit $unit"
    done
done
if command -v systemctl &> /dev/null; then
    systemctl --user daemon-reload 2>/dev/null || true
fi

# 10. Stop the daemons LAST. Doing it here means an in-flight transcription
#     or utterance has had until this point to either complete or be
#     force-killed. We use `stop` (clean shutdown) and fall back to `kill`
#     if the process is still alive.
if command -v systemctl &> /dev/null; then
    for service in "${SERVICES[@]}"; do
        if systemctl --user is-active --quiet "$service" 2>/dev/null; then
            print_info "Stopping $service..."
            systemctl --user stop "$service" 2>/dev/null || true
        fi
    done
fi
# Catch a daemon started outside of systemd (e.g. `just run-daemon`).
for proc in "${DAEMONS[@]}"; do
    if pgrep -u "$(id -u)" -x "$proc" > /dev/null 2>&1; then
        print_info "Killing leftover $proc process..."
        pkill -u "$(id -u)" -x "$proc" 2>/dev/null || true
    fi
done

print_info ""
print_info "Uninstall complete."
print_info ""
print_info "Preserved (delete manually if you want a deeper clean):"
print_info "  - Config:  $CONFIG_DIR"
print_info "  - Secrets: $STATE_DIR (session tokens and API keys)"
print_info "  - Data:    $DATA_DIR (installed backends, models, cloned voices)"
