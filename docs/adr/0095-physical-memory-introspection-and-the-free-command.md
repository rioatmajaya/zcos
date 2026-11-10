# 0095 — Physical memory introspection and the free command

- **Status:** accepted
- **Date:** 2026-11-11
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F4
- **Supersedes:** none

## Context

The kernel's physical frame allocator (`FrameAllocator`) manages usable RAM
handed over by the UEFI bootloader through `BootInfo` memory regions. It
allocates frames monotonically and recycles freed frames through a bounded LIFO
stack.

However, neither the kernel nor userspace had a mechanism to query memory
statistics. The total usable bytes were computed at boot, but the allocator did
not maintain an active allocation counter, and no system call existed to query
memory usage. As a result, userspace tools (such as system monitors, task
managers, and the shell) could not inspect physical memory consumption.

## Decision

**Expose physical memory statistics through `SYS_MEM_INFO` and add the `free`
command to the shell.**

1. **Allocator Accounting:**
   `FrameAllocator` tracks `allocated_frames: u64`. Each successful frame
   allocation (`allocate` or `allocate_contiguous`) increments the counter, and
   each returned frame (`free`) decrements it. The allocator exposes:
   - `total_usable_bytes(&self) -> u64`: sum of loader-reported usable regions.
   - `allocated_frames(&self) -> u64`: count of currently allocated physical frames.
   - `used_bytes(&self) -> u64`: `allocated_frames * PAGE_SIZE`.
   - `free_bytes(&self) -> u64`: `total_usable_bytes - used_bytes`.

2. **System Call Contract:**
   Add `SYS_MEM_INFO` (62):
   - Arguments: `rdi` points to a userspace buffer of length `rsi`, which must
     equal `MEM_INFO_LEN` (32 bytes).
   - Returns: `0` on success, or `u64::MAX` on error (invalid buffer pointer,
     unmapped memory, or incorrect length).
   - Calling requires no special privilege: physical memory statistics are
     non-sensitive system diagnostics.

3. **Wire Format (`MemInfo`):**
   `MemInfo` is a 32-byte `#[repr(C)]` little-endian structure:
   - `total_bytes: u64`: Total usable RAM.
   - `used_bytes: u64`: Allocated RAM.
   - `free_bytes: u64`: Unallocated RAM.
   - `page_size: u64`: Base page size (4096).

4. **Shell Inspection:**
   The shell gains `free` (with alias `mem`), which calls `mem_info` and prints:
   `mem: total <N> KB used <N> KB free <N> KB`.

## Consequences

- Userspace can inspect system-wide physical memory usage dynamically.
- `FrameAllocator` maintains exact accounting across allocations and frees
  (including teardowns, reaps, and surface destructions).
- The three locked proof frame checksums remain byte-identical: memory
  introspection affects only the serial transcript and shell builtins.

## References

- [ADR 0050](0050-the-microkernel-revokes-runtime-resources.md)
- [ADR 0094](0094-system-and-device-inspection-in-the-shell.md)
- [docs/specs/syscall-abi.md](../specs/syscall-abi.md)
