# Blocked — SMP bring-up under OVMF

- **Parked:** 2026-10-01
- **Phase:** F3
- **Owner:** Rio Atmajaya
- **Branch:** dormant in `main` (code present, disabled)
- **Time spent:** under the two-week threshold

## Symptom

With `-smp 2`, this environment's OVMF deterministically triple-faults in real
mode at `8700:0035` during `ExitBootServices`, **before any kernel code runs**.
OVMF with a single CPU boots the same image fine. The failure is therefore in
firmware, not in our loader or kernel.

## What we tried

- INIT-SIPI-SIPI bring-up, a sub-megabyte trampoline with host-verified bytes,
  and per-AP stacks — all implemented and unit-tested on the host.
- Booting with `-smp 2`: triple fault in OVMF at the same real-mode address
  every time.
- Booting with `-smp 1`: clean boot to the kernel idle state.

## Current best guess

An OVMF/firmware bug or an incompatibility with the QEMU machine type in this
environment, triggered when more than one CPU is present at
`ExitBootServices`. Our code never executes, so it cannot be the cause.

## What unblocks it

A firmware or QEMU version where `-smp 2` survives `ExitBootServices`, or a
documented OVMF workaround. Then re-enable `-smp 2` in `tools/run-qemu.sh` and
add the per-AP heartbeat to the F3 pass criteria.

## Impact

SMP is not a dependency of F4–F6: the scheduler is already preemptive on one
CPU, and IPC, drivers, and fault isolation are proven single-CPU. F3 stays
`⚠️ partial` until this clears; everything from F4 on can proceed.
