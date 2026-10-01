#!/usr/bin/env sh
# Boot the ZC OS loader image under QEMU with OVMF.
#
# Usage: tools/run-qemu.sh [--test]
#   --test  run headless with isa-debug-exit and return the emulator's status,
#           for automated boot checks. Requires an image built with
#           tools/build-efi.sh --test.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

image=build/zcos.img
if [ ! -f "$image" ]; then
    echo "error: $image not found; run tools/build-efi.sh first" >&2
    exit 1
fi

# Locate a 4 MB OVMF code/variables pair.
code=""
vars_src=""
for pair in \
    "/usr/share/OVMF/OVMF_CODE_4M.fd:/usr/share/OVMF/OVMF_VARS_4M.fd" \
    "/usr/share/OVMF/OVMF_CODE.fd:/usr/share/OVMF/OVMF_VARS.fd" \
    "/usr/share/ovmf/OVMF_CODE.fd:/usr/share/ovmf/OVMF_VARS.fd"
do
    candidate_code=${pair%%:*}
    candidate_vars=${pair##*:}
    if [ -f "$candidate_code" ] && [ -f "$candidate_vars" ]; then
        code=$candidate_code
        vars_src=$candidate_vars
        break
    fi
done
if [ -z "$code" ]; then
    echo "error: no OVMF firmware found (install the ovmf package)" >&2
    exit 1
fi

# Firmware state must start clean on every run.
vars=build/OVMF_VARS.fd
cp "$vars_src" "$vars"

common="-machine q35 -m 512M -net none -vga std -no-reboot
-drive if=pflash,format=raw,readonly=on,file=$code
-drive if=pflash,format=raw,file=$vars
-drive if=virtio,format=raw,file=$image"

if [ "${1:-}" = "--test" ]; then
    # isa-debug-exit turns the loader's port write into a process exit code;
    # writing 0x10 makes QEMU exit with (0x10 << 1) | 1 = 33.
    # The shell transcript is scripted from tools/boot-script.txt so the
    # interactive shell is exercised deterministically; the final `exit`
    # ends the shell and the kernel check below ends the run. Firmware
    # eats early stdin during its own boot, so a slow drip keeps full
    # script copies arriving through the whole kernel phase; the shell
    # consumes one pass and ignores the rest after it exits. A FIFO feeds
    # QEMU so the drip dies with the emulator instead of hanging CI.
    # (QEMU monitor key injection does not deliver in this environment, so
    # the PS/2 IRQ path is proven by a self-IPI plus a translator loopback
    # instead; real keystrokes share the same ring when they arrive.)
    # The `||` keeps `set -e` from aborting on the expected non-zero status.
    status=0
    feed="$root/build/boot-feed.fifo"
    rm -f "$feed"
    mkfifo "$feed"
    # shellcheck disable=SC2086
    ( while :; do cat "$root/tools/boot-script.txt"; sleep 1; done > "$feed" ) &
    feeder=$!
    qemu-system-x86_64 $common \
        -display none -serial stdio -device isa-debug-exit < "$feed" || status=$?
    kill "$feeder" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -f "$feed"
    if [ "$status" -eq 33 ]; then
        exit 0
    fi
    echo "boot test failed with emulator status $status" >&2
    exit 1
fi

# shellcheck disable=SC2086
exec qemu-system-x86_64 $common -serial stdio
