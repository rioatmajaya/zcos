#!/usr/bin/env sh
# Build the ZC OS UEFI loader and kernel, and package them into a bootable
# FAT image that QEMU/OVMF can start.
#
# Usage: tools/build-efi.sh [--test | --test-close]
#   --test         build with the qemu-exit feature so tools/run-qemu.sh --test
#                  can terminate the emulator with a status code once the kernel
#                  reaches its idle state. The scripted mouse session stops with
#                  the window on screen.
#   --test-close   as above, and additionally build the kernel with close-proof so
#                  the scripted session also clicks close. That proves the
#                  window's session-ending protocol end to end, at the cost of a
#                  final frame with no window in it — which is why it is a
#                  separate image and a separate CI run, and why the default is
#                  the one a person boots (ADR 0026).
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

uefi_target=x86_64-unknown-uefi
none_target=x86_64-unknown-none
esp_size_mib=4
# `qemu-exit` belongs to both binaries; `close-proof` only to the kernel, which is
# the only one with the scripted mouse session.
loader_features=""
kernel_features=""
image=build/zcos.img

case "${1:-}" in
    --test)
        loader_features="--features qemu-exit"
        kernel_features="--features qemu-exit"
        ;;
    --test-close)
        loader_features="--features qemu-exit"
        kernel_features="--features qemu-exit,close-proof"
        # Its own image, so this CI-only variant can never overwrite the one a
        # person boots. Sharing a path meant the last build won: a CI close run
        # left `build/zcos.img` holding the close-proof kernel, and the next
        # `tools/run-qemu.sh` booted a desktop that destroys its own window.
        image=build/zcos-close.img
        ;;
    "")
        ;;
    *)
        echo "usage: $0 [--test|--test-close]" >&2
        exit 2
        ;;
esac

if ! rustup target list --installed | grep -qx "$uefi_target"; then
    echo "installing rust target $uefi_target"
    rustup target add "$uefi_target"
fi
if ! rustup target list --installed | grep -qx "$none_target"; then
    echo "installing rust target $none_target"
    rustup target add "$none_target"
fi

# shellcheck disable=SC2086
cargo build -p zc-uefi-loader --target "$uefi_target" --release $loader_features

# shellcheck disable=SC2086
cargo build --manifest-path kernel/zc-kernel-image/Cargo.toml \
    --target "$none_target" --target-dir target --release $kernel_features

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
cargo build --manifest-path user/zcompositor/Cargo.toml \
    --target "$none_target" --target-dir target --release
cargo build --manifest-path user/zc-blk/Cargo.toml \
    --target "$none_target" --target-dir target --release
cargo build --manifest-path user/zc-kbd/Cargo.toml \
    --target "$none_target" --target-dir target --release
cargo build --manifest-path user/zc-devmgr/Cargo.toml \
    --target "$none_target" --target-dir target --release
cargo build --manifest-path user/zc-initd/Cargo.toml \
    --target "$none_target" --target-dir target --release
cargo build --manifest-path user/zc-win/Cargo.toml \
    --target "$none_target" --target-dir target --release
./tools/mkinitramfs.sh \
    "target/$none_target/release/producer:producer.elf" \
    "target/$none_target/release/consumer:consumer.elf" \
    "target/$none_target/release/shell:shell.elf" \
    "target/$none_target/release/compositor:compositor.elf" \
    "target/$none_target/release/blk:blk.elf" \
    "target/$none_target/release/kbd:kbd.elf" \
    "target/$none_target/release/devmgr:devmgr.elf" \
    "target/$none_target/release/initd:initd.elf" \
    "target/$none_target/release/win:win.elf"
./tools/mkdisk.sh
dd if=/dev/zero of="$image" bs=1M count="$esp_size_mib" status=none
mkfs.vfat "$image" >/dev/null
mmd -i "$image" ::EFI ::EFI/BOOT
mcopy -i "$image" "$efi" ::EFI/BOOT/BOOTX64.EFI
mcopy -i "$image" "$kernel" ::EFI/BOOT/KERNEL.ELF
mcopy -i "$image" build/initramfs.cpio ::EFI/BOOT/INITRAMFS.CPIO

echo "built $image (loader + kernel)"
