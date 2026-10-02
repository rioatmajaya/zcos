#!/usr/bin/env sh
# Validate the local tools needed to develop and test ZC OS.
set -eu

missing=0

require_command() {
    if command -v "$1" >/dev/null 2>&1; then
        printf 'found: %s (%s)\n' "$1" "$(command -v "$1")"
    else
        printf 'missing: %s\n' "$1" >&2
        missing=1
    fi
}

require_command cargo
require_command rustup
require_command qemu-system-x86_64
require_command objdump
require_command mkfs.vfat
require_command mcopy
require_command mmd
require_command mdir
require_command mtype
require_command mke2fs
require_command debugfs

if command -v rustup >/dev/null 2>&1; then
    if rustup component list --toolchain 1.95.0 --installed | grep -qx 'rust-src'; then
        printf 'found: rust-src component\n'
    else
        printf 'missing: rust-src component (install with: rustup component add rust-src)\n' >&2
        missing=1
    fi

    if rustup target list --installed | grep -qx 'x86_64-unknown-uefi'; then
        printf 'found: x86_64-unknown-uefi target\n'
    else
        printf 'missing: x86_64-unknown-uefi target (install with: rustup target add x86_64-unknown-uefi)\n' >&2
        missing=1
    fi

    if rustup target list --installed | grep -qx 'x86_64-unknown-none'; then
        printf 'found: x86_64-unknown-none target\n'
    else
        printf 'missing: x86_64-unknown-none target (install with: rustup target add x86_64-unknown-none)\n' >&2
        missing=1
    fi
fi

if [ -d /usr/share/OVMF ] || [ -d /usr/share/ovmf ]; then
    printf 'found: OVMF firmware directory\n'
else
    printf 'missing: OVMF firmware (install your distribution package, often ovmf)\n' >&2
    missing=1
fi

exit "$missing"
