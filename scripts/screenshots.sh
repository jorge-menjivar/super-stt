#!/usr/bin/env bash
# Screenshot the settings app for the README, off screen.
#
# Usage: scripts/screenshots.sh APP OUT_DIR [NAME...]
#   APP      a debug build of super-voice-app (`just screenshots` builds one)
#   OUT_DIR  where the PNGs go, as NAME.png
#   NAME     the shots to take (default: all of SHOTS below)
#
# An Xvfb display hosts a nested cosmic-comp, with cosmic-bg drawing the
# wallpaper, so the app renders frosted over it as it does on the desktop
# without anything appearing there. The compositor gets a copy of
# ~/.config/cosmic set to dark, floating windows and no active-window hint,
# and a state directory of its own, so it never writes to the real one.
#
# Each shot is one launch of the app with SUPER_VOICE_APP_SCENE naming the view
# (see super-voice-app/src/core/app/scene.rs). The app talks to the running
# daemon and changes nothing in it. It prints when its view is up, or where
# the widget to open is; xdotool clicks there through Xvfb. grim captures the
# window once the picture stops changing.
#
# The app runs on the real session bus, for its keyring token. The daemon
# binds that token to the binary, so the first run after a rebuild asks for
# consent on the desktop.
set -euo pipefail

# File name, then scene, in the sidebar's order, a feature's tabs in theirs.
# Keep in step with the README's captions.
SHOTS=(
    "app-1 models"
    "app-2 models/select-backend"
    "app-3 models/model-menu"
    "app-4 models/device-menu"
    "app-5 models/language"
    "app-6 library"
    "app-7 library/browse"
    "app-8 recording"
    "app-9 input-simulation"
    "app-10 contexts"
    "app-11 contexts/edit"
    "app-12 speech"
    "app-13 voices"
    "app-14 customization"
    "app-15 connection"
    "app-16 updates"
)

# The nested output, and where the window lands on it. The window size is
# `WINDOW_SIZE` in scene.rs. cosmic-comp opens a floating window at most two
# thirds of the output each way, so the output is at least 1.5 times that. It
# puts a window opened on an empty output in the middle across, and down by
# half the space left or an eighth of the output, whichever is less (its
# floating layout's `map_internal`).
OUTPUT_W=1920
OUTPUT_H=1280
WINDOW_W=1280
WINDOW_H=800
WINDOW_X=$(((OUTPUT_W - WINDOW_W) / 2))
WINDOW_Y=$(((OUTPUT_H - WINDOW_H) / 2))
if [ "$WINDOW_Y" -gt $((OUTPUT_H / 8)) ]; then
    WINDOW_Y=$((OUTPUT_H / 8))
fi

# Frames this many pixels apart count as the same. The header's GPU memory
# bar moves with every allocation on the card, and would otherwise keep a
# shot from ever settling.
SAME_PIXELS=200

die() {
    echo "error: $*" >&2
    exit 1
}

