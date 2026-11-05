# 0070 — A task is spawned by name and waited by slot

- **Status:** accepted
- **Date:** 2026-11-05
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, F7k
- **Supersedes:** none

## Context

The registry already numbered incarnations and recorded an exit status, but
nothing could create a task after boot or consume that record. `initd` could
revive a supervised slot, not start arbitrary work, and the shell had no way to
run an image. POSIX `fork` would duplicate an address space the table never
shares, and `exec` alone cannot place the child in a slot the caller controls.
The reference implementation's dynamic spawn is a `procmgr`/`loaderd` service;
the public kernel needs the mechanism underneath such a manager, not a copy of
it.

## Decision

**A spawn names an image; a wait names a slot and reaps it.**

- `SYS_SPAWN` (51) takes the name of an initramfs image and builds the child
  with the boot's own address-space builder and ELF loader, one fresh stack
  frame, and the lowest empty task slot. The child inherits the caller's
  identity and session, and nothing else: its capability table and descriptor
  table start empty. Window clients are refused because their slot, compositor
  link, surface, and input session belong to the boot layout. A missing or
  unmappable image, a refused name, or a full table returns `u64::MAX`; a
  successful spawn returns the slot index.
- `SYS_WAIT` (52) blocks while the slot is alive, using the same
  block-and-replay path as a full send or empty receive. Any exit or fault
  unblocks every task, so the replay either blocks again or observes the dead
  slot. It then returns the status the slot recorded and removes the registry
  record — the reap — which frees the index for a later spawn. Waiting on the
  caller's own slot or on a supervised service slot is refused: `initd` owns
  service lifecycles through the service syscalls and fault events.
- Both calls need `WRITE` on one lifecycle capability (`spawn_cap`, its own
  bit-28 namespace). Creating work and accounting for its end is one trust
  decision, not two. The shell receives the grant at spawn.
- `TaskTable` keeps a per-slot `next_generation` counter. Spawn and revive both
  take the counter's value and advance it, so an incarnation number never
  repeats even when a reap returns the slot to the free set.
- `map_elf` now returns a typed error instead of halting, because a syscall
  must refuse a bad image rather than stop the machine; the boot maps the same
  errors back to its `fail` diagnostics.

## Consequences

- The boot script reaps the bring-up producer and consumer, spawns fresh
  `producer.elf` and `consumer.elf` into slots 0 and 1, proves the pair runs
  over the shared endpoint, waits for both, and reaps them again. Because the
  lone producer blocks on the full endpoint, `ps` deterministically shows slot
  0 at generation 2 before the consumer is spawned.
- `fork`, `exec`, signals, and image lookup beyond the boot initramfs remain
  absent; the table is still the bounded 10 slots. A spawn that fails after
  allocating its address space and stack leaks those frames until reboot, and
  a reap frees the slot record but not the child's address space or stack
  either. Concurrency is bounded by the table, but cumulative frame use is not;
  reclaiming a dead task's frames needs a teardown path in the address-space
  builder, which this increment does not add.
- This is parity of capability, not of implementation or ABI: no reference
  source, ABI, or pixel output is copied.
