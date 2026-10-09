#!/usr/bin/env sh
# Prove clean shutdown: power off the running desktop and check the aftermath.
#
# Usage: tools/check-poweroff.sh
#
# Why this exists
# ---------------
# An interactive `tools/run-qemu.sh` used to run until killed, and killing it
# is a yank: whatever the write-back cache and the zcfs log had not yet made
# durable is left torn, and the next boot must recover from a crash that never
# needed to happen. `SYS_POWEROFF` (34) exists so the machine can stop itself —
# the shell's `poweroff` flushes and unmounts `/data` through the same path
# `umount` uses (so `mark_clean` stamps `FLAG_CLEAN`), then writes ACPI S5.
#
# How it decides
# --------------
# The check asserts three independent signals, because each one alone lies:
#   1. QEMU exited 0. Exit 0 without the marker could be any early exit, but a
#      machine that powered off is a machine whose emulator had nothing left
#      to run.
#   2. The log holds `power: halt clean`. The marker without the fsck could be
#      a log line ahead of a failed flush, so on its own it is only a claim.
#   3. A host `fsck --check` of the disk image reports `fsck: clean`. The fsck
#      without the marker could be a volume that was already clean, so on its
#      own it proves nothing about the shutdown path.
# Together they prove the shutdown flushed (the volume is clean), announced
# itself (the marker), and actually cut power (the emulator exited).
#
# It deliberately uses the interactive image (no watchdog): a machine that is
# *supposed* to stay up is what is being tested, and only ACPI S5 may stop it.

set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

image=build/zcos.img
build_hint="tools/build-efi.sh"

if [ ! -f "$image" ]; then
    echo "error: $image not found; build it with: $build_hint" >&2
    exit 1
fi
# Same rule as run-qemu.sh: never test an image older than the sources.
if find boot kernel libs user -name '*.rs' -newer "$image" -print -quit 2>/dev/null | grep -q .; then
    echo "error: $image is older than the sources; rebuild with: $build_hint" >&2
    exit 1
fi

boot_timeout=${ZC_POWEROFF_TIMEOUT:-300}
exit_timeout=${ZC_POWEROFF_EXIT_TIMEOUT:-60}
serial_port=${ZC_POWEROFF_SERIAL_PORT:-4466}

tmp=$(mktemp -d)
cleanup() {
    rm -rf "$tmp"
}
trap cleanup EXIT
# A persistent copy of the full serial log survives the tmp cleanup, so a
# later failure can be diagnosed without re-running the boot.
fulllog="build/poweroff-serial.log"

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

echo "== booting $image, then powering it off through the shell =="
# The guest serial rides a TCP socket, not stdio pipes: stdio-pipe input has
# proven unreliable here (bytes written after the shell prompt never arrive,
# with no echo and no error), while the socket pattern from
# check-live-input.sh works every time.
python3 - "$tmp" "$ovmf_dir/$code_file" "$boot_timeout" "$exit_timeout" "$serial_port" "$fulllog" <<'PY'
import socket
import subprocess
import sys
import threading
import time

tmp, code, boot_timeout, exit_timeout, serial_port, fulllog = (
    sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4]),
    int(sys.argv[5]), sys.argv[6])
t0 = time.monotonic()

