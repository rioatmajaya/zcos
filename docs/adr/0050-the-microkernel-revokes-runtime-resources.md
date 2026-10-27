# 0050 — The microkernel revokes runtime resources at end of life

- **Status:** accepted
- **Date:** 2026-10-27
- **Affects:** `kernel/zc-kernel`, `kernel/zc-kernel-image`, F4

## Context

The audit of the task/service lifecycle found five separate gaps that were
true in the common path but not enforced at the boundary:

1. The task table has ten fixed slots. `spawn` runs only during boot; a
   supervised service is revived in place with `respawn`. The table is not a
   process model, and the parity map asked for either a real lifecycle or a
   documented bounded capacity.
2. Fault and stop paths released IRQ claims, the keyboard ring, and I/O ports,
   but not brokered MMIO regions, pixel surfaces, the window links naming
   them, or the capability copies that grant access to them. A restart could
   therefore reuse a service with a stale region or surface still recorded,
   and repeated restarts could exhaust `MMIO_SLOTS` or `SURFACE_SLOTS`.
3. `SYS_CAP_DELEGATE` ignored a minted capability insert that failed because
   the target table was full. The broker reported success and, for MMIO,
   leaked a region slot nobody could name.
4. `check_user_range` accepted any present page inside the user window. A
   supervisor-only mapping that happened to fall there would have become a
   kernel readable or writable pointer.
5. The filesystem proxy kept a per-task replay log and a global in-flight
   request. A restarted slot could replay answers from its previous life, and
   a dead caller's request could keep the bridge tagged busy forever.

The proof images depend on stable slot identities, so replacing the table
with a dynamic process model is a core-system change, not a microkernel
hardening step.

## Decision

**The ten slots are a documented capacity, and every slot loses its runtime
authority before it can be reused.**

- The bounded table stays for now. Each slot is provisioned at boot with an
  address space, a stack, and its capability grants, and the slot index is a
  stable identity for `service.rs`, window ids, and the boot proofs. Growing
  the capacity is a change to the bring-up layout; a real create/wait/reap
  model is settled in the core-system step, together with service discovery.
- Fault, exit, and service stop all call one revocation path. It releases IRQ
  claims, the keyboard ring, and I/O ports, then removes every MMIO region and
  surface owned by the task, frees the surface frames, clears the window links
  that name those slots, drops every capability copy that names the removed
  objects, resets the task's descriptor table, and forgets its filesystem
  replay log (releasing an in-flight request it still owned). Spawn-time
  grants and the address space stay, because a revived service keeps its
  identity and its provisioned authority.
- A broker delegation fails closed when the target table cannot hold the
  minted capability, and the MMIO slot is rolled back.
- A user buffer is valid only when every covered page is present **and**
  user-accessible.

## Consequences

- Repeated service restarts no longer accumulate regions, surfaces, frames,
  window links, or capability slots. Capacity exhaustion now means real use,
  not a leak.
- The boot proves the path when the device manager exits:
  `revoke: task 6 released 1 mmio region(s), 0 surface(s), 1 capability
  copy/copies`. The service-restart proofs still log `service blk started`
  and `service kbd started`, and the three checksums stay
  `0xd616ad79aaefb099`, `0x6d66e82d0f2d6fb8`, and `0x78ebd6d359914da7`.
- A delegated copy of a surface or MMIO object is revoked kernel-wide when the
  object dies, so a reused slot can never be reached through an old object id.
- A revived service observes a fresh runtime view: no open descriptors, no
  regions, no surfaces, and no filesystem replay log from before the fault. It
  re-brokers or re-creates what it needs, and the restart proof keeps working.
- The proof frames and checksums do not move: the window surfaces still go
  through their existing destroy-and-snapshot path, owner revocation only
  frees what remains, and the user bit is already set on every page the kernel
  maps into a task.
- The fixed table remains a known limit. It is recorded in
  [parity-with-zcos.md](../parity-with-zcos.md), not presented as an app
  catalog decision.
