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
#   - All Super STT binaries in ~/.local/bin (both layouts)
#   - Desktop entries, icons, metainfo
#   - Runtime socket dir under $XDG_RUNTIME_DIR/stt
#   - systemd user unit
#   - COSMIC keyboard shortcut (only if it's the lone entry)
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

INSTALL_PREFIX="${INSTALL_PREFIX:-$HOME/.local}"
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

# 2. Remove binaries — both layouts. Missing files are not an error
#    (the user may have only installed a subset).
print_info "Removing binaries from $INSTALL_PREFIX/bin..."
for bin in \
    super-stt \
    super-stt-daemon \
    super-stt-cli \
    super-stt-consent \
    super-stt-app \
    super-stt-cosmic-applet \
    stt
do
    if [ -e "$INSTALL_PREFIX/bin/$bin" ]; then
        rm -f "$INSTALL_PREFIX/bin/$bin"
        print_info "  removed $INSTALL_PREFIX/bin/$bin"
    fi
done

# 3. Desktop entries.
print_info "Removing desktop entries..."
for desktop in \
    "$DESKTOP_DIR/super-stt-app.desktop" \
    "$DESKTOP_DIR/super-stt-cosmic-applet-full.desktop" \
    "$DESKTOP_DIR/super-stt-cosmic-applet-left.desktop" \
    "$DESKTOP_DIR/super-stt-cosmic-applet-right.desktop"
do
    [ -f "$desktop" ] && rm -f "$desktop" && print_info "  removed $desktop"
done

# 4. Icons (try both hicolor-scalable and the flat layout some old
#    versions of the script used).
print_info "Removing icons..."
for icon in \
    "$ICON_DIR_HICOLOR/super-stt-app.svg" \
    "$ICON_DIR_HICOLOR/super-stt-cosmic-applet.svg" \
    "$ICON_DIR_FLAT/super-stt-app.svg" \
    "$ICON_DIR_FLAT/super-stt-cosmic-applet.svg"
do
    [ -f "$icon" ] && rm -f "$icon" && print_info "  removed $icon"
done

# 5. metainfo
if [ -f "$METAINFO_DIR/super-stt-app.metainfo.xml" ]; then
    rm -f "$METAINFO_DIR/super-stt-app.metainfo.xml"
    print_info "Removed metainfo file"
fi

# 6. Refresh icon / desktop caches so the system reflects the removal.
if command -v gtk-update-icon-cache &> /dev/null; then
    gtk-update-icon-cache -f "$ICON_DIR_FLAT/hicolor" 2>/dev/null || true
fi
if command -v update-desktop-database &> /dev/null; then
    update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
fi

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

# 9. systemd unit file + reload so systemd forgets the unit.
if [ -f "$USER_SYSTEMD_DIR/$SERVICE_NAME.service" ]; then
    rm -f "$USER_SYSTEMD_DIR/$SERVICE_NAME.service"
    print_info "Removed systemd unit file"
fi
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
