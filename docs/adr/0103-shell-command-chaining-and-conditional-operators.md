# 0103 — Shell command chaining (;, &&, ||)

- **Status:** accepted
- **Date:** 2026-11-11
- **Phase:** F7j, F4 (see [`../roadmap.md`](../roadmap.md))
- **Affects:** `user/zc-shell`, `tools/boot-script.txt`, `.github/workflows/rust.yml`, Section 6.4 (Shell)
- **Supersedes:** none

## Context

With working directory tracking (ADR 0100), standard output redirection and exit status tracking (ADR 0101), and pattern matching / conditional evaluation (ADR 0102) in place, `zc-shell` could only execute a single standalone command per line. Users could not chain commands sequentially or conditionally based on previous exit status.

In POSIX and standard unix shells, command chaining operators are foundational:
- `;` executes the subsequent command sequentially regardless of the previous outcome.
- `&&` executes the subsequent command conditionally only if the preceding command succeeded (`$? == 0`).
- `||` executes the subsequent command conditionally only if the preceding command failed (`$? != 0`).

Supporting these operators in `zc-shell` completes the shell execution pipeline specified in Section 6.4 of `docs/parity-with-zcos.md`.

## Decision

**Implement command chaining operators (`;`, `&&`, and `||`) in `zc-shell`.**

1. **Parser and Dispatch Architecture:**
   - Define `ChainOp { None, Semi, And, Or }`.
   - Separate line execution into `run_single(line: &[u8]) -> bool` and `run(line: &[u8]) -> bool`.
   - `run_single` handles single-command argument parsing and command-local standard output redirection (`>`, `>>`), restoring stdout upon completion.
   - `run` scans the raw line for chaining tokens:
     - `;` -> `ChainOp::Semi`
     - `&&` -> `ChainOp::And`
     - `||` -> `ChainOp::Or`
   - Strips leading and trailing ASCII whitespace from each segment using `trim_ascii`.
   - Evaluates whether to execute the current segment against `prev_op`:
     - `ChainOp::None` or `ChainOp::Semi`: unconditionally executes.
     - `ChainOp::And`: executes only if `get_status() == 0`.
     - `ChainOp::Or`: executes only if `get_status() != 0`.
   - If any command in the chain requests exit (`run_single` returns `true`), `run` returns `true` immediately.

2. **Integration with Redirection and Status Tracking:**
   - Each segment in a chain independently manages its own output redirection without leaking redirection file descriptors across commands (e.g. `echo one > 1.txt; echo two > 2.txt`).
   - Every executed segment updates `LAST_STATUS` via `set_status()`, which directly determines whether subsequent `&&` or `||` segments run.

3. **Testing and Verification:**
   - Added test cases in `tools/boot-script.txt`:
     - Sequential chaining: `echo chain_one; echo chain_two`
     - Conditional AND: `true && echo chain_and_ok`
     - Conditional OR: `false || echo chain_or_ok`
     - Combined testing, pattern matching, and redirection: `test -f /hello.txt && grep -n hello /hello.txt > grep_out.txt`
   - Added assertions to `.github/workflows/rust.yml` to verify outputs in CI.

## Consequences

- The shell now supports robust multi-command chaining and conditional execution entirely in `#![no_std]` without dynamic memory allocation.
- All workspace unit tests (455/455) pass, all disk partition/fsck checks pass, and all locked framebuffer checksums remain byte-for-byte identical.
