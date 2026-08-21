#!/bin/bash
# Unit tests for install.sh's pure, network-free logic: arch detection,
# channel validation, and release-tag resolution from a JSON string.
#
# install.sh is sourced with INSTALL_SH_SOURCE_ONLY set so its guard skips
# `main` and only defines the functions under test — see the comment on
# that guard in install.sh for why a plain `[ "${BASH_SOURCE[0]}" = "$0" ]`
# check doesn't work for a script whose documented entry point is
# `curl ... | bash`.
#
# Usage: bash scripts/test-install.sh   (or `just test-install`)

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

INSTALL_SH_SOURCE_ONLY=1
# shellcheck source=/dev/null
source "$REPO_ROOT/install.sh"

PASS=0
FAIL=0
SKIPPED=0

pass() {
    PASS=$((PASS + 1))
    echo "  ok   - $1"
}

fail() {
    FAIL=$((FAIL + 1))
    echo "  FAIL - $1"
    echo "         expected: [$2]"
    echo "         actual:   [$3]"
}

skip() {
    SKIPPED=$((SKIPPED + 1))
    echo "  skip - $1"
}

assert_eq() {
    local desc="$1" expected="$2" actual="$3"
    if [ "$expected" = "$actual" ]; then
        pass "$desc"
    else
        fail "$desc" "$expected" "$actual"
    fi
}

HAVE_JQ=0
if command -v jq >/dev/null 2>&1; then
    HAVE_JQ=1
fi

echo "== arch detection (detect_triple) =="
assert_eq "x86_64 maps to the gnu triple" \
    "x86_64-unknown-linux-gnu" "$(detect_triple x86_64)"
assert_eq "aarch64 maps to the gnu triple" \
    "aarch64-unknown-linux-gnu" "$(detect_triple aarch64)"
assert_eq "arm64 maps to the same triple as aarch64" \
    "aarch64-unknown-linux-gnu" "$(detect_triple arm64)"
if detect_triple riscv64 >/dev/null 2>&1; then
    fail "an unsupported machine value errors" "non-zero exit" "0"
else
    assert_eq "an unsupported machine value prints nothing" \
        "" "$(detect_triple riscv64 2>/dev/null)"
fi

echo "== channel validation (validate_channel) =="
if validate_channel stable; then
    pass "'stable' is a valid channel"
else
    fail "'stable' is a valid channel" "exit 0" "exit $?"
fi
if validate_channel beta; then
    pass "'beta' is a valid channel"
else
    fail "'beta' is a valid channel" "exit 0" "exit $?"
fi
if validate_channel nightly; then
    fail "a bogus channel is rejected" "non-zero exit" "exit 0"
else
    pass "a bogus channel is rejected"
fi

echo "== stable tag resolution (resolve_stable_tag) =="
FIXTURE_STABLE_LATEST='{
  "url": "https://api.github.com/repos/jorge-menjivar/super-stt/releases/1",
  "tag_name": "v0.2.3",
  "target_commitish": "main",
  "name": "v0.2.3",
  "draft": false,
  "prerelease": false,
  "body": "Stable release notes."
}'
assert_eq "extracts tag_name from a /releases/latest-shaped object" \
    "v0.2.3" "$(resolve_stable_tag "$FIXTURE_STABLE_LATEST")"

echo "== beta tag resolution (resolve_beta_tag) =="
FIXTURE_BETA_LIST='[
  {
    "tag_name": "v0.3.0-beta.2",
    "prerelease": true,
    "body": "Newest beta."
  },
  {
    "tag_name": "v0.3.0-beta.1",
    "prerelease": true,
    "body": "Older beta."
  },
  {
    "tag_name": "v0.2.9",
    "prerelease": false,
    "body": "Stable."
  }
]'
assert_eq "picks the newest prerelease from a /releases-shaped array" \
    "v0.3.0-beta.2" "$(resolve_beta_tag "$FIXTURE_BETA_LIST")"

FIXTURE_BETA_NONE='[
  {
    "tag_name": "v0.2.9",
    "prerelease": false,
    "body": "Stable."
  },
  {
    "tag_name": "v0.2.8",
    "prerelease": false,
    "body": "Older stable."
  }
]'
assert_eq "resolves empty when no release is a prerelease" \
    "" "$(resolve_beta_tag "$FIXTURE_BETA_NONE")"

