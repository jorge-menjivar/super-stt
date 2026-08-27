#!/bin/bash
# End-to-end install verification: performs a REAL install of a REAL
# published release onto THIS machine, asserts the complete installed tree,
# then runs uninstall.sh and asserts nothing is left behind.
#
# This is the counterpart to scripts/test-install.sh, which only covers
# install.sh's pure, network-free logic (arch detection, channel validation,
# tag resolution from fixture JSON). Nothing there — and nothing in
# super-stt-install's own `--dry-run` e2e test — ever writes a file, escalates,
# or runs post-install. That is what this script covers.
#
# Two passes run per channel, each install → assert → uninstall → assert-clean:
#
#   1. Bootstrap pass. `install.sh --<channel>`, the documented `curl | bash`
#      path: it resolves the release, downloads the installer asset published
#      with it, and execs that. This is the released installer, so the pass
#      verifies the bootstrap's own resolution and asset naming against a real
#      release.
#   2. Local pass. The same install driven by a locally built
#      `super-stt-install` ($SUPER_STT_INSTALLER_BIN), so changes to the
#      installer crate in a PR are what is under test rather than whatever the
#      last release shipped.
#
# The fixture is deliberately the *published* tarball rather than one built
# here: it makes the job cheap (only the installer binary compiles — no GUI
# crates) and it tests exactly the path real users and the in-app updater take.
# The trade-off is that a change to the dist layout (a new resource in
# release.yml that stage::build_manifest starts requiring) fails this test
# until that release actually ships.
#
# Usage: SUPER_STT_INSTALL_E2E_YES=1 bash scripts/test-install-e2e.sh [stable|beta]
#        (or `just test-install-e2e beta`)

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

GITHUB_REPO="jorge-menjivar/super-stt"

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'
info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
err() { echo -e "${RED}[ERROR]${NC} $1"; }

# ---- Destructive-run guard ------------------------------------------------

# This script installs to, and then deletes from, the real /usr/local and
# /usr/lib/systemd/user — the installer hardcodes both prefixes, so there is
# no sandbox to point it at. That is fine on a disposable CI runner and fine
# in a container; it is not something to run by accident on a workstation
# that has Super STT installed. Opting in is explicit and never defaulted.
if [ "${SUPER_STT_INSTALL_E2E_YES:-}" != "1" ]; then
    err "This test performs a REAL install into /usr/local and /usr/lib/systemd/user,"
    err "then uninstalls it. It will remove an existing Super STT install on this host."
    err ""
    err "Run it on a disposable machine (CI) or in a container, and opt in explicitly:"
    err "  SUPER_STT_INSTALL_E2E_YES=1 bash scripts/test-install-e2e.sh [stable|beta]"
    exit 2
fi

CHANNEL="${1:-stable}"
case "$CHANNEL" in
    stable | beta) ;;
    *)
        err "Unknown channel '$CHANNEL' — expected 'stable' or 'beta'."
        exit 2
        ;;
esac

# ---- Tooling preflight ----------------------------------------------------

# `script` (util-linux) is load-bearing, not a convenience: a non-dry-run
# install always escalates, and escalate::pick_method only picks `sudo` when
# stderr is a TTY — otherwise it picks `pkexec`, which has no agent to talk to
# on a headless runner. Running the installer under a pty is what puts it on
# the sudo path, where CI's passwordless sudo just works.
MISSING=()
for tool in curl jq tar sudo script; do
    command -v "$tool" >/dev/null 2>&1 || MISSING+=("$tool")
done
if [ "${#MISSING[@]}" -ne 0 ]; then
    err "missing required tools: ${MISSING[*]}"
    exit 2
fi

INSTALLER_BIN="${SUPER_STT_INSTALLER_BIN:-}"

WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT

PASS=0
FAIL=0
XFAIL=0
SKIPPED=0

pass() {
    PASS=$((PASS + 1))
    echo "  ok    - $1"
}

fail() {
    FAIL=$((FAIL + 1))
    echo "  FAIL  - $1"
    if [ "$#" -gt 1 ]; then
        echo "          expected: [$2]"
        echo "          actual:   [$3]"
    fi
}

xfail() {
    XFAIL=$((XFAIL + 1))
    echo "  xfail - $1"
}

skip() {
    SKIPPED=$((SKIPPED + 1))
    echo "  skip  - $1"
}

note() { echo "  note  - $1"; }

# ---- The complete installed tree ------------------------------------------

