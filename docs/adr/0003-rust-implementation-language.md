# 0003 — Rust as the implementation language

- **Status:** Accepted
- **Date:** 2026-10-01
- **Phase:** F0

## Context

An OS is mostly memory management, parsing untrusted data, and concurrency —
the three areas where C and C++ fail most often. We want memory safety without
giving up bare-metal control.

## Decision

Implement the loader, kernel, libraries, and userspace in Rust on the stable
toolchain (pinned in `rust-toolchain.toml`). `no_std` freestanding crates are
built with `x86_64-unknown-uefi` and `x86_64-unknown-none`. Pure logic lives in
host-testable modules; the workspace denies `unsafe_code` and warns on missing
docs.

## Consequences

- Parsers (boot ABI, cpio, ELF, ACPI, PCI) are memory-safe and unit-tested on
  the host, not only on bare metal.
- `unsafe` is confined to a small, reviewable set of modules (page tables,
  GDT/TSS, APIC, naked assembly stubs).
- We accept the nightly/stable friction around `extern "x86-interrupt"` and
  write naked-assembly stubs instead.
- Contributors must know Rust; there is no C ABI for kernel code.
