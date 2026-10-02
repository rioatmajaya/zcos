# 0004 — Framebuffer-first rendering, no 3D in v1

- **Status:** Accepted
- **Date:** 2026-10-01
- **Phase:** F6, F8

## Context

A GPU driver is one of the largest, least portable pieces of an OS, and 3D
acceleration is not required to prove a desktop stack. The UEFI loader already
hands us a linear framebuffer.

## Decision

The first renderer draws to the UEFI framebuffer with software composition.
The compositor and its clients use a shared-buffer protocol; text is rendered
from TrueType `glyf` outlines. Hardware GPU acceleration (AMD iGPU) is a later
driver-domain feature and is optional for the 1.0 daily-driver release.

## Consequences

- The desktop stack can be built and tested in QEMU with no GPU driver.
- Rendering is CPU-bound; damage tracking and partial updates matter early.
- We avoid X11/Wayland compatibility and 3D APIs entirely in v1, which keeps
  the desktop phase from ballooning.
