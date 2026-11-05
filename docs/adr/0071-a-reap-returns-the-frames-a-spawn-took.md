# 0071 — A reap returns the frames a spawn took

- **Status:** accepted
- **Date:** 2026-11-05
- **Affects:** `kernel/zc-kernel`, `kernel/zc-kernel-image`, F7k
- **Supersedes:** none

## Context

[ADR 0070](0070-a-task-is-spawned-by-name-and-waited-by-slot.md) gave the
process model spawn and wait, but a reap only dropped the slot record. A
spawned task's four private page-table frames, its image pages, and its stack
page stayed allocated until reboot. Walking the dead task's page tables to free
them would also meet tables and frames the task only borrowed — the copied
kernel levels, the shared framebuffer tables, the input ring, and runtime
surface mappings — so a generic page-table walk is the wrong shape.

## Decision

**Record the frames a spawn allocates, and return exactly that record on reap.**

- `FrameLedger<N>` in `zc_kernel::memory` is a bounded, fixed-size record of
  frames. `push` refuses overflow instead of growing. `release` returns each
  frame to the `FrameAllocator` and keeps whatever the bounded recycle stack
  refuses, so a caller can retry rather than silently dropping a frame.
- `new_address_space_logged` records the four private table levels, `map_elf`
  records each image page as it allocates it, and the spawn records its one
  stack frame. The boot wrapper uses the same logged builder but ignores the
  ledger, because boot frames are never returned while the machine runs.
- A spawn failure releases everything it recorded before returning
  `u64::MAX`, so a missing stack frame, a missing page-table frame, or an
  unmappable image no longer leaks the frames taken on the way to the refusal.
- `SYS_WAIT` reaps the slot record and then releases the slot's ledger. A slot
  that was never spawned keeps an empty ledger, so waiting on a boot task
  reclaims zero frames: the kernel never walks a boot task's page tables.
- The allocator's recycle stack grows from 64 to 192 frames. Two worst-case
  spawn reaps (69 frames each) must fit without an allocation in between, with
  room for frames a surface teardown already parked.

## Consequences

- The boot script proves both sides. The first waits on the bring-up pair
  print no reclaim line, because boot tasks own no spawn ledger. The waits on
  the spawned producer and consumer print `reclaimed 6 frames` each: four
  table levels, one image page, and one stack page.
- The ledger is per slot, not inside `Task`; `zc-kernel` stays free of a
  spawn-layout policy and the process table remains a pure scheduler record.
- A reap does not reclaim runtime mappings a spawned task acquired after
  start — surfaces, the input ring, a DMA window. Those remain the revocation
  path's job, which already runs at exit; their frame ownership is unchanged.
- Ledger capacity equals the ELF loader's page cap plus five frames. An image
  larger than 64 pages is refused by the loader before the ledger could
  overflow.
- This is parity of capability, not of implementation or ABI: no reference
  source, ABI, or pixel output is copied.
