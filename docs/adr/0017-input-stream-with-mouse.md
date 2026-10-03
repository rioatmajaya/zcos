# 0017 — Input stream with a PS/2 mouse

- **Status:** Accepted
- **Date:** 2026-10-03
- **Phase:** F8 (see [`../roadmap.md`](../roadmap.md))

## Context

F8b adds the PS/2 mouse next to the proven keyboard and promises "one event
stream to all clients". The mouse shares the 8042 controller with the
keyboard (`0x60`/`0x64`, bytes tagged by the aux bit), QEMU's CI config does
not guarantee a device behind the aux port, and the harness cannot inject
mouse movement — so the real line can only ever be proven synthetically.

## Decision

**The existing kbd domain (task 5) owns both lines.** A second domain would
race the first on the shared controller ports; the same domain claims
`IRQ_KEYBOARD` and `IRQ_MOUSE`, and the port grants stay exactly three.

**Mouse bytes travel tag-framed in the existing byte ring.** A report is five
bytes (`0xFF, 'M', buttons, dx, dy`); `0xFF` can never be a keyboard byte, so
the kernel drain strips frames before they reach the shell and the shell proof
is untouched. Deltas are two's complement with saturation on overflow, and
device-Y is negated to screen-down. The 8042 aux enable runs best-effort at
boot and logs instead of failing; the loopback and self-IPI IRQ tests prove
the path without hardware.

**The kernel stashes the latest mouse state** (`MOUSE_LAST` + a frame counter)
for the future pointer consumer (F8d/F8f). A full evdev
`{time, type, code, value}` ring with `SYN_REPORT` is deferred: there is no
cheap tick source on the input path today, and the compositor consumes nothing
in this milestone.

## Consequences

- One stream serves every client: keyboard ASCII keeps flowing to the shell
  while mouse frames ride the same ring, parsed out by the drain.
- Mouse movement does not move the window in F8b; the deterministic frame and
  its checksum are unchanged.
- `SYS_SURFACE_DESTROY` still does not unmap other tasks (ADR 0015); input
  adds no new cross-task mapping, so nothing changes here.
- USB HID (F8j) and the I2C-HID touchpad stay later milestones; when they land
  they reuse the same frame scheme on the same ring.