# Every file `--components=all` installs, as `mode:path`, mirroring
# super-stt-install/src/stage.rs::build_manifest. `stt` has no source in the
# tarball — build_manifest generates the wrapper — and `super-stt-install` is
# the installer copying its own binary into place; both are as much a part of
# a complete install as anything unpacked from the release.
INSTALLED_FILES=(
    "755:/usr/local/bin/super-stt-daemon"
    "755:/usr/local/bin/super-stt-cli"
    "755:/usr/local/bin/super-stt-consent"
    "755:/usr/local/bin/super-stt-app"
    "755:/usr/local/bin/super-stt-cosmic-applet"
    "755:/usr/local/bin/super-stt-install"
    "755:/usr/local/bin/stt"
    "644:/usr/lib/systemd/user/super-stt.service"
    "644:/usr/local/share/applications/super-stt-app.desktop"
    "644:/usr/local/share/applications/super-stt-cosmic-applet-full.desktop"
    "644:/usr/local/share/applications/super-stt-cosmic-applet-left.desktop"
    "644:/usr/local/share/applications/super-stt-cosmic-applet-right.desktop"
    "644:/usr/local/share/icons/hicolor/scalable/apps/super-stt-app.svg"
    "644:/usr/local/share/icons/hicolor/scalable/apps/super-stt-cosmic-applet.svg"
    "644:/usr/local/share/metainfo/super-stt-app.metainfo.xml"
)

# KNOWN GAP — uninstall.sh's binary loop (the `for bin in ...` list) omits
# `super-stt-install`, so every install leaves that one file behind after an
# uninstall. It is asserted below as an expected failure rather than dropped
# from the list, so the gap stays visible and this harness still lands green;
# the follow-up that adds the binary to uninstall.sh removes both this
# constant and the `sudo rm -f` that compensates for it between passes.
KNOWN_UNINSTALL_LEFTOVER="/usr/local/bin/super-stt-install"

path_of() { echo "${1#*:}"; }
mode_of() { echo "${1%%:*}"; }

# ---- Release resolution ---------------------------------------------------

gh_api() {
    if [ -n "${GITHUB_TOKEN:-}" ]; then
        curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" "$1"
    else
        curl -fsSL "$1"
    fi
}

latest_stable_tag() {
    gh_api "https://api.github.com/repos/$GITHUB_REPO/releases/latest" \
        | jq -r '.tag_name // empty'
}

newest_prerelease_tag() {
    gh_api "https://api.github.com/repos/$GITHUB_REPO/releases" \
        | jq -r '[.[] | select(.prerelease==true)][0].tag_name // empty'
}

# `is_accepted <tag> <space-separated accepted tags>`
is_accepted() {
    local candidate="$1" accepted="$2" t
    for t in $accepted; do
        [ "$t" = "$candidate" ] && return 0
    done
    return 1
}

# Render an accepted-tag list for an assertion message.
accepted_desc() {
    if [ "$(echo "$1" | wc -w)" -gt 1 ]; then
        echo "one of: $1"
    else
        echo "$1"
    fi
}

# ---- Running an install ---------------------------------------------------

# Run `$1` under a pty (see the `script` note in the preflight above),
# capturing everything it writes to `$2`. Returns the child's exit status.
run_under_pty() {
    local cmd="$1" log="$2"
    script -qec "$cmd" /dev/null >"$log" 2>&1
}

# Assert an install run's outcome from its --json-progress event stream.
# $1 exit status, $2 log path, $3 accepted tags, $4 label.
#
# Two outcomes are accepted:
#   - a `complete` event for an accepted tag and all three components; or
#   - a `post_install_failed` error whose message is the daemon start/restart
#     step. That step is the only hard error in the whole post-install
#     sequence (every other step logs a warning), and it cannot succeed on a
#     headless runner: `systemctl --user` has no user session bus to talk to,
#     and the daemon fail-fasts without a keyring even if it did. Every file
#     is already on disk by the time post-install runs, and the tree
#     assertions below are the real gate — so this is tolerated, narrowly, by
#     message. Any other error code, and any other post_install_failed
#     message, is a real failure.
assert_install_outcome() {
    local status="$1" log="$2" accepted="$3" label="$4"
    local events complete_evt error_code error_msg

    # install.sh prints its own non-JSON `[INFO]` lines before exec'ing the
    # installer, and the pty leaves CR line endings behind; keep only the
    # lines that parse as JSON.
    events=$(tr -d '\r' <"$log" | jq -R 'fromjson? // empty' -c 2>/dev/null)

    complete_evt=$(echo "$events" | jq -c 'select(.event=="complete")' | tail -1)
    error_code=$(echo "$events" | jq -r 'select(.event=="error") | .code' | tail -1)
    error_msg=$(echo "$events" | jq -r 'select(.event=="error") | .message' | tail -1)

    if [ -n "$complete_evt" ]; then
        local got_version got_components
        got_version=$(echo "$complete_evt" | jq -r '.installed_version')
        got_components=$(echo "$complete_evt" | jq -r '.components | join(",")')
        if is_accepted "$got_version" "$accepted"; then
            pass "$label: completed on $got_version"
        else
            fail "$label: completed on a release of the $CHANNEL channel" \
                "$(accepted_desc "$accepted")" "$got_version"
        fi
        if [ "$got_components" = "daemon,app,applet" ]; then
            pass "$label: installed all three components"
        else
            fail "$label: installed all three components" "daemon,app,applet" "$got_components"
        fi
        if [ "$status" -ne 0 ]; then
            fail "$label: exited 0 after completing" "0" "$status"
        fi
        return
    fi

    if [ "$error_code" = "post_install_failed" ] \
        && echo "$error_msg" | grep -qE "daemon (start|restart) failed"; then
        xfail "$label: daemon start (no user systemd session on this host): $error_msg"
        note "files are all installed by this point — the tree assertions below are the gate"
        return
    fi

    fail "$label: install run reported a usable outcome" \
        "complete, or post_install_failed on the daemon start step" \
        "exit $status, error code '${error_code:-none}': ${error_msg:-no error event}"
    echo "--- last 40 lines of $label output ---"
    tail -40 "$log" | sed 's/^/          /'
    echo "--- end ---"
}

