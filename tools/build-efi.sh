#!/usr/bin/env sh
# Build the ZC OS UEFI loader and kernel, and package them into a bootable
# FAT image that QEMU/OVMF can start.
#
# Usage: tools/build-efi.sh [--test]
#   --test  build both the loader and the kernel with the qemu-exit feature so
#           tools/run-qemu.sh --test can terminate the emulator with a status
#           code once the kernel reaches its idle state.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

uefi_target=x86_64-unknown-uefi
none_target=x86_64-unknown-none
image=build/zcos.img
esp_size_mib=4
features=""

if [ "${1:-}" = "--test" ]; then
    features="--features qemu-exit"
fi

if ! rustup target list --installed | grep -qx "$uefi_target"; then
    echo "installing rust target $uefi_target"
    rustup target add "$uefi_target"
fi
if ! rustup target list --installed | grep -qx "$none_target"; then
    echo "installing rust target $none_target"
    rustup target add "$none_target"
fi

# shellcheck disable=SC2086
cargo build -p zc-uefi-loader --target "$uefi_target" --release $features

# shellcheck disable=SC2086
cargo build --manifest-path kernel/zc-kernel-image/Cargo.toml \
    --target "$none_target" --target-dir target --release $features

efi="target/$uefi_target/release/zc-uefi-loader.efi"
kernel="target/$none_target/release/zc-kernel"
if [ ! -f "$efi" ]; then
    echo "error: expected $efi" >&2
    exit 1
fi
if [ ! -f "$kernel" ]; then
    echo "error: expected $kernel" >&2
    exit 1
fi

# Firmware silently refuses an image whose PE subsystem is not EFI application.
if ! objdump -x "$efi" | grep -q 'Subsystem.*0000000a'; then
    echo "error: $efi is not an EFI application (subsystem 0x0a)" >&2
    exit 1
fi

mkdir -p build
rm -f "$image"

# Userspace tasks are freestanding ELFs packed into the initramfs.
cargo build --manifest-path user/zc-producer/Cargo.toml \
    --target "$none_target" --target-dir target --release
cargo build --manifest-path user/zc-consumer/Cargo.toml \
    --target "$none_target" --target-dir target --release
cargo build --manifest-path user/zc-shell/Cargo.toml \
    --target "$none_target" --target-dir target --release
cargo build --manifest-path user/zc-fb/Cargo.toml \
    --target "$none_target" --target-dir target --release
cargo build --manifest-path user/zc-blk/Cargo.toml \
    --target "$none_target" --target-dir target --release
cargo build --manifest-path user/zc-kbd/Cargo.toml \
    --target "$none_target" --target-dir target --release
cargo build --manifest-path user/zc-devmgr/Cargo.toml \
    --target "$none_target" --target-dir target --release
./tools/mkinitramfs.sh \
    "target/$none_target/release/producer:producer.elf" \
    "target/$none_target/release/consumer:consumer.elf" \
    "target/$none_target/release/shell:shell.elf" \
    "target/$none_target/release/fb:fb.elf" \
    "target/$none_target/release/blk:blk.elf" \
    "target/$none_target/release/kbd:kbd.elf" \
    "target/$none_target/release/devmgr:devmgr.elf"
./tools/mkdisk.sh
dd if=/dev/zero of="$image" bs=1M count="$esp_size_mib" status=none
mkfs.vfat "$image" >/dev/null
mmd -i "$image" ::EFI ::EFI/BOOT
mcopy -i "$image" "$efi" ::EFI/BOOT/BOOTX64.EFI
mcopy -i "$image" "$kernel" ::EFI/BOOT/KERNEL.ELF
mcopy -i "$image" build/initramfs.cpio ::EFI/BOOT/INITRAMFS.CPIO

echo "built $image (loader + kernel)"
