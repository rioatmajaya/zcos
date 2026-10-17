# 0036 — A directory can be listed, and `ls` proves it twice

- **Status:** accepted
- **Date:** 2026-10-18
- **Affects:** `libs/zc-abi` (`vfs`, `syscall`, `terminal`), `kernel/zc-kernel`
  (`vfs`, `ramfs`, `tmpfs`, `devfs`, `syscall`), `kernel/zc-kernel-image`,
  `user/zc-user`, `user/zc-shell`, `user/zc-win`, F8h

## Context

Every F8h app starts with the same question — what is in this directory? — and
the system had no answer. `SYS_OPEN`/`SYS_READ` address a path the caller
already knows, and `SYS_STAT` describes one node; nothing enumerated children.
A file manager would have had to embed the initramfs names, which would make it
a second, divergent copy of the filesystem layout and would fail the moment a
file was added.

The kernel already owns the mount table and the filesystems, and the boot proof
already derives the window's screen from the same data the client reads. The
listing had to join that path rather than bypass it: a userspace parser of the
cpio archive would give the client a second source of truth, and the frame
verifier could no longer recompute what the client painted.

## Decision

**`SYS_READDIR` (36) lists a directory by path and index.** The record is a
fixed-size `zc_abi::DirEntry` (kind, name length, 256-byte name), encoded with
the same field-by-field little-endian style as `Stat`, so the kernel validates
one `DIRENT_LEN` buffer and needs no length argument. The caller walks indices
from zero until `READDIR_END`; the index is stable for an unchanged
filesystem.

`FileSystem` gains `read_dir(dir, index, out)` returning `Ok(None)` at the end
and `Err(NotSupported)` when a filesystem cannot enumerate. The distinction is
deliberate: an empty directory and a permission failure must not print the same
thing. `MountTable::read_dir` checks search permission on the directory, lists
mount points directly below it first, then delegates to the filesystem. Mount
points come first because that needs no count of the filesystem's entries — the
table knows its own children, and the filesystem answers the remainder.

`ls` lives in the shared terminal, next to `cat`, with the lister injected the
same way the reader is. The window client answers through `SYS_READDIR`; the
kernel's verifier answers through its own mount table. The scripted session now
ends with `ls`, so `wm: content ok` compares two independently produced
listings pixel for pixel.

## Consequences

- The window-up checksum changes to `0xd616ad79aaefb099` because the scripted
  screen now ends with a listing; the close run keeps `0x6d66e82d0f2d6fb8`
  because it erases the window before the final frame.
- The proof is two-sided and positive: the client logs `client: ls ok` on its
  first real entry, the kernel logs `wm: ls ok` after its own walk, and the
  kernel fails the boot with `window script listed nothing` if its listing was
  empty. Without that check both sides could print `ls: cannot list` and still
  agree.
- `ls /` shows `data/`, `dev/`, and `tmp/` even though the initramfs archive
  contains none of them, because the mount table, not the filesystem, owns
  mount points. A mount-point name shadows a same-named archive entry; that is
  harmless, since resolution reaches the mount either way.
- zcfs cannot list yet: its mount is served by another task, and a listing
  needs a new request in the exchange protocol. `ls /data` reports
  `ls: cannot list` rather than pretending the volume is empty. That is the
  next slice, and it belongs with the file manager that will consume it.
- Listing the cpio archive is a walk per entry, so `ls` is O(n²) in the number
  of files. With a dozen entries that is cheaper than a cursor type or an
  allocation, and the syscall's index is what a cursor would carry anyway.
- The syscall is path-based rather than descriptor-based. `open` on a directory
  still fails, so there is no directory descriptor to hang a cursor on; a
  path-based call also keeps the permission check next to the node it protects.