# ---- Assertions on the installed tree -------------------------------------

# $1 accepted tags, $2 label.
assert_installed_tree() {
    local accepted="$1" label="$2" entry path mode actual missing=0 wrong_mode=0

    for entry in "${INSTALLED_FILES[@]}"; do
        path="$(path_of "$entry")"
        mode="$(mode_of "$entry")"
        if [ ! -f "$path" ]; then
            fail "$label: $path is installed" "a regular file" "missing"
            missing=$((missing + 1))
            continue
        fi
        actual=$(stat -c '%a' "$path")
        if [ "$actual" != "$mode" ]; then
            fail "$label: $path has mode $mode" "$mode" "$actual"
            wrong_mode=$((wrong_mode + 1))
        fi
    done
    if [ "$missing" -eq 0 ]; then
        pass "$label: every file of a complete install is present (${#INSTALLED_FILES[@]} files)"
    fi
    if [ "$wrong_mode" -eq 0 ]; then
        pass "$label: every installed file carries its manifest mode"
    fi

    # The unit is what `systemctl --user enable super-stt` will read; a
    # truncated or mangled copy would still satisfy the existence check above.
    if grep -q '^\[Service\]' /usr/lib/systemd/user/super-stt.service \
        && grep -q '^ExecStart=' /usr/lib/systemd/user/super-stt.service; then
        pass "$label: the systemd unit has a [Service] section with ExecStart"
    else
        fail "$label: the systemd unit has a [Service] section with ExecStart" \
            "a parseable unit" "$(head -5 /usr/lib/systemd/user/super-stt.service 2>&1)"
    fi

    # The installed binaries are the real release binaries, so they must run —
    # and the version they report is what proves a release of the requested
    # channel landed, not merely that some files did.
    local cli_out version
    cli_out=$(/usr/local/bin/super-stt-cli --version 2>&1)
    version="${cli_out#super-stt-cli }"
    if [ "$cli_out" != "$version" ] && is_accepted "v$version" "$accepted"; then
        pass "$label: the installed CLI runs and reports $version"
    else
        fail "$label: the installed CLI runs and reports its version" \
            "super-stt-cli <$(accepted_desc "$accepted")>" "$cli_out"
    fi

    # The generated `stt` wrapper is what keyboard shortcuts invoke; a wrapper
    # pointing at the wrong prefix would be invisible to every check above.
    local wrapper_out
    wrapper_out=$(/usr/local/bin/stt --version 2>&1)
    if [ "$wrapper_out" = "$cli_out" ]; then
        pass "$label: the stt wrapper execs the installed CLI"
    else
        fail "$label: the stt wrapper execs the installed CLI" "$cli_out" "$wrapper_out"
    fi
}

assert_clean_tree() {
    local label="$1" entry path leftovers=0

    for entry in "${INSTALLED_FILES[@]}"; do
        path="$(path_of "$entry")"
        if [ "$path" = "$KNOWN_UNINSTALL_LEFTOVER" ]; then
            if [ -e "$path" ]; then
                xfail "$label: $path is removed (uninstall.sh's binary list omits it)"
            else
                pass "$label: $path is removed"
                note "the known uninstall gap is fixed — drop KNOWN_UNINSTALL_LEFTOVER from this script"
            fi
            continue
        fi
        if [ -e "$path" ] || [ -L "$path" ]; then
            fail "$label: $path is removed" "absent" "still present"
            leftovers=$((leftovers + 1))
        fi
    done
    if [ "$leftovers" -eq 0 ]; then
        pass "$label: uninstall left nothing behind"
    fi

    # Compensates for the known gap so the next pass starts from a genuinely
    # clean host; removed together with KNOWN_UNINSTALL_LEFTOVER.
    sudo rm -f "$KNOWN_UNINSTALL_LEFTOVER"
}

