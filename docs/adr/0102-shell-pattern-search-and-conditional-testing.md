# 0102 — Shell pattern search (grep) and conditional testing (test, true, false)

- **Status:** accepted
- **Date:** 2026-11-11
- **Phase:** F7j, F4 (see [`../roadmap.md`](../roadmap.md))
- **Affects:** `user/zc-shell`, `tools/boot-script.txt`, `.github/workflows/rust.yml`, Section 6.4 (Shell)
- **Supersedes:** none

## Context

Following stdout redirection and exit status tracking in ADR 0101, `zc-shell` lacked two ubiquitous unix shell capabilities:
1. Pattern matching and filtering across files (`grep`), allowing users to search for matching lines, inspect line numbers, and invert matches.
2. Conditional evaluation (`test`, `[`, `true`, `false`), allowing users and scripts to test file existence, file type (regular file vs directory), string emptiness, and string equality, communicating outcomes via command exit status `$?`.

In accordance with the project roadmap (Phase F7j / Section 6.4 of `docs/parity-with-zcos.md`) and the user mandate prioritizing core microkernel services and command shell features before GUI polish, these tools complete the basic POSIX scripting and inspection foundation of `zc-shell`.

## Decision

**Implement `grep` pattern matching, conditional evaluator `test` (and alias `[`), and boolean constants `true` and `false` in `zc-shell`.**

1. **Pattern Matching (`grep`):**
   - Syntax: `grep [-n] [-v] <pattern> <file>`
   - Supports flag `-n` (prefix line number to each matching line) and `-v` (invert match, selecting non-matching lines). Multiple flags can be specified independently or combined (e.g. `-nv`).
   - File paths resolve against the working directory (`CWD`) via `resolve_path_buf`.
   - Reads file data in 64-byte chunks and buffers lines up to 256 bytes in userspace.
   - Evaluates substring matches using `contains_subslice(haystack, needle)`.
   - Directs output through `out_bytes` and `out_flush()`, automatically routing to the console log or redirected files (`>`, `>>`).
   - Exit status semantics: returns status `0` if at least one matching line was found, status `1` if no lines matched, and status `2` on syntax or file open errors.

2. **Boolean Built-ins (`true`, `false`):**
   - `true`: records exit status `0`.
   - `false`: records exit status `1`.

3. **Conditional Testing (`test` and `[`):**
   - Supports both `test <expr>` and `[ <expr> ]` syntax (verifying the matching `]` token).
   - Evaluates file/directory conditions:
     - `-e <path>`: true if path exists.
     - `-f <path>`: true if path exists and is a regular file (`st.kind != KIND_DIR`).
     - `-d <path>`: true if path exists and is a directory (`st.kind == KIND_DIR`).
   - Evaluates string conditions:
     - `-z <str>`: true if string length is zero.
     - `-n <str>`: true if string length is non-zero.
     - `<str>`: true if non-empty.
     - `<str1> = <str2>` or `<str1> == <str2>`: true if strings are equal.
     - `<str1> != <str2>`: true if strings are not equal.
   - Returns exit status `0` on true, `1` on false, and `2` on syntax error.

4. **Integration and Testing:**
   - Updated `help` in `zc-shell` to advertise `grep`, `test`, `true`, `false`.
   - Added test exercises in `tools/boot-script.txt` covering `true`, `false`, `test -f`, `test -d`, `test -e`, `[ = ]`, `[ != ]`, `grep`, `grep -n`, `grep` non-matching, and redirection of `grep` output to a file followed by verification with `cat` and cleanup with `rm`.
   - Updated `.github/workflows/rust.yml` to assert all new behaviors in CI.

## Consequences

- The shell now supports pattern filtering and expression testing, enabling robust scripted verification in userspace.
- Implemented entirely in `#![no_std]` without dynamic memory allocation or heap usage.
- All 455 workspace host tests pass, and all locked framebuffer checksums remain byte-for-byte identical.
