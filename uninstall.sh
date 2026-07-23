#!/bin/bash

# Super STT Uninstall Script
#
# Removes Super STT regardless of which channel installed it.
# Handles both layouts:
#   - Stable / legacy: single `super-stt` binary + `stt` wrapper.
#   - Beta / post-rewrite: super-stt-daemon + super-stt-cli +
#     super-stt-consent + `stt` wrapper.
#
# Usage:
#   curl -sSL https://raw.githubusercontent.com/jorge-menjivar/super-stt/main/uninstall.sh | bash
#   bash uninstall.sh
#
# What gets removed:
#   - All Super STT binaries in /usr/local/bin and ~/.local/bin
#     (current system layout plus both legacy per-user layouts)
#   - Desktop entries, icons, metainfo (system + per-user)
#   - Runtime socket dir under $XDG_RUNTIME_DIR/stt
#   - systemd user unit (/usr/lib/systemd/user + ~/.config/systemd/user)
#   - COSMIC keyboard shortcut (only if it's the lone entry)
#
# Root-owned files are removed via sudo; it is only invoked when such
# files are actually present.
#
# What is PRESERVED:
#   - ~/.local/share/stt/logs/ (in case you need to inspect history)
#   - ~/.config/super-stt/ (user-set defaults)
#   - System keyring entries (cached session tokens, API keys)
#   - The `stt` system group (other users may depend on it)
#
# The daemon is stopped as the final step so any in-flight
# transcription completes (or at least gets a chance to flush) before
# the process exits.

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

RUN_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/stt"
LOG_DIR="$HOME/.local/share/stt/logs"
CONFIG_DIR="$HOME/.config/super-stt"
COSMIC_SHORTCUTS="$HOME/.config/cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom"
SERVICE_NAME="super-stt"

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

# Detect whether a system install is present at all, so legacy-only
# setups never see a sudo prompt.
SYSTEM_INSTALL_PRESENT=false
for probe in \
    "$SYSTEM_BIN_DIR/super-stt-daemon" \
    "$SYSTEM_BIN_DIR/super-stt-app" \
    "$SYSTEM_BIN_DIR/super-stt-cosmic-applet" \
    "$SYSTEM_BIN_DIR/stt" \
    "$SYSTEM_SYSTEMD_DIR/$SERVICE_NAME.service"
do
    [ -e "$probe" ] && SYSTEM_INSTALL_PRESENT=true
done

print_info "Uninstalling Super STT..."

# 1. Disable the unit so it doesn't auto-start after the next reboot,
#    BUT don't stop it yet — we want the daemon to remain running
#    while we clear out its on-disk artifacts, and stop it at the end.
if command -v systemctl &> /dev/null; then
    if systemctl --user is-enabled --quiet "$SERVICE_NAME" 2>/dev/null; then
        print_info "Disabling user systemd unit '$SERVICE_NAME'..."
        systemctl --user disable "$SERVICE_NAME" 2>/dev/null || true
    fi
fi

# 2. Remove binaries — system layout plus both legacy layouts. Missing
#    files are not an error (the user may have only installed a subset).
print_info "Removing binaries from $SYSTEM_BIN_DIR and $LEGACY_BIN_DIR..."
for bin in \
    super-stt \
    super-stt-daemon \
    super-stt-cli \
    super-stt-consent \
    super-stt-app \
    super-stt-cosmic-applet \
    super-stt-applet-full \
    super-stt-applet-left \
    super-stt-applet-right \
    stt
do
    for dir in "$SYSTEM_BIN_DIR" "$LEGACY_BIN_DIR"; do
        remove_path "$dir/$bin" && print_info "  removed $dir/$bin"
    done
done

# 3. Desktop entries (system + legacy per-user).
print_info "Removing desktop entries..."
for name in \
    super-stt-app.desktop \
    super-stt-cosmic-applet-full.desktop \
    super-stt-cosmic-applet-left.desktop \
    super-stt-cosmic-applet-right.desktop
