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
#
# The pure logic below (arch detection, channel validation, tag resolution
# from a JSON string) is factored into functions so `scripts/test-install.sh`
# can source this file and exercise them directly against fixture JSON,
# without making a network call or running a real install. See the guard at
# the bottom of the file for how that source-only mode is triggered.

GITHUB_REPO="jorge-menjivar/super-stt"
DEFAULT_BRANCH="main"

GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'
print_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
print_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# ---- Pure helpers (network-free, fixture-testable) -----------------------

# $1: `uname -m` output. Echoes the matching Rust target triple on stdout;
# returns non-zero with nothing printed for an unrecognized machine type
# (the caller decides how to report that).
detect_triple() {
    case "$1" in
        x86_64) echo "x86_64-unknown-linux-gnu" ;;
        aarch64 | arm64) echo "aarch64-unknown-linux-gnu" ;;
        *) return 1 ;;
    esac
}

# $1: candidate channel name. Succeeds only for the two supported channels.
validate_channel() {
    case "$1" in
        stable | beta) return 0 ;;
        *) return 1 ;;
    esac
}

# $1: JSON body of `GET /repos/<repo>/releases/latest` (a single release
# object). Echoes its `tag_name`, or empty if the field is missing/blank.
# The `grep`+`sed` scrape a single object has no ordering hazard (unlike the
# beta list below), so this is a plain string scrape either way — split out
# only so `scripts/test-install.sh` can exercise the exact fallback path
# directly, independent of whether `jq` happens to be installed on the test
# host.
_resolve_stable_tag_grep() {
    local json="$1"
    printf '%s' "$json" | grep '"tag_name"' | head -1 \
        | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/'
}

# Prefers `jq`; falls back to [`_resolve_stable_tag_grep`] when `jq` isn't
# installed.
resolve_stable_tag() {
    local json="$1"
    if command -v jq >/dev/null 2>&1; then
        printf '%s' "$json" | jq -r '.tag_name // empty'
    else
        _resolve_stable_tag_grep "$json"
    fi
}

# $1: JSON body of `GET /repos/<repo>/releases` (an array, newest first).
# Echoes the `tag_name` of the newest release with `"prerelease": true`, or
# empty if there isn't one.
#
# This fallback (used when `jq` isn't installed) greps every
# `"tag_name"`/`"prerelease"` line out of the whole document and pairs them
# up positionally — it silently assumes `tag_name` always immediately
# precedes `prerelease` in each object (true for GitHub's current field
# order, but not a contract) AND that neither literal string ever appears
# inside a release's `body` text (a changelog that quotes either desyncs
# every later pair). Both hazards are real and NOT closed here — `jq` is
# what actually fixes them (see [`resolve_beta_tag`]'s exact reasoning-over-
# parsed-objects path below); `scripts/test-install.sh` pins the
# field-order-swapped case as a documented known limitation of this
# fallback specifically, run unconditionally (not gated on `jq`'s absence)
# so that weak path always has real coverage.
_resolve_beta_tag_grep() {
    local json="$1"
    printf '%s' "$json" \
        | grep -E '"tag_name"|"prerelease"' \
        | paste - - \
        | awk -F'[:,]' '{ gsub(/[ "]/,"",$2); gsub(/[ "]/,"",$4); if ($4=="true") { print $2; exit } }'
}

# The `jq` path is exact: it reasons over parsed release objects, so field
# order within an object and any text in a release's markdown `body` are
# irrelevant. Falls back to [`_resolve_beta_tag_grep`] when `jq` isn't
# installed — see that function's doc comment for the hazards it doesn't
# close.
resolve_beta_tag() {
    local json="$1"
    if command -v jq >/dev/null 2>&1; then
        printf '%s' "$json" \
            | jq -r '[.[] | select(.prerelease==true)][0].tag_name // empty'
    else
        _resolve_beta_tag_grep "$json"
    fi
}

# ---- Main install flow -----------------------------------------------------

