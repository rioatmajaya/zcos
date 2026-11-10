# 0098 — File utilities in the shell: cp, touch, head, and wc

- **Status:** accepted
- **Date:** 2026-11-11
- **Phase:** F7j, F4 (see [`../roadmap.md`](../roadmap.md))
- **Affects:** `user/zc-shell`, `tools/boot-script.txt`, `.github/workflows/rust.yml`
- **Supersedes:** none

## Context

While `zc-shell` provided basic file operations (`cat`, `ls`, `stat`, `rm`,
`mkdir`, `rmdir`, `mv`, `write`, `append`, `chmod`), several standard shell file
utilities were missing:
1. `cp`: Duplicate or copy file contents from a source path to a destination path.
   Users had to read and rewrite manually or move (`mv`).
2. `touch`: Ensure a file exists (creating an empty file if missing) without
   overwriting existing contents.
3. `head`: Preview the initial lines of a file (up to 10 lines) rather than dumping
   the entire file via `cat`.
4. `wc`: Count lines (`\n`), whitespace-separated words, and bytes in a file.

## Decision

**Implement `cp`, `touch`, `head`, and `wc` built-in commands in `zc-shell`.**

- `touch <file>`: Attempts to open the file with `open(path)`. If it exists, closes
  it and reports `touch: updated <file>`. If it does not exist, invokes `create(path)`
  and reports `touch: created <file>` on success, or `touch: cannot create` on failure.
- `cp <src> <dst>`: Opens the source file, creates/opens the destination file,
  and streams bytes in 64-byte chunks using `read` and `write`. Reports
  `cp: copied <src> -> <dst>` on success.
- `head <file>`: Opens `<file>` and outputs characters up to 10 newline (`\n`)
  boundaries or EOF, closing the file and flushing output.
- `wc <file>`: Reads `<file>` in chunks and tracks line count (`\n`), word count
  (transitions from whitespace to non-whitespace), and total byte count, reporting
  `wc: lines <N> words <N> bytes <N> <file>`.
- Update `help` in `zc-shell` and the CI workflow assertion in `.github/workflows/rust.yml`
  to include the new commands.

## Consequences

- Common file inspection and management tasks in the interactive shell no longer
  require workarounds.
- Commands operate purely via standard VFS syscalls (`open`, `read`, `write`,
  `create`, `close`), adhering to existing ring-3 security and capability boundaries.
- Preserves all locked framebuffer proof checksums.