# ---- Passes ---------------------------------------------------------------

run_uninstall() {
    local label="$1" log="$WORK_DIR/uninstall.log"
    if bash "$REPO_ROOT/uninstall.sh" >"$log" 2>&1; then
        pass "$label: uninstall.sh exited 0"
    else
        fail "$label: uninstall.sh exited 0" "0" "$?"
        tail -20 "$log" | sed 's/^/          /'
    fi
}

# install.sh resolves the newest *prerelease* for `--beta` and pins it with
# `--version=`, so the bootstrap pass expects exactly that tag.
bootstrap_pass() {
    local log="$WORK_DIR/bootstrap.log" accepted status
    echo "== bootstrap pass: install.sh --$CHANNEL =="
    if [ "$CHANNEL" = "beta" ]; then
        accepted="$BETA_TAG"
    else
        accepted="$STABLE_TAG"
    fi
    run_under_pty \
        "bash '$REPO_ROOT/install.sh' --$CHANNEL --non-interactive --json-progress --components=all" \
        "$log"
    status=$?
    assert_install_outcome "$status" "$log" "$accepted" "bootstrap"
    assert_installed_tree "$accepted" "bootstrap"
    run_uninstall "bootstrap"
    assert_clean_tree "bootstrap"
}

# The installer's own `--beta` means "prereleases are *also* eligible", and
# resolve::pick_release then takes the highest semver of the two — so a
# stable release that supersedes the newest beta is the correct answer here
# (the beta-to-stable update path). Which of the two wins is pinned by
# resolve.rs's own unit tests; this pass accepts either and checks that
# whichever one it picked is what actually ended up on disk.
local_pass() {
    local log="$WORK_DIR/local.log" accepted flags="" status
    echo "== local pass: the installer built from this tree =="
    if [ -z "$INSTALLER_BIN" ]; then
        skip "local pass: set SUPER_STT_INSTALLER_BIN to a built super-stt-install to run it"
        return
    fi
    if [ ! -x "$INSTALLER_BIN" ]; then
        fail "local pass: SUPER_STT_INSTALLER_BIN is executable" \
            "an executable installer" "$INSTALLER_BIN"
        return
    fi
    if [ "$CHANNEL" = "beta" ]; then
        accepted="$BETA_TAG $STABLE_TAG"
        flags="--beta"
    else
        accepted="$STABLE_TAG"
    fi
    run_under_pty \
        "'$INSTALLER_BIN' $flags --non-interactive --json-progress --components=all" \
        "$log"
    status=$?
    assert_install_outcome "$status" "$log" "$accepted" "local"
    assert_installed_tree "$accepted" "local"
    run_uninstall "local"
    assert_clean_tree "local"
}

# ---- Main -----------------------------------------------------------------

# Starting dirty would make a leftover file read as a successful install (or
# an uninstall failure), so refuse rather than clobber whatever is here.
PREEXISTING=()
for entry in "${INSTALLED_FILES[@]}"; do
    path="$(path_of "$entry")"
    [ -e "$path" ] && PREEXISTING+=("$path")
done
if [ "${#PREEXISTING[@]}" -ne 0 ]; then
    err "Super STT is already installed on this host — refusing to run:"
    printf '        %s\n' "${PREEXISTING[@]}"
    err "Uninstall it first (bash uninstall.sh), or run this in a container."
    exit 2
fi

# Resolved straight from the API with `jq` rather than by sourcing install.sh's
# own resolvers: reusing those here would make the bootstrap pass assert
# install.sh against itself. Their logic is already pinned against fixtures in
# scripts/test-install.sh.
STABLE_TAG=$(latest_stable_tag)
if [ -z "$STABLE_TAG" ]; then
    err "Could not resolve a stable release for $GITHUB_REPO"
    exit 2
fi
BETA_TAG=""
if [ "$CHANNEL" = "beta" ]; then
    BETA_TAG=$(newest_prerelease_tag)
    if [ -z "$BETA_TAG" ]; then
        err "Could not resolve a beta release for $GITHUB_REPO"
        exit 2
    fi
fi

info "channel=$CHANNEL latest stable=$STABLE_TAG${BETA_TAG:+ newest prerelease=$BETA_TAG}"
if [ -n "$INSTALLER_BIN" ]; then
    info "local installer=$INSTALLER_BIN"
fi
echo

bootstrap_pass
echo
local_pass

echo
echo "passed=$PASS failed=$FAIL xfail=$XFAIL skipped=$SKIPPED"
if [ "$FAIL" -ne 0 ]; then
    exit 1
fi
