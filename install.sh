#!/bin/bash

# Super STT Installation Bootstrap
#
# Documented entry point:
#
#   curl -sSL https://raw.githubusercontent.com/jorge-menjivar/super-stt/main/install.sh | bash
#   curl -sSL https://raw.githubusercontent.com/jorge-menjivar/super-stt/main/install.sh | bash -s -- --beta
#
# This script is deliberately tiny: it detects the architecture, resolves
# the requested release, downloads the `super-stt-install` binary attached
# to that release, and execs it. All installer logic lives in that binary
# (rustup-style), so the bootstrap can never disagree with the release
# layout it installs.
#
# Releases that predate the installer binary fall back to the legacy
# channel scripts (scripts/install-stable.sh / scripts/install-beta.sh).
#
# Flags consumed here:
#   --beta / --stable / --channel=<name>   Pick the release channel
#   --version=<tag>                        Pin a release tag
# Everything else is passed through to the installer.

set -e

GITHUB_REPO="jorge-menjivar/super-stt"
DEFAULT_BRANCH="main"
CHANNEL="stable"
VERSION=""
PASSTHROUGH=()

GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'
print_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
print_error() { echo -e "${RED}[ERROR]${NC} $1"; }

for arg in "$@"; do
    case "$arg" in
        --beta) CHANNEL="beta" ;;
        --stable) CHANNEL="stable" ;;
        --channel=*) CHANNEL="${arg#--channel=}" ;;
        --version=*) VERSION="${arg#--version=}" ;;
        *) PASSTHROUGH+=("$arg") ;;
    esac
done

case "$CHANNEL" in
    stable|beta) ;;
    *) print_error "Unknown channel '$CHANNEL' — expected 'stable' or 'beta'."; exit 1 ;;
esac

case "$(uname -m)" in
    x86_64) TRIPLE="x86_64-unknown-linux-gnu" ;;
    aarch64|arm64) TRIPLE="aarch64-unknown-linux-gnu" ;;
    *) print_error "Unsupported architecture: $(uname -m)"; exit 1 ;;
esac

# Resolve the target release tag for the channel (unless pinned).
if [ -z "$VERSION" ]; then
    if [ "$CHANNEL" = "stable" ]; then
        VERSION=$(curl -fsSL "https://api.github.com/repos/$GITHUB_REPO/releases/latest" \
            | grep '"tag_name"' | head -1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')
    else
        # Newest prerelease: walk tag_name/prerelease pairs, first true wins.
        VERSION=$(curl -fsSL "https://api.github.com/repos/$GITHUB_REPO/releases" \
            | grep -E '"tag_name"|"prerelease"' \
            | paste - - \
            | awk -F'[:,]' '{ gsub(/[ "]/,"",$2); gsub(/[ "]/,"",$4); if ($4=="true") { print $2; exit } }')
    fi
fi
if [ -z "$VERSION" ]; then
    print_error "Could not resolve a $CHANNEL release for $GITHUB_REPO"
    exit 1
fi

INSTALLER_ASSET="super-stt-install-$TRIPLE"
INSTALLER_URL="https://github.com/$GITHUB_REPO/releases/download/$VERSION/$INSTALLER_ASSET"

TEMP_DIR=$(mktemp -d)
trap 'rm -rf "$TEMP_DIR"' EXIT

if curl -fsSL -o "$TEMP_DIR/$INSTALLER_ASSET" "$INSTALLER_URL" 2>/dev/null; then
    chmod +x "$TEMP_DIR/$INSTALLER_ASSET"
    print_info "Launching installer for $VERSION ($TRIPLE)"
    EXTRA_ARGS=(--version="$VERSION")
    [ "$CHANNEL" = "beta" ] && EXTRA_ARGS+=(--beta)
    exec "$TEMP_DIR/$INSTALLER_ASSET" "${EXTRA_ARGS[@]}" "${PASSTHROUGH[@]}"
fi

[ -n "$VERSION" ] && PASSTHROUGH+=("--version=$VERSION")

# ---- Legacy fallback: release has no installer binary ----
print_info "Release $VERSION has no installer binary; using the legacy $CHANNEL script."

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd || true)"
if [ -n "$SCRIPT_DIR" ] && [ -f "$SCRIPT_DIR/scripts/install-${CHANNEL}.sh" ]; then
    exec bash "$SCRIPT_DIR/scripts/install-${CHANNEL}.sh" "${PASSTHROUGH[@]}"
fi

REMOTE_SCRIPT="https://raw.githubusercontent.com/${GITHUB_REPO}/${DEFAULT_BRANCH}/scripts/install-${CHANNEL}.sh"
SCRIPT_BODY=$(curl -fsSL "$REMOTE_SCRIPT")
if [ -z "$SCRIPT_BODY" ]; then
    print_error "Failed to fetch installer from $REMOTE_SCRIPT"
    exit 1
fi
# Process substitution keeps /dev/tty available for the legacy script's menu.
exec bash <(echo "$SCRIPT_BODY") "${PASSTHROUGH[@]}"
