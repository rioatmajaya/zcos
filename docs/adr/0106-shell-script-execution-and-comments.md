# 0106 — Shell script execution (`sh`, `source`, `.`) and comments (`#`)

- **Status:** accepted
- **Date:** 2026-11-11
- **Phase:** F7j, F4 (see [`../roadmap.md`](../roadmap.md))
- **Affects:** `user/zc-shell`, `kernel/zc-kernel-image`, `tools/boot-script.txt`, `.github/workflows/rust.yml`, Section 6.4 (Shell)
- **Supersedes:** none

## Context

Following working directory management (ADR 0100), output redirection and exit status tracking (ADR 0101), pattern matching and conditionals (ADR 0102), command chaining (ADR 0103), environment variables with parameter expansion (ADR 0104), and pipeline execution (ADR 0105), `zc-shell` lacked batch script execution capabilities and shell comment parsing.

Section 6.4 of `docs/parity-with-zcos.md` explicitly specifies:
> "Add pipelines, redirection, background jobs, history, completion, and scripts only as far as the core services can support them."

Without script execution and comment parsing:
- Users and automated bring-up services could not execute multi-line batch scripts stored in the filesystem.
- Shell configuration scripts (such as environment exports or initialization sequences) could not be sourced into the current shell environment.
- Inline and line-level comments (`#`) caused syntax and execution errors ("unknown command: #...").
- Command invocations with arguments in scripts could not receive positional parameters (`$0`, `$1`..`$9`, `$#`).

## Decision

**Implement shell script execution (`sh`, `source`, `.`, and direct path invocation), shell comment parsing (`#`), quote-aware argument splitting, and positional parameters (`$0`..`$9`, `$#`) in `zc-shell`.**

1. **Comment Parsing (`#`):**
   - Unquoted `#` characters at the start of a line or preceded by whitespace (`' '`, `'\t'`) mark the start of a comment.
   - All characters from `#` to the end of the line are stripped via `strip_comment(line)`.
   - Empty or comment-only lines are ignored without altering exit status or interrupting execution.
   - Quoted `#` characters (e.g. `echo "# not a comment"`) are preserved.

2. **Quote-Aware Splitting & Operator Delimitation:**
   - Single (`'`) and double (`"`) quotes are recognized during argument splitting in `split()`, chaining operator detection in `run()`, and pipeline operator detection in `run_pipeline()`.
   - Operators (`;`, `&&`, `||`, `|`, `>`, `>>`) inside quotes are treated as literal argument characters rather than syntax delimiters.
   - Outer quotes enclosing arguments are cleanly stripped when passed to commands.

3. **Script Execution Built-ins & Path Execution:**
   - `sh <file> [args...]`: Executes `<file>` line-by-line using chunked reads bounded by `MAX_USER_IO_LEN` (512 bytes).
   - `sh -c "<command_string>"`: Directly executes `<command_string>` through `run()`.
   - `source <file> [args...]` and `. <file> [args...]`: Sourced execution into the current shell environment.
   - Direct path execution: If an unrecognized command name starts with `./` or `/`, or ends with `.sh`, the shell inspects VFS via `stat`; if it exists and is a regular file, it executes the file as a script.

4. **Positional Parameters & Environment Scoping:**
   - Script invocations pass `$0` (script path), `$1`..`$9` (arguments), and `$#` (argument count) via the shell parameter expansion mechanism (`expand_word`).
   - Prior positional parameters are backed up to static `.bss` storage (`SAVED_PARAMS`) and restored upon script termination, supporting nested scripts up to `MAX_SCRIPT_DEPTH = 4`.
   - `env` output skips internal positional parameters (`$0`..`$9`, `$#`), keeping environment variable listings clean.

5. **Serial Ring Capacity Headroom:**
   - Increased COM1 serial receive ring buffer `INPUT_RING_CAP` in `kernel/zc-kernel-image` from 2048 to 4096 bytes to accommodate extended scripted test suites.

6. **Testing and Verification:**
   - Verified via `tools/boot-script.txt`:
     - Multi-argument script creation and execution via `sh /tmp/test_sh.sh sh_arg`
     - Sourced script execution via `source /tmp/test_sh.sh src_arg`
     - Command string execution via `sh -c "echo zcos_sh_c_ok"`
     - Full line comments and inline comments with `#`
   - Asserted in `.github/workflows/rust.yml` CI boot diagnostics step.
