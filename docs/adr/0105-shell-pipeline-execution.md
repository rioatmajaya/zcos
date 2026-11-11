# 0105 — Shell pipeline execution (`|` operator)

- **Status:** accepted
- **Date:** 2026-11-11
- **Phase:** F7j, F4 (see [`../roadmap.md`](../roadmap.md))
- **Affects:** `user/zc-shell`, `tools/boot-script.txt`, `.github/workflows/rust.yml`, Section 6.4 (Shell)
- **Supersedes:** none

## Context

Following working directory management (ADR 0100), output redirection and exit status tracking (ADR 0101), pattern matching and conditionals (ADR 0102), command chaining (ADR 0103), and environment variables with parameter expansion (ADR 0104), `zc-shell` lacked pipeline support (`|`).

In Unix/POSIX shells, pipelines connect the standard output of an upstream command to the standard input of a downstream command. In `zc-shell`'s microkernel and `#![no_std]` environment, commands execute within the single-threaded shell domain. Without pipeline execution, users could not directly compose utilities (e.g. `echo ... | grep ...`, `ps | grep ...`, `svc | grep ...`, or `cat ... | wc`).

Section 6.4 of `docs/parity-with-zcos.md` explicitly calls for pipeline support:
> "Add pipelines, redirection, background jobs, history, completion, and scripts only as far as the core services can support them."

## Decision

**Implement bounded, ping-pong pipeline execution (`|` operator) in `zc-shell` across built-in utilities (`grep`, `wc`, `head`, `cat`).**

1. **Pipeline Execution Hierarchy & Precedence:**
   - Chain operators (`;`, `&&`, `||`) retain highest splitting precedence in `run(line)`.
   - Each chained command segment is passed to `run_pipeline(seg)`:
     - If `seg` contains no `|` token, it executes as a single command via `run_single(seg, None)`.
     - If `seg` contains `|`, it is split into sequential pipeline stages (`[stage_0, stage_1, ...]`) up to `STAGE_CAP = 4`.
   - Intermediate stages capture standard output into static buffers in `.bss`.
   - The final stage outputs to normal console logging or file redirection (`>`, `>>`) if specified.

2. **Ping-Pong Buffer Staging (`.bss`):**
   - Allocate two fixed-capacity staging buffers: `PIPE_A: [u8; 4096]` and `PIPE_B: [u8; 4096]`.
   - Use an alternating flag `PIPE_USE_B` and capture state `PIPE_CAPTURING`.
   - When `PIPE_CAPTURING` is active, `out_bytes()`, `print_bytes()`, and `print_lines()` direct output into the active pipeline staging buffer (`pipe_write_bytes`). Error messages (`log(...)`) continue to flow directly to console/stderr.
   - Stage `k` consumes the buffer populated by stage `k - 1` and writes to the alternate buffer, eliminating heap allocations, recursion, and data aliasing.

3. **Piped Input Consumers:**
   - `grep [-n] [-v] <pattern> [file]`:
     - When `[file]` is omitted or specified as `-` and piped input is present, searches line-by-line in the piped buffer.
     - Supports `-n` (line numbers) and `-v` (invert match), returning exit status 0 on match and 1 on no match.
   - `wc [file]`:
     - When `[file]` is omitted or `-` and piped input is present, counts lines, words, and bytes from the piped buffer.
   - `head [-n <N>] [file]`:
     - When `[file]` is omitted or `-` and piped input is present, prints up to N lines (default 10) from the piped buffer.
   - `cat [file]`:
     - When `[file]` is omitted or `-` and piped input is present, writes the piped buffer directly to standard output.

4. **Integration with Redirection and Conditionals:**
   - Pipelines compose cleanly with output redirection on the terminal stage:
     `cmd1 | cmd2 > file`
   - Pipelines compose with logical and sequential chaining:
     `cmd1 | grep needle && echo found`
     `cmd1 | grep needle || echo fallback`

5. **Testing and Verification:**
   - Verified via `tools/boot-script.txt`:
     - `echo zcos_pipe_ok | grep pipe_ok`
     - `echo foo | wc`
     - `echo pipe_and | grep pipe && echo pipe_and_ok`
   - Asserted in `.github/workflows/rust.yml` CI boot diagnostics step.