# The `main` flow treats an empty resolved VERSION as a hard error (see the
# `[ -z "$VERSION" ]` check right after tag resolution) — confirm that
# happens cleanly (a clear message, exit 1) rather than main() carrying on
# with an empty VERSION. curl is shadowed with a shell function so this
# never touches the network.
MAIN_NO_PRERELEASE_OUT=$(
    curl() { printf '%s' "$FIXTURE_BETA_NONE"; }
    main --beta 2>&1
)
MAIN_NO_PRERELEASE_STATUS=$?
if [ "$MAIN_NO_PRERELEASE_STATUS" -eq 1 ] \
    && printf '%s' "$MAIN_NO_PRERELEASE_OUT" | grep -q "Could not resolve a beta release"; then
    pass "beta resolution with no prereleases makes main() fail cleanly"
else
    fail "beta resolution with no prereleases makes main() fail cleanly" \
        "exit 1, 'Could not resolve a beta release'" \
        "exit $MAIN_NO_PRERELEASE_STATUS, output: $MAIN_NO_PRERELEASE_OUT"
fi

# A release object with `prerelease` written before `tag_name`. The
# grep/paste/awk fallback assumes tag_name always comes first within an
# object and mis-pairs this one.
FIXTURE_BETA_SWAPPED='[
  {
    "prerelease": true,
    "tag_name": "v0.4.0-beta.1",
    "body": "Field order swapped."
  }
]'

# A release whose body value is exactly the word `prerelease` (e.g. a
# templated changelog placeholder). Its JSON string delimiters make the
# rendered line read `"body": "prerelease"` — a literal match for the
# fallback's grep pattern despite `body` not being the `prerelease` key.
# That extra match shifts every later `paste - -` pairing by one, so the
# pipeline never finds the real prerelease that follows.
FIXTURE_BETA_BODY_LITERAL='[
  {
    "tag_name": "v0.5.0",
    "prerelease": false,
    "body": "prerelease"
  },
  {
    "tag_name": "v0.4.9-beta.3",
    "prerelease": true,
    "body": "Latest beta before the stable cut."
  }
]'

echo "== grep/awk fallback, exercised directly regardless of jq availability =="
# `_resolve_stable_tag_grep`/`_resolve_beta_tag_grep` are what
# `resolve_stable_tag`/`resolve_beta_tag` fall back to when `jq` isn't
# installed. On a host that HAS jq (the normal case, e.g. this CI runner),
# the public resolvers never take this path at all — so without asserting
# against the private fallback functions directly, this whole path would
# ship with zero real coverage on any ordinary dev/CI machine. Honest tests
# of a known-weak path beat tests that silently skip.
assert_eq "grep fallback extracts tag_name from a /releases/latest-shaped object" \
    "v0.2.3" "$(_resolve_stable_tag_grep "$FIXTURE_STABLE_LATEST")"
assert_eq "grep/awk fallback picks the newest prerelease from a well-formed, conventionally-ordered array" \
    "v0.3.0-beta.2" "$(_resolve_beta_tag_grep "$FIXTURE_BETA_LIST")"
assert_eq "grep/awk fallback resolves empty when no release is a prerelease" \
    "" "$(_resolve_beta_tag_grep "$FIXTURE_BETA_NONE")"

# Known limitation of the grep/awk fallback, documented on
# `_resolve_beta_tag_grep` in install.sh: a field-order swap desyncs the
# positional pairing and the fallback resolves EMPTY rather than the real
# tag — it silently misses the beta release instead of picking a wrong one.
# `jq` (asserted below, when available) is what actually closes this; pinning
# the fallback's real behavior here is more honest than skipping the case
# whenever this host happens to have jq installed.
assert_eq "grep/awk fallback mis-pairs field-order-swapped JSON (known limitation, closed only by jq)" \
    "" "$(_resolve_beta_tag_grep "$FIXTURE_BETA_SWAPPED")"

echo "== beta resolution robustness (S1) =="
if [ "$HAVE_JQ" -eq 1 ]; then
    assert_eq "field-order-swapped JSON still resolves the tag" \
        "v0.4.0-beta.1" "$(resolve_beta_tag "$FIXTURE_BETA_SWAPPED")"
    assert_eq "a release body containing the literal word desyncs nothing" \
        "v0.4.9-beta.3" "$(resolve_beta_tag "$FIXTURE_BETA_BODY_LITERAL")"
else
    skip "field-order-swapped JSON still resolves the tag (needs jq)"
    skip "a release body quoting tag_name/prerelease doesn't desync the result (needs jq)"
fi

echo
echo "passed=$PASS failed=$FAIL skipped=$SKIPPED"
if [ "$FAIL" -ne 0 ]; then
    exit 1
fi
