# 0101 — Shell standard output redirection and exit status tracking

- **Status:** accepted
- **Date:** 2026-11-11
- **Phase:** F7j, F4 (see [`../roadmap.md`](../roadmap.md))
- **Affects:** `user/zc-shell`, `tools/boot-script.txt`, `.github/workflows/rust.yml`, Section 6.4 (Shell)
- **Supersedes:** none

## Context

Prior to this change, `zc-shell` sent all command outputs exclusively to the kernel serial log via `SYS_LOG`. Commands had no mechanism to redirect stdout to files (`>` for overwrite/create, `>>` for append), and the shell did not retain or expose the exit status of executed commands.

In standard POSIX and unix shells, file redirection (`cmd > file`, `cmd >> file`) and the `$?` variable (which expands to the decimal exit status of the most recently executed command) are fundamental primitives for scripting, composability, and error handling. Furthermore, Section 6.4 of `docs/parity-with-zcos.md` explicitly calls for redirection and exit status reporting in the shell.

## Decision

**Implement standard output redirection (`>`, `>>`) and command exit status tracking (`$?`) in `zc-shell`.**

1. **Redirection State and Dispatch:**
   - Define `static mut REDIR_FD: u64 = u64::MAX;` to hold an open file descriptor when stdout is redirected.
   - Define `RedirMode { None, Overwrite, Append }`.
   - Update `out_bytes`, `out_flush`, `print_bytes`, and `print_lines`:
     - When `REDIR_FD == u64::MAX`, stdout formatting stages bytes into `OUT` and flushes to the serial console via `log()`, preserving normal logging prefixes (`task 2:`).
     - When `REDIR_FD != u64::MAX`, output writes directly to the open file descriptor via `write(redir_fd, bytes)`, bypassing kernel console logging.
   - Update `cat` to track `last_byte` and omit trailing newlines when the file already terminates in a newline or when output is redirected, avoiding spurious empty log lines on serial console.

2. **Command Line Parsing & File Creation/Append:**
   - Scan command line tokens in `run()` for `>` or `>>` operators (supporting both separated syntax `cmd > out.txt` and attached syntax `cmd >out.txt`).
   - Extract the redirection target path and resolve it relative to the working directory (`CWD`) using `resolve_path_buf`.
   - On `Overwrite` (`>`): if the target file exists, remove it with `unlink` before creating a new node with `SYS_CREATE`, ensuring 0-byte truncation semantics across both `tmpfs` and `zcfs`.
   - On `Append` (`>>`): create the target if nonexistent, open it, and seek to `st.size` with `SYS_SEEK` (when supported) or open directly for appended writes.
   - Execute the command with `REDIR_FD` set, flush remaining staged bytes via `out_flush()`, close the file descriptor with `close(redir_fd)`, and reset `REDIR_FD = u64::MAX`.

3. **Exit Status Tracking (`LAST_STATUS` and `$?`):**
   - Define `static mut LAST_STATUS: u32 = 0;` and a helper `fn set_status(status: u32)`.
   - Instrument all built-in commands:
     - Record status `0` on successful command execution.
     - Record status `1` (or appropriate non-zero error) on failure or invalid arguments.
     - Record status `127` for unknown commands.
   - Update `echo` argument parsing: if an argument matches `"$?"`, expand it to the ASCII decimal string of `LAST_STATUS`.

4. **Testing and Verification:**
   - Add exercises in `tools/boot-script.txt`:
     - Check `$?` after a failing `cd` command (verifies status `1`).
     - Redirect `echo` with `>` and verify content via `cat`.
     - Check `$?` after successful redirection (verifies status `0`).
     - Redirect `echo` with `>>` and verify appended content via `cat`.
     - Redirect `pwd` into a file and verify via `cat`.
     - Clean up temporary redirection files with `rm`.
     - Test redirection failure on non-existent directories and verify `$?` reflects failure.
   - Update `.github/workflows/rust.yml` with assertions verifying redirected contents and `$?` inspection.

## Consequences

- The shell now supports basic output redirection and exit status inspection without dynamic memory allocation or altering the kernel capability boundary.
- All workspace tests (455/455) pass, and all locked desktop framebuffer checksums (`0x18f9ce724a929efc`, `0x6d66e82d0f2d6fb8`, `0xf89e1ca133f3e8dc`) remain byte-for-byte identical.
