#!/usr/bin/env sh
# Prove that live keyboard input reaches the window client and repaints it.
#
# Usage: tools/check-live-input.sh
#
# Why this exists
# ---------------
# The boot checks prove the *scripted* frame: the kernel serves a fixed mouse and
# keyboard script and recomputes the desktop from it. That says nothing about
# input a person actually types. For a long stretch this repository carried a
# written claim that the window "shows the terminal's scripted keystrokes rather
# than anything typed" — and nothing in CI contradicted it, because no check typed
# anything. The claim was false; a user had to prove it with a screenshot.
#
# A green boot test is therefore not evidence about live input, and this script is
# the evidence. It fails if the claim regresses.
#
# How it decides
# --------------
# A screendump before and after typing is only meaningful if an untouched desktop
# is itself stable. The default image now has one deliberate exception: the
# taskbar clock shows live `HH:MM:SS`, so the top panel must change while the
# desktop below it stays still. The check runs a control first: two idle dumps
# two seconds apart must agree below the 32px panel, and the panel must differ
# (otherwise the clock is frozen). Only then does a difference below the panel
# mean that typing reached the window.
#
# The check asserts:
#   1. the human-testable image contains the file manager and starts its task,
#   2. the live clock is actually live (`clock: rtc ok`, a clock tick marker,
#      and panel pixels that change between two idle dumps),
#   3. the desktop below the panel is stable while idle,
#   4. typing changes the desktop below the panel (the window repainted),
#   5. the session is still healthy afterwards (no watchdog, no content or
#      checksum mismatch, still running).
#
# It deliberately does *not* assert the exact pixels of the typed line. That would
# be a golden image: it would need editing for every future change to the
# terminal's layout, and it would fail for reasons that have nothing to do with
# whether input works. "Typing changed the screen" is the claim that was wrong,
# and the clock only ever changes the panel it owns.

set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

image=build/zcos.img
build_hint="tools/build-efi.sh"
[ "$#" -gt 0 ] && [ "$1" = "--test-close" ] && { image=build/zcos-close.img; build_hint="tools/build-efi.sh --test-close"; }

if [ ! -f "$image" ]; then
    echo "error: $image not found; build it with: $build_hint" >&2
    exit 1
fi
# Same rule as run-qemu.sh: never test an image older than the sources.
if find boot kernel libs user -name '*.rs' -newer "$image" -print -quit 2>/dev/null | grep -q .; then
    echo "error: $image is older than the sources; rebuild with: $build_hint" >&2
    exit 1
fi

port=${ZC_LIVE_INPUT_PORT:-4455}
boot_timeout=${ZC_LIVE_INPUT_TIMEOUT:-60}

tmp=$(mktemp -d)
qemu_pid=""
cleanup() {
    if [ -n "$qemu_pid" ]; then
        kill "$qemu_pid" 2>/dev/null || true
        wait "$qemu_pid" 2>/dev/null || true
    fi
    rm -rf "$tmp"
}
trap cleanup EXIT

