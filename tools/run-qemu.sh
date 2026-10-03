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
-drive if=virtio,format=raw,file=$image
-drive file=$root/build/disk.img,format=raw,if=none,id=vdisk
-device virtio-blk-pci,disable-modern=on,drive=vdisk"

if [ "${1:-}" = "--test" ]; then
    # isa-debug-exit turns the loader's port write into a process exit code;
    # writing 0x10 makes QEMU exit with (0x10 << 1) | 1 = 33.
    #
    # The shell transcript is scripted from tools/boot-script.txt so the
    # interactive shell is exercised deterministically; the final `exit`
    # ends the shell and the kernel check below ends the run.
    #
    # tools/boot-feed.py launches QEMU with its serial on stdio, copies the
    # guest's output to stdout (which CI captures), and paces the commands so
    # the shell reads them intact. See the helper's module docstring for why
    # the feed is shaped the way it is; a whole-script burst overruns the
    # guest UART FIFO and the kernel's input ring and tears lines.
    #
    # The watchdog inside the helper bounds the run so a guest that never
    # reaches the exit write (e.g. a wedged shell) fails the test instead of
    # hanging CI. The `||` keeps `set -e` from aborting on the expected
    # non-zero status.
    #
    # The scripted shell writes `/data/probe`, which is the same file the host
    # planted as the boot probe. That makes the disk a mutable fixture: a
    # second boot against the previous run's disk would find the host pattern
    # overwritten and the block driver's probe would fail. Rebuild the data
    # disk up front so every run starts from the same image and the test is
    # repeatable (CI's `build-efi.sh` already did this, so it is a no-op
    # there).
    ./tools/mkdisk.sh >/dev/null
    status=0
    # shellcheck disable=SC2086
    python3 "$root/tools/boot-feed.py" \
        --script "$root/tools/boot-script.txt" \
        -- qemu-system-x86_64 $common \
        -display none -serial stdio -device isa-debug-exit || status=$?
    if [ "$status" -eq 33 ]; then
        exit 0
    fi
    echo "boot test failed with emulator status $status" >&2
    exit 1
fi

# shellcheck disable=SC2086
exec qemu-system-x86_64 $common -serial stdio
