# 0078 — A stable userspace C ABI

- **Status:** accepted
- **Date:** 2026-11-07
- **Affects:** `user/zc-hello`, `tools/build-efi.sh`, F5
- **Supersedes:** none

## Context

Every userspace task so far has been Rust: the freestanding `no_std`
binaries link against `zc-user`, which wraps the `int $0x80` gate. A task
written in C had no entry point, no syscall wrappers, and no way to reach
the kernel without reimplementing the trap contract. The ABI numbers and
calling convention are stable, but nothing exposed them to a C compiler.

## Decision

**Provide a minimal freestanding C runtime and a proof task.**

- `user/zc-hello/main.c` is a freestanding C program: it defines `_start`,
  calls `SYS_LOG_WRITE` through inline assembly, then `SYS_TASK_EXIT`. It
  uses no libc and no start files.
- `user/zc-hello/user.ld` links it at `0x4C0000`, clear of the Rust proof
  images, so its symbols never collide with them.
- `tools/build-efi.sh` compiles it with the system cross-compiler
  (`x86_64-linux-gnu-gcc -nostdlib -nostartfiles -static`) and packs it as
  `.hello-c.elf`, which stays hidden from `RamFs::read_dir` like the waiter.
- The boot proof spawns `.hello-c.elf` and greps for `task 0: hello from C`.

The syscall numbers and register convention are the contract; the C program
reaches them through inline assembly rather than a shared library, so the ABI
stays the single source of truth in `libs/zc-abi`.

## Consequences

- A non-Rust task can now run: the proof logs `task 0: hello from C` on
  every boot. The three locked checksums still hold because the image is
  dot-prefixed and hidden from the root listing.
- This is a proof, not a libc: there are no C library functions, no
  headers, and no dynamic linking. A real C library is future work.
- Adding a real libc would build on the same trap contract and the same
  freestanding link model.

## References

- [ADR 0003](0003-rust-implementation-language.md)
- [docs/specs/syscall-abi.md](../specs/syscall-abi.md)