# Locate a 4 MB OVMF code/variables pair, as run-qemu.sh does.
ovmf=""
for dir in /usr/share/OVMF /usr/share/ovmf /usr/share/edk2*; do
    for pair in "OVMF_CODE.fd:OVMF_VARS.fd" "OVMF_CODE_4M.fd:OVMF_VARS_4M.fd"; do
        code=${pair%%:*}
        vars=${pair##*:}
        if [ -f "$dir/$code" ] && [ -f "$dir/$vars" ]; then
            ovmf="$dir|$code|$vars"
            break 2
        fi
    done
done
if [ -z "$ovmf" ]; then
    echo "error: no OVMF firmware pair found" >&2
    exit 1
fi
ovmf_dir=${ovmf%%|*}
rest=${ovmf#*|}
code_file=${rest%%|*}
vars_file=${rest##*|}
cp "$ovmf_dir/$vars_file" "$tmp/VARS.fd"

echo "== preparing the boot disk =="
./tools/mkdisk.sh >/dev/null

echo "== booting $image headless with a monitor on 127.0.0.1:$port =="
qemu-system-x86_64 \
    -machine q35 -m 512M -net none -vga std -no-reboot \
    -drive "if=pflash,format=raw,readonly=on,file=$ovmf_dir/$code_file" \
    -drive "if=pflash,format=raw,file=$tmp/VARS.fd" \
    -drive "if=virtio,format=raw,file=$image" \
    -display none -serial "file:$tmp/serial.log" \
    -monitor "tcp:127.0.0.1:$port,server,nowait" \
    </dev/null >/dev/null 2>&1 &
qemu_pid=$!

# Send monitor commands. Each argument is one command, sent in order.
mon() {
    python3 - "$port" "$@" <<'PY'
import socket, sys, time
port = int(sys.argv[1])
deadline = time.time() + 20
sock = None
while sock is None and time.time() < deadline:
    try:
        sock = socket.create_connection(("127.0.0.1", port), timeout=2)
    except OSError:
        time.sleep(0.3)
if sock is None:
    sys.exit("cannot reach the QEMU monitor")
sock.settimeout(2)
try:
    sock.recv(65536)  # the banner
except OSError:
    pass
for command in sys.argv[2:]:
    sock.sendall((command + "\n").encode())
    time.sleep(0.15)
    try:
        sock.recv(65536)
    except OSError:
        pass
time.sleep(0.4)
sock.close()
PY
}

# Wait for the desktop to reach its idle event loop.
#
# The interactive image serves no scripted session (the kernel gates the mouse
# and keyboard scripts on the `qemu-exit` test builds), so there is no
# `compositor: window restored` marker to wait for. The compositor logs
# `compositor: cursor ok` right after its (empty) scripted cursor session and
# before it blocks in the reply channel, so that marker is the point where the
# desktop is up, both windows are painted, and it is idle waiting for input.
echo "== waiting for the desktop to come up =="
waited=0
until grep -aq "compositor: cursor ok" "$tmp/serial.log" 2>/dev/null; do
    if [ "$waited" -ge "$boot_timeout" ]; then
        echo "error: the desktop never came up" >&2
        tail -5 "$tmp/serial.log" 2>/dev/null || true
        exit 1
    fi
    sleep 1
    waited=$((waited + 1))
done
sleep 2

# The image a person boots must show the apps the roadmap just added, not only
# the ones a special CI image proves. `fm.elf` is packed into this image and the
# compositor opens it as window 1 before the first frame; `fm: ready` is that
# task running, `window 1 launched` is the compositor actually opening it, and
# `fm: listing ok` is the file manager painting its first directory. The
# scripted `--fm` boot still proves the launcher, navigation, and preview; this
# check makes sure the human-testable image neither omits nor hides the app.
for marker in "task 9: fm: ready" "compositor: window 1 launched" "task 9: fm: listing ok"; do
    if ! grep -aq "$marker" "$tmp/serial.log"; then
        echo "error: the human-testable image is missing '$marker'" >&2
        echo "       it must include the file manager, not only the CI proof image" >&2
        exit 1
    fi
done

# The default image must prove the clock path, not only have a clock-shaped
# label. A proof image is the one exception: it must keep the fixed reading so
# its recorded checksum stays exact.
if [ "$image" = "build/zcos.img" ]; then
    clock=live
    for marker in "clock: rtc ok" "compositor: clock live"; do
        if ! grep -aq "$marker" "$tmp/serial.log"; then
            echo "error: the live clock is missing '$marker'" >&2
            exit 1
        fi
    done
    # Wait for the first real tick instead of racing a fixed sleep. The timer
    # nudges the compositor every ~250ms; the compositor logs this marker once,
    # when the displayed second actually changes.
    tick_waited=0
    until grep -aq "compositor: clock tick" "$tmp/serial.log" 2>/dev/null; do
        if [ "$tick_waited" -ge 10 ]; then
            echo "error: the live clock never logged a tick" >&2
            exit 1
        fi
        sleep 1
        tick_waited=$((tick_waited + 1))
    done
else
    clock=fixed
    for marker in "clock: proof" "compositor: clock fixed"; do
        if ! grep -aq "$marker" "$tmp/serial.log"; then
            echo "error: the proof image is missing '$marker'" >&2
            exit 1
        fi
    done
fi

shot() {
    rm -f "$tmp/frame.ppm"
    mon "screendump $tmp/frame.ppm" >/dev/null
    i=0
    while [ ! -s "$tmp/frame.ppm" ]; do
        if [ "$i" -ge 20 ]; then
            echo "error: screendump produced nothing" >&2
            exit 1
        fi
        sleep 0.5
        i=$((i + 1))
    done
}

# Compares two PPM dumps while ignoring the live taskbar clock.
#
# Usage: compare_frames idle|typed live|fixed a.ppm b.ppm
#
# `idle` requires the desktop below the 32px panel to be identical; when the
# clock is live it also requires the panel to differ, otherwise the clock is
# frozen. `typed` requires the desktop below the panel to differ. The panel is
# never part of the typed comparison, so a ticking clock can neither hide nor
# create a keyboard repaint.
compare_frames() {
    python3 - "$1" "$2" "$3" "$4" <<'PY'
import sys

mode, clock, first, second = sys.argv[1:]

def read_ppm(path):
    with open(path, "rb") as handle:
        magic = handle.readline().strip()
        if magic != b"P6":
            sys.exit(f"{path}: not a binary PPM (got {magic!r})")
        line = handle.readline()
        while line.startswith(b"#"):
            line = handle.readline()
        width, height = map(int, line.split())
        maximum = int(handle.readline())
        if maximum != 255:
            sys.exit(f"{path}: unsupported maximum {maximum}")
        pixels = handle.read()
    if len(pixels) != width * height * 3:
        sys.exit(f"{path}: truncated pixel data")
    return width, height, pixels

aw, ah, a = read_ppm(first)
bw, bh, b = read_ppm(second)
if (aw, ah) != (bw, bh):
    sys.exit(f"frame sizes differ: {aw}x{ah} vs {bw}x{bh}")

panel_rows = min(32, ah)
cut = panel_rows * aw * 3
below_same = a[cut:] == b[cut:]
panel_same = a[:cut] == b[:cut]

if mode == "idle":
    if not below_same:
        sys.exit("idle desktop changed below the taskbar; something animates or drifts")
    if clock == "live" and panel_same:
        sys.exit("live clock panel did not change between idle dumps; the clock is frozen")
elif mode == "typed":
    if below_same:
        sys.exit("typed input did not change the window below the taskbar")
else:
    sys.exit(f"unknown compare mode: {mode}")
PY
}

echo "== control: idle desktop stable, live clock panel changing =="
shot
cp "$tmp/frame.ppm" "$tmp/idle_a.ppm"
# Two seconds is enough for the displayed second to differ. The control does
# not require the whole frame to match: the panel is the clock's own region.
sleep 2
shot
cp "$tmp/frame.ppm" "$tmp/idle_b.ppm"
if compare_frames idle "$clock" "$tmp/idle_a.ppm" "$tmp/idle_b.ppm"; then
    if [ "$clock" = "live" ]; then
        echo "   desktop below the panel is stable; the clock panel advanced"
    else
        echo "   idle proof desktop is stable (good: a difference later means input caused it)"
    fi
else
    echo "error: the idle control failed, so this check cannot conclude anything" >&2
    exit 1
fi

echo "== typing 'cat hello.txt' + Enter through the monitor =="
# QEMU's key names, not the characters: `.` is `dot`, and a shifted symbol is
# `shift-1`. Sending the literal characters silently drops the ones that are not
# key names, which would make the test weaker than it looks.
for key in c a t spc h e l l o dot t x t ret; do
    mon "sendkey $key" >/dev/null
done
sleep 3

shot
if compare_frames typed "$clock" "$tmp/idle_a.ppm" "$tmp/frame.ppm"; then
    echo "   the window changed: typed input reached the client"
else
    echo "error: the window did not change after typing." >&2
    echo "       Live keystrokes are NOT reaching the client — exactly the claim" >&2
    echo "       this check exists for." >&2
    exit 1
fi

echo "== the session is still healthy =="
for marker in "timed out" "mismatch" "error:"; do
    if grep -aq "$marker" "$tmp/serial.log"; then
        echo "error: the log contains '$marker' after live input" >&2
        grep -a "$marker" "$tmp/serial.log" | tail -3 >&2
        exit 1
    fi
done
if ! kill -0 "$qemu_pid" 2>/dev/null; then
    echo "error: the machine died during live input" >&2
    exit 1
fi
echo "   no timeout, no mismatch, still running"

echo
echo "live input ok: typed keystrokes reach the window client and repaint it"