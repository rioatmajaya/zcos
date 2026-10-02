# 0002 — x86_64 + UEFI as the first platform

- **Status:** Accepted
- **Date:** 2026-10-01
- **Phase:** F1

## Context

A first platform has to be debuggable and testable without owning a lab. We
need a firmware interface that can hand over the framebuffer, memory map, and
ACPI tables, and an emulator that behaves like real firmware.

## Decision

The first platform is **x86_64 booted through UEFI**, with QEMU + OVMF as the
required development and test target. We write our own loader; we do not use
GRUB, systemd-boot, or a Linux payload. Physical hardware is introduced only
after the QEMU integration tests are reliable.

## Consequences

- QEMU/OVMF gives a scriptable, headless boot test in CI.
- We own the boot path, so bugs are ours to fix — but we control the boot
  protocol and the `BootInfo` contract.
- x86_64-specific code lives behind clear seams so other architectures remain
  possible later.
- Firmware quirks are real: OVMF's SMP triple-fault is documented as a blocker
  rather than worked around blindly.
