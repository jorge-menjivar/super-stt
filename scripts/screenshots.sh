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
# (see super-voice-app/src/core/app/scene.rs). It prints when its view is up,
# or where the widget to open is; xdotool clicks there through Xvfb. grim
# captures the window once the picture stops changing.
#
# The Models page draws a different card for each state speech to text can be
# in, so each shot first puts the running daemon's speech to text in the one
# it needs: no backend, a backend with no model picked, or the model running
# (super-voice-app/src/core/app/scene/stt.rs). The run reads what speech to
# text is doing first, shows that backend and model, and puts it back at the
# end, after Ctrl-C too. Dictation is out while the model reloads.
#
# The app runs on the real session bus, for its keyring token. The daemon
# binds that token to the binary, so the first run after a rebuild asks for
# consent on the desktop.
set -euo pipefail

# File name, then scene, in the sidebar's order: the shots the README and the
# app's metainfo show. Keep in step with both.
SHOTS=(
    "models models/device-menu"
    "library library/browse"
    "contexts contexts/edit"
    "voices voices"
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
# Speech to text as found: its state, backend, model and device. Put back at
# the end once a shot may have moved it.
stt_state=""
stt_source=""
stt_model=""
stt_device=""
moved=false
first=true

# Start the app on scene $1, with speech to text's backend, model and device
# for the scene to put it on.
launch() {
    : >"$run/app.out"
    env -u DISPLAY WAYLAND_DISPLAY="$socket" XDG_CONFIG_HOME="$run/config" \
        SUPER_VOICE_APP_SCENE="$1" \
        SUPER_VOICE_APP_SCENE_STT_SOURCE="$stt_source" \
        SUPER_VOICE_APP_SCENE_STT_MODEL="$stt_model" \
        SUPER_VOICE_APP_SCENE_STT_DEVICE="$stt_device" \
        RUST_LOG=warn "$app" >"$run/app.out" 2>>"$log" &
    app_pid=$!
}

stop_app() {
    [ -n "$app_pid" ] || return 0
    kill "$app_pid" 2>/dev/null || true
    for _ in $(seq 50); do
        kill -0 "$app_pid" 2>/dev/null || break
        sleep 0.1
    done
    kill -KILL "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
    app_pid=""
}

# The app's first `scene:` line, or nothing if it exits or $1 seconds pass.
# Only the first launch can be waiting on consent: the rest reuse its token.
# A later one waits as long as speech to text takes to move: a model load.
scene_line() {
    local waited=0 line
    while [ "$waited" -lt "$(($1 * 10))" ]; do
        line=$(grep -m 1 '^scene: ' "$run/app.out" || true)
        if [ -n "$line" ]; then
            echo "${line#scene: }"
            return 0
        fi
        kill -0 "$app_pid" 2>/dev/null || return 0
        if [ "$waited" -eq 100 ]; then
            if $first; then
                echo "  waiting for the daemon; if a consent prompt is on your desktop, choose Allow" >&2
            else
                echo "  waiting for speech to text; a model may be loading" >&2
            fi
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
}

# Speech to text as it was, in words.
stt_described() {
    case "$stt_state" in
    empty) echo "no backend" ;;
    selected) echo "$stt_source, no model" ;;
    unloaded) echo "$stt_model from $stt_source, not loaded" ;;
    running) echo "$stt_model from $stt_source, running on $stt_device" ;;
    esac
}

# Put speech to text back as the run found it.
restore_stt() {
    echo "Putting speech to text back: $(stt_described)."
    first=false
    launch "stt/$stt_state"
    local line
    line=$(scene_line 300)
    stop_app
    if [ "$line" != ready ]; then
        echo "error: could not put speech to text back (${line:-the app did not say why})." >&2
        echo "It was: $(stt_described)." >&2
        return 1
    fi
}

teardown() {
    local status=$?
    # Every step, whatever the one before did: under `set -e` the first kill
    # that found nothing ended the teardown before it removed the run
    # directory. And no second Ctrl-C: speech to text is put back first. Only
    # INT: the app started to put it back inherits what is ignored here, and
    # has to stop on the TERM `stop_app` sends.
    set +e
    trap '' INT
    stop_app
    if $moved; then
        restore_stt || status=1
    fi
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
    exit "$status"
}
trap teardown EXIT
# Through the EXIT trap, so an interrupted run still puts speech to text back.
trap 'exit 130' INT
trap 'exit 143' TERM

# Xvfb, the compositor and the wallpaper run in sessions of their own, out of
# reach of the Ctrl-C that ends the run: the teardown still needs them, to put
# speech to text back. Xvfb picks a free display and reports it on the fd.
setsid Xvfb -displayfd 3 -screen 0 "${OUTPUT_W}x${OUTPUT_H}x24" -nolisten tcp 3>"$run/display" 2>>"$log" &
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
    setsid dbus-run-session -- env DISPLAY="$display" cosmic-comp --no-xwayland >>"$log" 2>&1 &
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
    XDG_CONFIG_HOME="$run/config" setsid cosmic-bg >>"$log" 2>&1 &
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

first=true
launch stt
line=$(scene_line 120)
stop_app
first=false
case "$line" in
stt$'\t'*) IFS=$'\t' read -r _ stt_state stt_source stt_model stt_device <<<"$line" ;;
*) die "could not read what speech to text is doing (${line:-the app did not say})" ;;
esac
[ "$stt_device" = "-" ] && stt_device=""
if [ "$stt_model" = "-" ]; then
    die "speech to text has no model to show. Load the one the shots should show, then run again."
fi
echo "Speech to text: $(stt_described). Put back at the end."

written=()
failed=()
for shot in "${selected[@]}"; do
    name=${shot%% *}
    scene=${shot#* }
    echo "$name: $scene"
    moved=true
    launch "$scene"
    line=$(scene_line 300)
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
    unavailable:\ * | failed:\ *)
        echo "  ${line#*: }" >&2
        failed+=("$name: ${line#*: }")
        ;;
    *)
        echo "  the app did not report its scene" >&2
        failed+=("$name: the app did not report its scene")
        ;;
    esac
    stop_app
done

echo
[ ${#written[@]} -eq 0 ] || echo "Wrote ${written[*]} to $out."
if [ ${#failed[@]} -gt 0 ]; then
    echo "Failed:" >&2
    printf '  %s\n' "${failed[@]}" >&2
    exit 1
fi
