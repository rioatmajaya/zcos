# 0007 — Read-only VFS with a trait-based filesystem

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** F7 (see [`../roadmap.md`](../roadmap.md))

## Context

Files were reached through `zc-kernel::fs`, a single parser hard-wired to the
cpio initramfs: a flat name table with no directories, no mount points, and no
way to add ext2 or a device filesystem without editing that module. The block
domain already parses FAT32 and ext2 in userspace, so the kernel needs a shape
that can accept those filesystems later without a rewrite.

## Decision

Introduce `zc-kernel::vfs`, a read-only VFS core: a `MountTable` mapping
normalised path prefixes to filesystems, a `DescriptorTable` holding open-file
descriptions, and an object-safe `FileSystem` trait whose methods all take
`&self` — `name`, `root`, `lookup`, `stat`, `read`, and `write`, the last
defaulting to `NotSupported`. Mounts store `&'static dyn FileSystem`, the
codebase's first trait object, chosen over an enum so a new filesystem is a new
type rather than a new match arm in every method. Descriptors carry the read
offset, not the filesystem, so a mount can be shared by every task. The
initramfs becomes one adapter, `zc-kernel::ramfs`, over a fixed node table.

## Consequences

- ext2, FAT32, `devfs`, and `tmpfs` become new `FileSystem` implementations;
  the VFS and the syscall layer do not change.
- Read-only is the default: `write` fails until a filesystem overrides it, so
  F7e can add one without reopening this decision.
- Mounts are shared, so a filesystem adapter must hold its state behind
  interior mutability (or an immutable view) rather than `&mut self`.
- Trait objects cost a vtable indirection per call, which is irrelevant next to
  a block read, and rule out `const` filesystem construction.
- The ABI adds `SYS_STAT`; `SYS_WRITE` waits for F7e so no syscall ships that
  can only fail.
