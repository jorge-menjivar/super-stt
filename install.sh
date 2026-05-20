#!/bin/bash

# Super STT Installation Dispatcher
#
# This script is the documented entry point:
#
#   curl -sSL https://raw.githubusercontent.com/jorge-menjivar/super-stt/main/install.sh | bash
#   curl -sSL https://raw.githubusercontent.com/jorge-menjivar/super-stt/main/install.sh | bash -s -- --beta
#
# It is intentionally tiny — all real work lives in
# scripts/install-stable.sh (legacy single-`super-stt` releases) and
# scripts/install-beta.sh (post-protocol-rewrite workspace with
# separate daemon / CLI / consent-helper binaries).
#
# Why a thin dispatcher: the stable channel still serves users who
# installed before the workspace split, and we don't want a tarball-
# layout change to break their `install.sh` URL. The new layout
# (consent helper, no `sg stt -c`, etc.) ships under `--beta` until
# it's promoted.
#
# Flags consumed here:
#   --beta            Use the beta installer (pre-release artifacts)
#   --channel=<name>  Force `stable` or `beta` (overrides --beta)
#
# Everything else is passed through to the chosen sub-installer.

set -e

GITHUB_REPO="jorge-menjivar/super-stt"
DEFAULT_BRANCH="main"
CHANNEL="stable"
PASSTHROUGH=()

# Color helpers (mirror the sub-scripts so the dispatcher's lines
# don't look out-of-place when piped through).
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'
print_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
print_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error() { echo -e "${RED}[ERROR]${NC} $1"; }

for arg in "$@"; do
    case "$arg" in
        --beta)
            CHANNEL="beta"
            ;;
        --stable)
            CHANNEL="stable"
            ;;
        --channel=*)
            CHANNEL="${arg#--channel=}"
            ;;
        *)
            PASSTHROUGH+=("$arg")
            ;;
    esac
done

case "$CHANNEL" in
    stable|beta) ;;
    *)
        print_error "Unknown channel '$CHANNEL' — expected 'stable' or 'beta'."
        exit 1
        ;;
esac

# When invoked from a clone (e.g. `bash install.sh --beta`) we have
# the sub-scripts on disk and should use them directly. When invoked
# via curl-to-bash the path doesn't exist, so we re-fetch over HTTP.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd || true)"
LOCAL_SCRIPT=""
if [ -n "$SCRIPT_DIR" ] && [ -f "$SCRIPT_DIR/scripts/install-${CHANNEL}.sh" ]; then
    LOCAL_SCRIPT="$SCRIPT_DIR/scripts/install-${CHANNEL}.sh"
fi

if [ -n "$LOCAL_SCRIPT" ]; then
    print_info "Dispatching to local installer: $LOCAL_SCRIPT"
    exec bash "$LOCAL_SCRIPT" "${PASSTHROUGH[@]}"
fi

REMOTE_SCRIPT="https://raw.githubusercontent.com/${GITHUB_REPO}/${DEFAULT_BRANCH}/scripts/install-${CHANNEL}.sh"
print_info "Dispatching to remote ${CHANNEL} installer: $REMOTE_SCRIPT"

# We need a controlling tty for the interactive menu in the
# sub-script. Pulling the script body into a variable first lets us
# bypass the `bash -c` stdin issue when this dispatcher was itself
# piped from curl.
SCRIPT_BODY=$(curl -fsSL "$REMOTE_SCRIPT")
if [ -z "$SCRIPT_BODY" ]; then
    print_error "Failed to fetch installer from $REMOTE_SCRIPT"
    exit 1
fi

# Hand off to the channel-specific installer with the residual args.
# Using a process substitution preserves stdin so the sub-script's
# interactive prompts still see /dev/tty correctly.
exec bash <(echo "$SCRIPT_BODY") "${PASSTHROUGH[@]}"
