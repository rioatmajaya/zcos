# 0107 — Standard character devices (`null`, `zero`, `urandom`) and service health inspection

- **Status:** accepted
- **Date:** 2026-11-11
- **Phase:** F7f, F7h (see [`../roadmap.md`](../roadmap.md))
- **Affects:** `kernel/zc-kernel`, `libs/zc-abi`, `user/zc-user`, `user/zc-shell`, `tools/boot-script.txt`, `.github/workflows/rust.yml`, Section 6.3 (Core system)
- **Supersedes:** none

## Context

Prior ADRs established the microkernel capability architecture, the VFS with mounted filesystems (`ramfs`, `tmpfs`, `zcfs`, and `devfs`), supervised service domains (`initd`, `blk`, `kbd`), and the interactive shell (`zc-shell`). However, key foundational core system gaps remained documented in [`docs/parity-with-zcos.md`](../parity-with-zcos.md) Section 6.3:

1. **Absence of Standard Character Pseudo-Devices:**
   - `DevFs` only held descriptive nodes for supervised services (`/dev/blk`, `/dev/kbd`).
   - Standard Unix pseudo-devices (`/dev/null`, `/dev/zero`, `/dev/urandom`) were absent.
   - Any shell redirection or script seeking to discard output via `> /dev/null` failed with `VfsError::NotFound` or `NotSupported` on write.
   - Tools and tests could not generate zero-byte streams or entropy from `/dev`.

2. **Incomplete Service Registry Observation:**
   - While `RestartBudget` classified service health into `Healthy`, `Degraded`, and `Exhausted` ([ADR 0097](0097-explicit-service-dependencies-and-failure-health-states.md)), this classification was internal to `zc-supervise`.
   - The userspace shell command `svc` reported raw mechanism counters (`restarts`, `faults`) and runtime endpoints, but did not present human- or script-readable health classifications or explicit dependency declarations (`requires`).

3. **Bounded Stream Inspection in Shell:**
   - Shell utility `head` only supported line counts (`-n <lines>`). Character device streams lacking newline delimiters (such as `/dev/zero` or binary files) could not be read with a byte-count limit (`-c <bytes>`).

## Decision

**Implement standard character pseudo-devices (`/dev/null`, `/dev/zero`, `/dev/urandom`) in `DevFs`, support byte-count limits in shell `head` (`-c`), and integrate service health classification and dependency reporting in `svc`.**

1. **Standard Character Pseudo-Devices in `DevFs` (`kernel/zc-kernel/src/devfs.rs`):**
   - Provide permanent pseudo-devices:
     - `/dev/null` (`NODE_NULL = 100`): reads return `Ok(0)` (immediate EOF); writes discard all data, returning `Ok(data.len())`.
     - `/dev/zero` (`NODE_ZERO = 101`): reads fill caller buffer with `0x00`, returning `Ok(out.len())`; writes discard all data.
     - `/dev/urandom` (`NODE_URANDOM = 102`): reads fill caller buffer with pseudo-random bytes generated via an allocation-free 64-bit xorshift PRNG initialized from offset and splitmix constants; writes discard all data.
   - Permissions: All three pseudo-devices report `mode: 0o666` (`PUBLIC_CHR_MODE`), granting unrestricted read/write access to both root and unprivileged tasks.
   - Service nodes (`blk`, `kbd`) retain node IDs `1` and `2`, `mode: 0o600`, and return `VfsError::NotSupported` on writes.
   - `read_dir` preserves catalog service order first (`blk`, `kbd`), followed deterministically by `null`, `zero`, and `urandom`.

2. **Shell Standard Output Redirection to `/dev/null`:**
   - `zc-shell` redirection (`>`, `>>`) seamlessly opens `/dev/null` and discards redirected output directly in the kernel without allocating or generating intermediate files.

3. **Shell `head` Byte-Count Flag (`-c`):**
   - Extended `head` in `user/zc-shell` to support `head -c <bytes> [file]` and pipeline inputs, enabling bounded byte reads from character streams like `/dev/zero`.

4. **Service Health Classification and Dependency Reporting (`libs/zc-abi`, `user/zc-shell`):**
   - Defined `service_health_classification(word: u64) -> &'static str` in `libs/zc-abi::service`:
     - `"healthy"`: 0 faults recorded.
     - `"degraded"`: 1..=3 faults, currently running within restart budget.
     - `"exhausted"`: down and exceeded restart budget (or down following faults).
   - In `zc-shell` `svc`: append `health=<healthy|degraded|exhausted>` and `requires=<dep1,dep2|none>` after `endpoint=...`, preserving full backward compatibility with existing CI substring checks.

5. **Testing and Verification:**
   - 456 workspace host unit tests passing (`cargo test --workspace`), including new unit tests for `DevFs` pseudo-devices and ABI health classification.
   - All three locked proof checksums verified and preserved:
     - `--test`: `0x18f9ce724a929efc`
     - `--test-close`: `0x6d66e82d0f2d6fb8`
     - `--fm`: `0xf89e1ca133f3e8dc`
   - Verified via `tools/boot-script.txt` and `.github/workflows/rust.yml`:
     - `stat /dev/null` (`char device, mode 0666`)
     - `stat /dev/zero` (`char device, mode 0666`)
     - `echo silent_dev_null > /dev/null`
     - `head -c 4 /dev/zero > /tmp/zero_test.txt` (`file, 4 bytes`)
     - `dev` enumerates `blk`, `kbd`, `null`, `zero`, `urandom`
     - `svc` reports `health=degraded requires=none` for restarted services.

## Consequences

- Tasks and scripts can now discard output via `/dev/null` and produce zeroed memory/buffers via `/dev/zero`.
- Service dependencies and health degradation are observable from userspace via `svc`.
- All kernel operations remain `#![no_std]` and allocation-free.
