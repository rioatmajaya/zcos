# 0080 — SMP bring-up is parked

- **Status:** accepted
- **Date:** 2026-11-07
- **Affects:** `kernel/zc-kernel-image/src/smp.rs`, F3
- **Supersedes:** none

## Context

The kernel already contains a full SMP bring-up path: `smp::bring_up` copies
the trampoline blob into a sub-megabyte frame, fills its data fields, wakes
each AP with INIT plus two SIPIs, and waits for it to signal online. The
code is correct and compiles, and `ap_main` brings an AP to long mode and
idles with interrupts off.

The environment's firmware, however, triple-faults during `ExitBootServices`
when two CPUs are configured, before any kernel code runs. Enabling `-smp 2`
in `tools/run-qemu.sh` therefore fails the boot test in CI, so the path
cannot be exercised here.

## Decision

**Keep the SMP code in tree but leave `-smp 2` disabled in the proof runs.**

- `smp::bring_up` stays unconditional; on a single CPU it logs
  `smp: single cpu` and returns, so the existing proofs are unaffected.
- The trampoline, AP stack, and INIT/SIPI sequence remain available for a
  machine whose firmware survives the hand-off.
- Re-enabling `-smp 2` is a one-line change in `tools/run-qemu.sh` once the
  firmware issue is resolved.

## Consequences

- No proof exercises more than one CPU today; SMP stays `⚠️ partial` on the
  roadmap.
- The SMP code is not dead code: it compiles and is reviewed, and a
  multi-CPU target can enable it without further changes.
- Per-CPU stacks, timers, and an SMP-aware scheduler remain future work once
  bring-up is provable.

## References

- [docs/roadmap.md](../roadmap.md) (F3)
- [docs/blocked/smp-ovmf.md](../blocked/smp-ovmf.md)
