# 0100 — Shell working directory tracking and relative path resolution

- **Status:** accepted
- **Date:** 2026-11-11
- **Phase:** F7j, F4 (see [`../roadmap.md`](../roadmap.md))
- **Affects:** `user/zc-shell`, `tools/boot-script.txt`, `.github/workflows/rust.yml`, Section 6.4 (Shell)
- **Supersedes:** none

## Context

Prior to this change, `zc-shell` operated with no working directory state:
every command operating on files or directories (`cat`, `head`, `wc`, `ls`, `stat`,
`touch`, `cp`, `write`, `append`, `rm`, `mkdir`, `rmdir`, `mv`, `chmod`) required
explicit paths, and relative paths were resolved by the kernel virtual filesystem (VFS)
against the root mount point (`/`).

Standard shell navigation commands `pwd` (print working directory) and `cd` (change
working directory) were missing. Users could not navigate directory hierarchies or
reference files relative to their current location, nor use standard path components
like `.` (current directory) or `..` (parent directory) from the shell.

## Decision

**Implement working directory tracking (`pwd`, `cd`) and canonical relative path resolution in `zc-shell`.**

1. **Working Directory State:**
   - Define a fixed-capacity buffer `CWD` (`const CWD_CAP: usize = 128;`) initialized to `"/"`.
   - Invariant: `CWD` always begins with `'/'` and contains no trailing slash unless it is the root `"/"`.
   - Provide `get_cwd() -> &'static [u8]` and `set_cwd(path: &[u8])`.

2. **Canonical Path Resolution (`resolve_path_buf`):**
   - Resolves any relative or absolute path against `get_cwd()` into a canonical absolute path inside a caller-provided stack buffer.
   - Strips redundant slashes and normalizes `.` and `..` components:
     - `.` leaves the current path unchanged.
     - `..` moves to the parent directory component (clamped at root `/`).
   - If `input` begins with `/`, it is treated as absolute and normalized; otherwise it is appended to `CWD` and normalized.

3. **Built-in `pwd` and `cd` Commands:**
   - `pwd`: Outputs `get_cwd()` followed by `\n` to the staged output buffer and flushes.
   - `cd [dir]`:
     - With no argument, `"/"`, or `"~"`, resets `CWD` to `"/"`.
     - Otherwise, resolves the target path with `resolve_path_buf`.
     - Validates the target using `stat(target, &mut st)`. If missing, reports `cd: no such file or directory\n`. If `st.kind != KIND_DIR`, reports `cd: not a directory\n`. On success, updates `CWD`.

4. **Relative Path Resolution Across Commands:**
   - Update file utilities (`cat`, `head`, `wc`, `ls`, `stat`, `touch`, `cp`, `write`, `append`, `rm`, `mkdir`, `rmdir`, `mv`, `chmod`) to resolve path arguments through `resolve_path_buf`.
   - When no argument is supplied to `ls`, list the current directory (`get_cwd()`).
   - Diagnostic messages preserve the exact command arguments (e.g. `stat file.txt` prints `file.txt: ...`).

5. **Tooling and Validation:**
   - Update `help` in `zc-shell` to advertise `cd` and `pwd`.
   - Add exercises in `tools/boot-script.txt` for `pwd`, `cd`, relative file creation (`write`, `cat`, `stat`, `rm`), and directory error reporting.
   - Update `.github/workflows/rust.yml` to verify `pwd` output and relative file operations.

## Consequences

- Interactive shell navigation mirrors standard POSIX conventions without changing the underlying microkernel capability or VFS model.
- Path normalization occurs entirely in userspace (`zc-shell`) within a bounded stack allocation without dynamic memory allocation (`no_std`).
- All existing tests and locked framebuffer checksums remain valid.
