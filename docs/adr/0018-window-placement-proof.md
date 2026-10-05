# 0018 — Window placement proof for interactive pixels

- **Status:** Accepted
- **Date:** 2026-10-05
- **Phase:** F8 (see [`../roadmap.md`](../roadmap.md))

## Context

F8d-2 proved the window by replaying the kernel's scripted keystroke session
through the shared `zc_abi::terminal::Term` state machine and recomputing the
expected pixels. That proof only works because the boot session is
deterministic. F8d-3b routed real PS/2 input to the window, so the client's
pixels are now whatever the user typed — content the kernel cannot recompute.
The frame checksum must still mean something for that content.

## Decision

**The frame checksum becomes a placement proof for the window region.** The
kernel owns every surface's backing frames (ADR 0015). The compositor releases
the window surface before the boot ends, so the kernel snapshots a 64-bit hash
of the client's pixels on the destroy path, and `verify_framebuffer` compares
the display's window region against that snapshot. This proves the compositor
placed the client's pixels faithfully — no offset, no corruption — for any
content.

**The kernel records which surface is the window.** A surface object delegated
to `WINDOW_CLIENT_TASK` is recorded in the capability-delegate handler, so the
verifier can tell the client's window from the compositor's own back buffer.
This stays correct if more clients appear, unlike scanning by geometry.

**The deterministic boot content is still proven, separately.** The same
snapshot pass compares every surface pixel to `Term` replayed over `SCRIPT` and
logs `wm: content ok`. This is a boot-only check: it holds because the boot
session is scripted, and it is the strong end-to-end guarantee that the client
rendered the kernel's session correctly. Placement is what generalizes.

**The desktop outside the window stays exact.** Non-window pixels are still
recomputed from `zc_abi::desktop::pixel_at`, so damage tracking and the static
layout remain fully proven. The desktop and window region hashes are combined
into the single reported `fb: desktop checksum ok` value.

## Consequences

- The frame checksum no longer claims to know the window's exact pixels; it
  claims the client's pixels reached the screen unchanged. `wm: content ok`
  carries the exact-content claim for the scripted boot.
- The kernel reads a surface's backing frames directly. It relies on the
  existing identity-map-below-4-GiB assumption that `build_fb_tables` already
  depends on; `Surface::pixel_location` is host-tested and fails closed out of
  range.
- Snapshotting a hash, not the pixels, keeps the kernel's memory flat; a
  mismatch reports placement failure without locating the pixel.
- When the terminal later becomes genuinely interactive at boot, the
  `wm: content ok` check is the piece to relax; the placement checksum is
  already the general proof.