qemu = subprocess.Popen(
    ["qemu-system-x86_64",
     "-machine", "q35", "-m", "512M", "-net", "none", "-vga", "std",
     "-no-reboot",
     "-drive", "if=pflash,format=raw,readonly=on,file=%s" % code,
     "-drive", "if=pflash,format=raw,file=%s/VARS.fd" % tmp,
     "-drive", "if=virtio,format=raw,file=build/zcos.img",
     "-drive", "file=build/disk.img,format=raw,if=none,id=vdisk",
     "-device", "virtio-blk-pci,disable-modern=on,drive=vdisk",
     "-display", "none",
     "-serial", "tcp:127.0.0.1:%d,server,nowait" % serial_port],
    stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
out = bytearray()
lock = threading.Lock()
stop = threading.Event()

# The serial socket may take a moment to appear after QEMU starts.
serial = None
start = time.monotonic()
while serial is None and time.monotonic() - start < 30:
    try:
        serial = socket.create_connection(("127.0.0.1", serial_port), timeout=2)
    except OSError:
        time.sleep(0.5)
if serial is None:
    qemu.kill()
    sys.exit("error: guest serial never appeared")
# Blocking mode from here on: the drain thread must never exit on a quiet
# spell, or QEMU blocks writing to a full socket buffer and the whole guest
# wedges wherever it happens to be. That exact failure once disguised itself
# as a boot hang at timer calibration.
serial.settimeout(None)

def drain():
    while not stop.is_set():
        try:
            chunk = serial.recv(4096)
        except OSError:
            return
        if not chunk:
            return
        with lock:
            out.extend(chunk)

reader = threading.Thread(target=drain, daemon=True)
reader.start()

def snapshot():
    with lock:
        return bytes(out)

def fail(message):
    # The tmp dir (and its serial log) is cleaned up on exit, so failures
    # carry the last of the transcript with them instead of going silent.
    tail = snapshot()[-2048:].decode(errors="replace")
    sys.exit("error: %s\n--- serial tail ---\n%s" % (message, tail))

def wait_for(marker, deadline, what):
    start = time.monotonic()
    while time.monotonic() - start < deadline:
        if marker in snapshot():
            print("seen %r at %.1fs" % (marker.decode(), time.monotonic() - t0))
            return
        if qemu.poll() is not None:
            open(fulllog, "wb").write(snapshot())
            fail("QEMU exited %d while waiting for %s"
                 % (qemu.returncode, what))
        time.sleep(0.5)
    open(fulllog, "wb").write(snapshot())
    fail("timed out waiting for %s" % what)

# The shell must be up and the writable volume mounted before the shutdown
# means anything: powering off before `/data` exists would prove nothing
# about the flush path. The shell prints its `> ` prompt and blocks in
# `serial_read` immediately after `shell ready`, so a short settle delay is
# enough — waiting for the prompt text itself is unreliable, because `> `
# also matches log lines like `initd -> blk` long before any shell runs.
wait_for(b"shell ready", boot_timeout, "the shell")
wait_for(b"vfs: mounted zcfs at /data", boot_timeout, "the /data mount")
time.sleep(10)
serial.sendall(b"poweroff\n")
print("sent poweroff at %.1fs" % (time.monotonic() - t0))
open(fulllog, "wb").write(snapshot())

try:
    status = qemu.wait(timeout=exit_timeout)
except subprocess.TimeoutExpired:
    qemu.kill()
    open(fulllog, "wb").write(snapshot())
    fail("QEMU still running %ds after poweroff; S5 never cut power"
         % exit_timeout)
stop.set()
try:
    serial.shutdown(socket.SHUT_RDWR)
except OSError:
    pass
with open("%s/serial.log" % tmp, "wb") as handle:
    handle.write(snapshot())
open(fulllog, "wb").write(snapshot())
if status != 0:
    fail("QEMU exited %d, not 0; poweroff did not cut power cleanly" % status)
if b"power: halt clean" not in snapshot():
    fail("no `power: halt clean` marker; the shutdown was not clean")
print("guest: QEMU exited 0 with `power: halt clean`")
PY

echo "== host fsck of the powered-off disk =="
python3 - "$tmp" <<'PY'
import struct
import subprocess
import sys

tmp = sys.argv[1]
image = open("build/disk.img", "rb").read()
if image[510:512] != b"\x55\xaa":
    sys.exit("error: missing MBR signature at 510")
base = 446 + 2 * 16
if image[base + 4] != 0x7F:
    sys.exit("error: third partition is not zcfs")
lba, sectors = struct.unpack_from("<II", image, base + 8)
with open("%s/zcfs.img" % tmp, "wb") as handle:
    handle.write(image[lba * 512:(lba + sectors) * 512])
done = subprocess.run(
    ["python3", "tools/zcfs.py", "fsck", "%s/zcfs.img" % tmp, "--check"],
    capture_output=True, text=True)
if done.stdout.strip() != "fsck: clean":
    sys.exit("error: host fsck reported %r, not 'fsck: clean'; "
             "the shutdown did not persist a clean volume"
             % done.stdout.strip())
print("host: fsck: clean")
PY

echo
echo "poweroff ok: the machine flushed, announced, and cut power"
