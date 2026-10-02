# 0005 — One TSS, per-task port bitmap projected on switch

- **Status:** Accepted
- **Date:** 2026-10-01
- **Phase:** F6

## Context

Per-task I/O port authority is required so a driver can hold exactly its own
ports instead of blanket `IOPL`. The natural idea is one TSS per task, each
with its own I/O permission bitmap.

## Decision

There is **one** TSS. The CPU marks a TSS descriptor busy once `LTR` loads it
and refuses to load a busy one, so per-task TSS descriptors are not
expressible. Instead, per-task rights live in a policy table
(`zc-kernel/iomap`), and the single TSS bitmap is *rebuilt from the running
task's entry on every context switch* and on every claim.

## Consequences

- A domain's authority cannot outlive it: on exit its ranges are revoked, and
  the next task to reuse the slot starts from an empty bitmap.
- Rebuilding costs an 8 KiB fill per switch, which is acceptable and, more
  importantly, cannot forget a revoke.
- Claims must also rebuild immediately, because a domain's next instruction is
  usually a port access, not a context switch.
- A reload needs the `0x66` operand-size prefix, which only a named 16-bit
  register in the assembly template produces.
