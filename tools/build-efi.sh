#!/usr/bin/env sh
# Build the ZC OS UEFI loader and package it into a bootable FAT image.
#
# Usage: tools/build-efi.sh [--test]
#   --test  build with the qemu-exit feature so tools/run-qemu.sh --test can
#           terminate the emulator with a status code.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

target=x86_64-unknown-uefi
image=build/zcos.img
esp_size_mib=4
features=""

if [ "${1:-}" = "--test" ]; then
    features="--features qemu-exit"
fi

if ! rustup target list --installed | grep -qx "$target"; then
    echo "installing rust target $target"
    rustup target add "$target"
fi

# shellcheck disable=SC2086
cargo build -p zc-uefi-loader --target "$target" --release $features

efi="target/$target/release/zc-uefi-loader.efi"
if [ ! -f "$efi" ]; then
    echo "error: expected $efi" >&2
    exit 1
fi

# Firmware silently refuses an image whose PE subsystem is not EFI application.
if ! objdump -x "$efi" | grep -q 'Subsystem.*0000000a'; then
    echo "error: $efi is not an EFI application (subsystem 0x0a)" >&2
    exit 1
fi

mkdir -p build
rm -f "$image"
dd if=/dev/zero of="$image" bs=1M count="$esp_size_mib" status=none
mkfs.vfat "$image" >/dev/null
mmd -i "$image" ::EFI ::EFI/BOOT
mcopy -i "$image" "$efi" ::EFI/BOOT/BOOTX64.EFI

echo "built $image from $efi"