main() {
    set -e

    CHANNEL="stable"
    VERSION=""
    PASSTHROUGH=()

    for arg in "$@"; do
        case "$arg" in
            --beta) CHANNEL="beta" ;;
            --stable) CHANNEL="stable" ;;
            --channel=*) CHANNEL="${arg#--channel=}" ;;
            --version=*) VERSION="${arg#--version=}" ;;
            *) PASSTHROUGH+=("$arg") ;;
        esac
    done

    if ! validate_channel "$CHANNEL"; then
        print_error "Unknown channel '$CHANNEL' — expected 'stable' or 'beta'."
        exit 1
    fi

    if ! TRIPLE=$(detect_triple "$(uname -m)"); then
        print_error "Unsupported architecture: $(uname -m)"
        exit 1
    fi

    # Resolve the target release tag for the channel (unless pinned). A
    # failed fetch (network down, rate-limited, etc.) is swallowed here on
    # purpose: `RELEASE_JSON` ends up empty, `resolve_*_tag` echoes nothing
    # for it, and the blanket "Could not resolve a $CHANNEL release" check
    # below reports it — same as before this script grew a resolver
    # function. The installer-asset download further down draws that same
    # distinction more precisely (see the `HTTP_CODE` handling there), since
    # that step is where a wrongly-generic message previously misled users.
    if [ -z "$VERSION" ]; then
        if [ "$CHANNEL" = "stable" ]; then
            RELEASE_JSON=$(curl -fsSL "https://api.github.com/repos/$GITHUB_REPO/releases/latest") || true
            VERSION=$(resolve_stable_tag "$RELEASE_JSON")
        else
            RELEASE_JSON=$(curl -fsSL "https://api.github.com/repos/$GITHUB_REPO/releases") || true
            VERSION=$(resolve_beta_tag "$RELEASE_JSON")
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

    # Download the installer binary, capturing the HTTP status and curl's
    # stderr instead of discarding them — a DNS failure, timeout, TLS error,
    # 5xx, or rate-limit must never be reported as "no installer binary",
    # since that's only true for a genuine 404.
    CURL_ERR_FILE="$TEMP_DIR/curl-stderr"
    HTTP_CODE=$(curl -sSL -o "$TEMP_DIR/$INSTALLER_ASSET" -w '%{http_code}' "$INSTALLER_URL" 2>"$CURL_ERR_FILE") || true

    if [ "$HTTP_CODE" = "200" ]; then
        chmod +x "$TEMP_DIR/$INSTALLER_ASSET"
        print_info "Launching installer for $VERSION ($TRIPLE)"
        EXTRA_ARGS=(--version="$VERSION")
        [ "$CHANNEL" = "beta" ] && EXTRA_ARGS+=(--beta)
        exec "$TEMP_DIR/$INSTALLER_ASSET" "${EXTRA_ARGS[@]}" "${PASSTHROUGH[@]}"
    fi

    [ -n "$VERSION" ] && PASSTHROUGH+=("--version=$VERSION")

    if [ "$HTTP_CODE" = "404" ]; then
        # ---- Legacy fallback: release has no installer binary ----
        print_info "Release $VERSION has no installer binary; using the legacy $CHANNEL script."
    else
        CURL_ERR="$(cat "$CURL_ERR_FILE" 2>/dev/null)" || true
        print_error "Installer download failed (HTTP ${HTTP_CODE:-unknown}): ${CURL_ERR:-curl reported no output}"
        print_info "Falling back to the legacy $CHANNEL script."
    fi

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
}

# Run the real install flow only outside of test sourcing.
#
# The usual "was I sourced?" check (`[ "${BASH_SOURCE[0]}" = "$0" ]`) does
# NOT work here: the documented entry point pipes this file into `bash` over
# stdin (`curl ... | bash`), and in that mode bash sets `BASH_SOURCE[0]` to
# empty and `$0` to the literal string `bash` — they'd never match, so that
# guard would skip `main` for every real user, not just the test harness.
# A sentinel env var sidesteps that: it's unset (and so falsy here) for
# every real invocation, piped or not, and is only ever set by
# `scripts/test-install.sh` before it sources this file.
if [ -z "${INSTALL_SH_SOURCE_ONLY:-}" ]; then
    main "$@"
fi