do
    for dir in "$SYSTEM_DESKTOP_DIR" "$DESKTOP_DIR"; do
        remove_path "$dir/$name" && print_info "  removed $dir/$name"
    done
done

# 4. Icons (system hicolor, legacy hicolor-scalable, and the flat
#    layout some old versions of the script used).
print_info "Removing icons..."
for name in super-stt-app.svg super-stt-cosmic-applet.svg; do
    for dir in "$SYSTEM_ICON_DIR" "$ICON_DIR_HICOLOR" "$ICON_DIR_FLAT"; do
        remove_path "$dir/$name" && print_info "  removed $dir/$name"
    done
done

# 5. metainfo (system + legacy per-user)
for dir in "$SYSTEM_METAINFO_DIR" "$METAINFO_DIR"; do
    remove_path "$dir/super-stt-app.metainfo.xml" && print_info "Removed $dir/super-stt-app.metainfo.xml"
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
# Nudge COSMIC's launcher caches so the removed entries disappear without
# a relogin; both processes respawn on demand and rescan.
pkill -f '^cosmic-app-library$' 2>/dev/null || true
pkill -f '^cosmic-launcher$' 2>/dev/null || true
pkill -f '^pop-launcher( |$)' 2>/dev/null || true

# 7. COSMIC custom keyboard shortcut. Remove only if Super STT is the
#    only entry; otherwise the user has other custom bindings we
#    shouldn't disturb. They can hand-edit if they want a finer
#    surgical removal.
if [ -f "$COSMIC_SHORTCUTS" ] && grep -q 'description: Some("Super STT")' "$COSMIC_SHORTCUTS"; then
    # Each binding starts with `    (` in column 0. Count them.
    entry_count=$(grep -c '^    (' "$COSMIC_SHORTCUTS" || echo 0)
    if [ "$entry_count" = "1" ]; then
        rm -f "$COSMIC_SHORTCUTS"
        print_info "Removed COSMIC keyboard shortcut"
    else
        print_warn "COSMIC custom shortcuts file has other entries — not touching."
        print_warn "  Edit by hand to remove the Super STT entry:"
        print_warn "  $COSMIC_SHORTCUTS"
    fi
fi

# 8. Runtime socket / pid dir (sockets will be cleared when the daemon
#    stops in step 10).
if [ -d "$RUN_DIR" ]; then
    rm -rf "$RUN_DIR"
    print_info "Removed runtime dir $RUN_DIR"
fi

# 9. systemd unit file (system + legacy per-user) + reload so systemd
#    forgets the unit.
for unit in \
    "$SYSTEM_SYSTEMD_DIR/$SERVICE_NAME.service" \
    "$USER_SYSTEMD_DIR/$SERVICE_NAME.service"
do
    remove_path "$unit" && print_info "Removed systemd unit $unit"
done
if command -v systemctl &> /dev/null; then
    systemctl --user daemon-reload 2>/dev/null || true
fi

# 10. Stop the daemon LAST. Doing it here means an in-flight
#     transcription has had until this point to either complete or
#     be force-killed. We use `stop` (clean shutdown) and fall back
#     to `kill` if the process is still alive after a grace window.
if command -v systemctl &> /dev/null; then
    if systemctl --user is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
        print_info "Stopping daemon..."
        systemctl --user stop "$SERVICE_NAME" 2>/dev/null || true
    fi
fi
# Catch a daemon started outside of systemd (e.g. `just run-daemon`).
for proc in super-stt-daemon super-stt; do
    if pgrep -u "$(id -u)" -x "$proc" > /dev/null 2>&1; then
        print_info "Killing leftover $proc process..."
        pkill -u "$(id -u)" -x "$proc" 2>/dev/null || true
    fi
done

print_info ""
print_info "Uninstall complete."
print_info ""
print_info "Preserved (delete manually if you want a deeper clean):"
print_info "  - Logs:    $LOG_DIR"
print_info "  - Config:  $CONFIG_DIR"
print_info "  - Keyring entries (open your keyring manager — Seahorse, KWalletManager — and delete entries under 'super-stt-session' and 'super-stt')"
