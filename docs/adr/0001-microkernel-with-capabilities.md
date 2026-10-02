# 0001 — Capability-based microkernel

- **Status:** Accepted
- **Date:** 2026-10-01
- **Phase:** F0, F4, F6

## Context

The kernel must stay small enough to audit, and a failed driver must not take
the machine down. Monolithic kernels put every driver in ring 0, where one bug
is fatal.

## Decision

The kernel keeps only privileged mechanisms — address spaces, threads,
interrupt delivery, scheduling, IPC, and capability validation. Policy,
filesystems, networking, the compositor, and drivers run in separate userspace
domains. Kernel objects are referenced through unforgeable capabilities that
convey explicit rights and transfer only over IPC, with a non-amplifying subset
check on delegation.

## Consequences

- A ring-3 fault kills one domain; the kernel and other domains continue, and
  a faulted domain can be restarted with a bounded budget.
- Authority is explicit: a driver holds exactly the ports and IRQs it was
  granted and claims them at runtime.
- Costs: IPC is on the hot path, so the fabric must be efficient; there is no
  single global lock to lean on for sharing state.
