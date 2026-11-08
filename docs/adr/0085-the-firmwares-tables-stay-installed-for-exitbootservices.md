# 0085 — The firmware's tables stay installed for ExitBootServices

- **Status:** accepted
- **Date:** 2026-11-08
- **Affects:** `boot/uefi-loader`, F1
- **Supersedes:** none

## Context

The loader installed its own minimal GDT — null, one code segment, one data
segment — *before* calling `ExitBootServices`, masking interrupts first so no
firmware interrupt could arrive between `lgdt` and the CS reload. The boot
still died intermittently: QEMU's `-no-reboot` turned a rare early death into
exit status 0, and CI failed on it for a day before it reproduced locally
once in nine boots.

The interrupt log caught the whole story: a hardware interrupt (the
firmware's timer, vector 0x20) fired *inside* `ExitBootServices` with
interrupts **enabled** — the firmware re-enables them during the call —
while the loader's three-entry GDT was loaded. The still-installed firmware
IDT names the firmware's own code selector in every gate; that selector does
not exist in the loader's table, so delivering the interrupt faulted, the
#GP nested into #DF, and the machine reset before the kernel ever ran.

## Decision

**The firmware's GDT and IDT stay the installed pair until
`ExitBootServices` returns; the loader masks interrupts again the moment it
does, and only then installs its own GDT.**

- Firmware code that runs during the call — event notifications, the timer
  that fires mid-call — is serviced through the firmware's own tables, which
  agree with each other. The interrupt is handled and the call completes.
- After the call returns, no firmware code runs again, so the table swap
  needs no protection beyond the immediate `cli`: the reload happens with
  interrupts masked and the very next thing the machine does is jump to the
  kernel.

## Consequences

- The early-boot death is gone at the source. Forty-four consecutive boots
  after the fix showed zero early deaths, against four in twenty-six before
  it on the same machine.
- A real double-fault after `ExitBootServices` would now surface after the
  call instead of during it — but nothing executes between the masked
  reload and the kernel jump except the boot-success line and the switch.
- The CI greps are unchanged; the fix moves no transcript line. The locked
  frame checksums are untouched.

## References

- [ADR 0002](0002-uefi-x86-64-first-platform.md)
- [`docs/blocked/smp-ovmf.md`](../blocked/smp-ovmf.md) — a different
  `ExitBootServices` failure, parked separately
