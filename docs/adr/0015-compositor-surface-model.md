# 0015 — Compositor surface model and damage tracking

- **Status:** Accepted
- **Date:** 2026-10-03
- **Phase:** F8 (see [`../roadmap.md`](../roadmap.md))

## Context

F8 starts the graphical desktop. The first milestone needs a userspace
compositor that owns the display, a way for clients to hand it pixels without
sharing a pointer, and a repaint strategy that does not redraw the whole screen
every frame. The design skill (`19-compositor-dan-input.md`) fixes the build
order — framebuffer, then double buffer, then damage tracking, then input, then
windows — and leaves two decisions to the project: the application model
(immediate vs retained) and the shell policy.

The kernel already maps one framebuffer into every task and has no shared
memory syscall (`SYS_MAP_FRAME` is a stub). A task is `no_std` with no
allocator and a single 4 KiB stack page, so a screen-sized buffer cannot live
in the task image: the kernel has to provide it.

## Decision

**Surfaces are kernel-owned objects handed out by capability.**
`SYS_SURFACE_CREATE` mints a surface and its backing frames for a task holding
the factory capability; `SYS_SURFACE_MAP` maps it into a caller that holds a
read capability for that surface; `SYS_SURFACE_DESTROY` returns the frames and
is restricted to the creator. A client delegates a read-only capability to the
compositor with the existing `SYS_CAP_DELEGATE`, so the compositor can map only
the surfaces it was explicitly granted. Surface capabilities set bit 29, beside
port caps (bit 31) and service caps (bit 30), so the namespaces never collide.

**The compositor keeps a persistent back buffer and presents by blit.** It
creates one full-screen surface, paints the desktop into it, and copies pixels
to the display framebuffer. It does not swap buffers: with no GPU driver there
is no page flip to synchronize, and a persistent back buffer always holds the
current frame, so the "back buffer contains frame N−1" trap cannot occur. When
a real flip path arrives, the `flip_done` rule and the copy of untouched
regions from the previous front buffer become mandatory.

**Damage tracking is mandatory and shared.** The compositor tracks a bounded
list of rectangles that merge when they touch and collapse to their bounding
box when the list is full, so tracking degrades to a full repaint rather than
dropping damage. Both the compositor and the kernel verifier recompute the
frame from the same pure layout functions in `zc_abi::desktop`, and the kernel
hashes the display against the expected final frame. A compositor that misses
the old window position leaves stale pixels and fails the boot.

**The application model is retained / backing-store.** The compositor owns a
back buffer per surface and copies damaged regions into its screen buffer. The
toolkit-level choice between immediate and retained drawing for applications
is deferred to F8e, and the shell policy (panel, launcher, focus) to F8f; this
record fixes only the compositor's side of the contract.

## Consequences

- A client's pixels never cross an IPC message: only the capability does. IPC
  stays one word per message and is used for control, not bulk data.
- Damage tracking is proven, not assumed: the kernel's independent checksum
  fails the boot when a repaint is missed.
- The surface table is bounded (`SURFACE_SLOTS`, `SURFACE_MAX_PAGES`), so a
  bad geometry fails the syscall instead of exhausting memory. A surface window
  spans at most two page tables, matching the framebuffer's ceiling.
- `SYS_SURFACE_DESTROY` does not yet unmap the surface from tasks that already
  mapped it; the creator is expected to stop using it. A per-mapping revoke
  (clearing page-table entries on destroy) is a follow-up, needed before
  clients can be untrusted.
- Only the compositor holds the factory in this milestone, so it is the only
  task that can create a surface. Granting the factory to a client is a policy
  change for a later milestone.
