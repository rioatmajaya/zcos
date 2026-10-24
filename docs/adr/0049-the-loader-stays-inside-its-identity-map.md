# 0049 — The loader stays inside its identity map

- **Status:** accepted
- **Date:** 2026-10-24
- **Affects:** `boot/uefi-loader`, `libs/zc-abi`, `docs/specs/boot-info.md`, F1

## Context

The kernel starts on the loader's page tables, which identity-map only
`0..4 GiB`. The loader used `AllocateAnyPages` and `AllocatePool`, so firmware
was free to return memory above that limit. `BootInfo`, the converted memory
map, the initramfs, the kernel stack, and the initial page tables are all
dereferenced by the kernel through the identity map. On a machine with enough
RAM, a high allocation is a boot fault rather than a theoretical concern.

Two smaller hand-off bugs sat next to it. `ExitBootServices` can reject a stale
map key and require a fresh `GetMemoryMap`; the code refreshed the map but left
`BootInfo::memory_map_len` at the older value. And `read_all` silently truncated
a kernel or initramfs larger than its fixed buffer, which could turn a bad image
into a confusing parse failure or a desktop with missing tasks.

## Decision

**Every loader-owned physical range the kernel reads stays below the identity
limit, and the boot info describes the final memory map.**

- Page allocations use `AllocateMaxAddress` with the identity limit as the
  upper bound; the loader also checks the returned range.
- `BootInfo` and the memory-map buffers are page-backed below that limit, not
  pool allocations whose address the loader cannot constrain.
- After `ExitBootServices` succeeds, the loader writes the region count from the
  final memory-map refresh back into `BootInfo`.
- A kernel image or initramfs that does not fit its buffer is a fatal error, not
  a truncated hand-off.
- The framebuffer and RSDP are handed over only when they lie inside the
  identity map; a malformed framebuffer geometry is reported as unavailable.
- `BootInfo` keeps its layout and protocol version. The proof images and their
  frame checksums are unaffected.

## Consequences

- A high-memory firmware allocation can no longer produce a kernel pointer the
  kernel cannot dereference.
- The loader fails before the hand-off when an image is too large or the final
  map cannot fit its buffers.
- The loader now has one physical-memory rule, documented in the boot ABI, so a
  future allocation helper cannot quietly reintroduce the same bug.
- This is still a 4 GiB identity map. Supporting RAM above 4 GiB needs a larger
  initial map or a kernel that switches tables before touching high memory; that
  is a separate change.
