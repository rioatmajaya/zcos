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
# is itself stable. So the check runs a control first: two dumps taken with no
# input must be byte-identical. Only then does a difference mean what it says.
#
# The check asserts:
#   1. an idle desktop is stable (two idle dumps are identical),
#   2. typing changes the frame (the window repainted on the keystrokes),
#   3. the session is still healthy afterwards (no watchdog, no content or
#      checksum mismatch, still running).
#
# It deliberately does *not* assert the exact pixels of the typed line. That would
# be a golden image: it would need editing for every future change to the
# terminal's layout, and it would fail for reasons that have nothing to do with
# whether input works. "Typing changed the screen" is the claim that was wrong.

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

# Wait for the desktop to finish its scripted session.
echo "== waiting for the scripted session to finish =="
waited=0
until grep -aq "compositor: window restored" "$tmp/serial.log" 2>/dev/null; do
    if [ "$waited" -ge "$boot_timeout" ]; then
        echo "error: the desktop never finished its scripted session" >&2
        tail -5 "$tmp/serial.log" 2>/dev/null || true
        exit 1
    fi
    sleep 1
    waited=$((waited + 1))
done
sleep 2

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

echo "== control: two idle dumps must be identical =="
shot
cp "$tmp/frame.ppm" "$tmp/idle_a.ppm"
shot
cp "$tmp/frame.ppm" "$tmp/idle_b.ppm"
if cmp -s "$tmp/idle_a.ppm" "$tmp/idle_b.ppm"; then
    echo "   idle desktop is stable (good: a difference later means input caused it)"
else
    echo "error: the idle desktop is not stable, so this check cannot conclude" >&2
    echo "       anything: something animates or drifts on its own." >&2
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
if cmp -s "$tmp/idle_a.ppm" "$tmp/frame.ppm"; then
    echo "error: the frame did not change after typing." >&2
    echo "       The window did not repaint, so live keystrokes are NOT reaching" >&2
    echo "       the client — which is exactly the claim this check exists for." >&2
    exit 1
fi
echo "   the frame changed: typed input reached the window"

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