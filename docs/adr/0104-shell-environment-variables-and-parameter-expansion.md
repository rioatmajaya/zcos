# 0104 — Shell environment variables and parameter expansion

- **Status:** accepted
- **Date:** 2026-11-11
- **Phase:** F7j, F4 (see [`../roadmap.md`](../roadmap.md))
- **Affects:** `user/zc-shell`, `tools/boot-script.txt`, `.github/workflows/rust.yml`, Section 6.4 (Shell)
- **Supersedes:** none

## Context

Following working directory management (ADR 0100), output redirection and exit status tracking (ADR 0101), pattern matching and conditionals (ADR 0102), and command chaining (ADR 0103), `zc-shell` lacked an environment variable storage mechanism and generalized parameter expansion.

Parameter expansion was previously restricted to `echo $?`, hardcoded solely inside the `echo` builtin handler. This prevented `$?` from being evaluated in conditionals (e.g. `test $? = 0`), hindered dynamic path lookups (such as `$PWD` or `$HOME`), and precluded setting custom shell variables.

Section 6.4 of `docs/parity-with-zcos.md` specifies a complete shell capable of inspecting and scripting system state within microkernel and `#![no_std]` constraints.

## Decision

**Implement static environment variable storage (`export`, `set`, `unset`, `env`) and parameter expansion (`$VAR`, `$?`, `$PWD`) in `zc-shell`.**

1. **Static Environment Storage:**
   - Define a fixed-capacity environment table in `.bss` (`ENV_CAP = 16`, `ENV_KEY_CAP = 32`, `ENV_VAL_CAP = 64`).
   - Store entries in `EnvEntry { key, key_len, val, val_len, occupied }`.
   - Pre-populate standard environment variables at shell initialization (`env_init`):
     - `USER=user`
     - `HOME=/`
     - `OS=zcos`
     - `SHELL=zc-shell`
     - `PWD=/` (dynamically updated whenever `set_cwd()` successfully executes).

2. **Built-in Environment Commands:**
   - `export` / `set`:
     - With no arguments: lists all defined variables (equivalent to `env`).
     - `export KEY=VALUE` / `set KEY=VALUE` (or `set KEY VALUE`): parses key-value pairs, expands value variables if referenced, and inserts or updates the environment table.
   - `unset KEY`: marks the matching variable key slot as unoccupied.
   - `env`: prints all active environment variables in `KEY=VALUE\n` format to stdout/redirected stream.

3. **Generalized Parameter Expansion:**
   - In `run_single`, before command execution:
     - Expand any token starting with `$` across argument slices via `expand_word`:
       - `$?`: formats `LAST_STATUS` into decimal ASCII in a static buffer slot (`STATUS_EXP_BUFS[slot]`).
       - `$PWD`: returns the active working directory from `get_cwd()`.
       - `$VAR`: looks up `VAR` in `ENV_TABLE`, returning the slice if found, or an empty slice `b""` if unset.
     - Redirection targets (`>`, `>>`) also undergo variable expansion, allowing dynamic target paths (e.g. `> $LOGFILE`).
     - `export`, `set`, and `unset` preserve argument variable names while resolving rhs values.
   - This eliminates the previous special case in `echo` and enables `$?` in `test` / `[` expressions (`[ $? = 0 ]`).

4. **Testing and Verification:**
   - Extended `tools/boot-script.txt` with environment variable lifecycle verification:
     - Setting variable: `export GREET=zcos_env_ok`
     - Variable expansion in echo: `echo $GREET`
     - Conditional evaluation with parameter expansion: `[ $GREET = zcos_env_ok ] && echo env_test_ok`
     - Unsetting variable: `unset GREET`
   - Added assertions to `.github/workflows/rust.yml` verifying `zcos_env_ok` and `env_test_ok` in boot logs.
   - Added `--no-install-recommends` and timeout options to CI apt package installation to prevent runner timeouts.

## Consequences

- The shell now supports standard environment variables, configuration parameters, and variable expansion across all built-in commands with zero heap allocations.
- Framebuffer proofs, all 455 workspace host tests, disk partition checks, and live input / poweroff tests pass without regression.