[ $# -ge 2 ] || die "usage: $0 APP OUT_DIR [NAME...]"
app=$1
out=$2
shift 2

[ -x "$app" ] || die "$app is not an executable"
missing=()
for tool in Xvfb xdotool cosmic-comp cosmic-bg grim magick dbus-run-session; do
    command -v "$tool" >/dev/null || missing+=("$tool")
done
[ ${#missing[@]} -eq 0 ] || die "missing: ${missing[*]}"
[ -d "$HOME/.config/cosmic" ] || die "no ~/.config/cosmic to take the theme and wallpaper from"

# The shots asked for, in SHOTS order.
selected=()
for shot in "${SHOTS[@]}"; do
    name=${shot%% *}
    if [ $# -eq 0 ]; then
        selected+=("$shot")
        continue
    fi
    for wanted in "$@"; do
        [ "$wanted" = "$name" ] && selected+=("$shot")
    done
done
for wanted in "$@"; do
    found=false
    for shot in "${SHOTS[@]}"; do
        [ "${shot%% *}" = "$wanted" ] && found=true
    done
    $found || die "no shot named $wanted (the shots are: $(printf '%s ' "${SHOTS[@]%% *}"))"
done
mkdir -p "$out"

# Everything the run starts lives under one runtime directory: the
# compositor's socket, its config copy and state, and the frames.
run=$(mktemp -d -p "${XDG_RUNTIME_DIR:-/tmp}" super-voice-screenshots.XXXXXX)
log="$run/log"
# Where the log is kept when the run fails, since the directory above goes.
kept_log="${TMPDIR:-/tmp}/super-voice-screenshots.log"
xvfb_pid=""
session_pid=""
bg_pid=""
app_pid=""

teardown() {
    local status=$?
    # Every step, whatever the one before did: on Ctrl-C the processes below
    # are already gone, and under `set -e` the first kill that found nothing
    # ended the teardown before it removed the run directory.
    set +e
    [ -n "$app_pid" ] && kill "$app_pid" 2>/dev/null
    [ -n "$bg_pid" ] && kill "$bg_pid" 2>/dev/null
    # cosmic-comp and the session bus are children of dbus-run-session.
    [ -n "$session_pid" ] && pkill -P "$session_pid" 2>/dev/null
    [ -n "$session_pid" ] && kill "$session_pid" 2>/dev/null
    [ -n "$xvfb_pid" ] && kill "$xvfb_pid" 2>/dev/null
    # Anything the private bus activated carries the runtime dir in its
    # environment. One grep over them all: one per process took seconds.
    local environ pid
    for environ in $(grep -lsF "$run" /proc/[0-9]*/environ 2>/dev/null); do
        pid=${environ#/proc/}
        kill "${pid%/environ}" 2>/dev/null
    done
    wait 2>/dev/null
    if [ "$status" -ne 0 ] && [ -f "$log" ]; then
        cp "$log" "$kept_log"
        echo "Log: $kept_log" >&2
    fi
    rm -rf "$run"
}
trap teardown EXIT

# Xvfb picks a free display and reports it on the fd.
Xvfb -displayfd 3 -screen 0 "${OUTPUT_W}x${OUTPUT_H}x24" -nolisten tcp 3>"$run/display" 2>>"$log" &
xvfb_pid=$!
for _ in $(seq 50); do
    [ -s "$run/display" ] && break
    sleep 0.1
done
[ -s "$run/display" ] || die "Xvfb did not start"
display=":$(cat "$run/display")"

mkdir -p "$run/config" "$run/state"
cp -r "$HOME/.config/cosmic" "$run/config/"
cosmic_config="$run/config/cosmic"
mkdir -p "$cosmic_config/com.system76.CosmicComp/v1" "$cosmic_config/com.system76.CosmicTheme.Mode/v1"
echo false >"$cosmic_config/com.system76.CosmicComp/v1/autotile"
echo false >"$cosmic_config/com.system76.CosmicComp/v1/active_hint"
echo true >"$cosmic_config/com.system76.CosmicTheme.Mode/v1/is_dark"

env -u WAYLAND_DISPLAY -u DISPLAY -u DBUS_SESSION_BUS_ADDRESS \
    XDG_RUNTIME_DIR="$run" XDG_CONFIG_HOME="$run/config" XDG_STATE_HOME="$run/state" \
    dbus-run-session -- env DISPLAY="$display" cosmic-comp --no-xwayland >>"$log" 2>&1 &
session_pid=$!
socket="$run/wayland-1"
for _ in $(seq 100); do
    [ -S "$socket" ] && break
    sleep 0.1
done
[ -S "$socket" ] || die "cosmic-comp did not start"

# The compositor's window is the output; resize it to the size wanted. Bounded:
# a compositor that died after making its socket never opens one.
window=$(DISPLAY=$display timeout 10 xdotool search --sync --onlyvisible --name '.' | head -n 1) ||
    true
[ -n "$window" ] || die "cosmic-comp did not open its window"
DISPLAY=$display xdotool windowsize "$window" "$OUTPUT_W" "$OUTPUT_H"
for _ in $(seq 50); do
    WAYLAND_DISPLAY=$socket grim "$run/output.png" 2>>"$log" || true
    [ "$(magick identify -format '%wx%h' "$run/output.png" 2>/dev/null)" = "${OUTPUT_W}x${OUTPUT_H}" ] && break
    sleep 0.1
done

# The wallpaper, on the compositor's own bus.
bus=$(tr '\0' '\n' <"/proc/$(pgrep -P "$session_pid" -x cosmic-comp)/environ" |
    sed -n 's/^DBUS_SESSION_BUS_ADDRESS=//p')
env -u DISPLAY WAYLAND_DISPLAY="$socket" DBUS_SESSION_BUS_ADDRESS="$bus" \
    XDG_CONFIG_HOME="$run/config" cosmic-bg >>"$log" 2>&1 &
bg_pid=$!
sleep 1

# A frame of the window, as an uncompressed PPM: frames are taken every
# second until the picture holds still, and compressing each to PNG took
# longer than the second between them.
capture() {
    WAYLAND_DISPLAY=$socket grim -t ppm -g "$WINDOW_X,$WINDOW_Y ${WINDOW_W}x${WINDOW_H}" "$1"
}

# Whether two frames show the same picture.
same() {
    local metric
    metric=$(magick compare -metric AE -fuzz 2% "$1" "$2" null: 2>&1 || true)
    [ "${metric%% *}" -le "$SAME_PIXELS" ] 2>/dev/null
}

# Save the window to $1, a PNG, once it has looked the same for three frames
# a second apart: data keeps arriving after the view is up. Gives up after
# 30 seconds and keeps the last frame.
settle() {
    local previous="$run/previous.ppm" current="$run/current.ppm" still=0
    capture "$previous"
    for _ in $(seq 30); do
        sleep 1
        capture "$current"
        if same "$previous" "$current"; then
            still=$((still + 1))
            if [ "$still" -ge 2 ]; then
                magick "$current" "$1"
                return 0
            fi
        else
            still=0
        fi
        mv "$current" "$previous"
    done
    echo "  never settled; keeping the last frame" >&2
    magick "$previous" "$1"
}

# The app's first `scene:` line, or nothing if it exits or $1 seconds pass.
# Only the first launch can be waiting on consent: the rest reuse its token.
scene_line() {
    local waited=0 line
    while [ "$waited" -lt "$(($1 * 10))" ]; do
        line=$(grep -m 1 '^scene: ' "$run/app.out" || true)
        if [ -n "$line" ]; then
            echo "${line#scene: }"
            return 0
        fi
        kill -0 "$app_pid" 2>/dev/null || return 0
        if $first && [ "$waited" -eq 100 ]; then
            echo "  waiting for the daemon; if a consent prompt is on your desktop, choose Allow" >&2
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
}

written=()
skipped=()
failed=()
first=true
for shot in "${selected[@]}"; do
    name=${shot%% *}
    scene=${shot#* }
    echo "$name: $scene"
    : >"$run/app.out"
    env -u DISPLAY WAYLAND_DISPLAY="$socket" XDG_CONFIG_HOME="$run/config" \
        SUPER_VOICE_APP_SCENE="$scene" RUST_LOG=warn "$app" >"$run/app.out" 2>>"$log" &
    app_pid=$!

    if $first; then line=$(scene_line 120); else line=$(scene_line 60); fi
    first=false
    case "$line" in
    ready)
        settle "$out/$name.png"
        written+=("$name")
        ;;
    click\ *)
        read -r x y <<<"${line#click }"
        # One frame: the app reports the widget only once it has held still.
        capture "$run/before.ppm"
        DISPLAY=$display xdotool mousemove $((WINDOW_X + x)) $((WINDOW_Y + y)) click 1
        # Off the window, so nothing in the shot shows a hover.
        DISPLAY=$display xdotool mousemove 0 0
        settle "$out/$name.png"
        if same "$run/before.ppm" "$out/$name.png"; then
            echo "  the click opened nothing" >&2
            failed+=("$name: the click opened nothing")
        else
            written+=("$name")
        fi
        ;;
    unavailable:\ *)
        echo "  skipped: ${line#unavailable: }"
        skipped+=("$name: ${line#unavailable: }")
        ;;
    *)
        echo "  the app did not report its scene" >&2
        failed+=("$name: the app did not report its scene")
        ;;
    esac

    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
    app_pid=""
done

echo
[ ${#written[@]} -eq 0 ] || echo "Wrote ${written[*]} to $out."
if [ ${#skipped[@]} -gt 0 ]; then
    echo "Skipped, for the daemon's current state:"
    printf '  %s\n' "${skipped[@]}"
fi
if [ ${#failed[@]} -gt 0 ]; then
    echo "Failed:" >&2
    printf '  %s\n' "${failed[@]}" >&2
    exit 1
fi
