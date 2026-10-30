# 0053 — Identity is readable but not settable

- **Status:** accepted
- **Date:** 2026-10-29
- **Affects:** `libs/zc-abi`, `kernel/zc-kernel`, `kernel/zc-kernel-image`, `user/zc-user`, `user/zc-shell`, `user/zc-initd`, F7f

## Context

The kernel has enforced POSIX-style file permissions for several phases: each
task carries an `Identity` fixed at spawn, the VFS checks it against a node's
owner and mode, and `initd` is the only root task. But the identity was
invisible from inside a domain. A person could only infer it from an `audit:`
line or by reading the kernel's owner table, and a task could not assert its
own privilege. The parity map lists a minimal identity/session boundary as part
of the core system, and this is the honest first half of it.

## Decision

**A task can read the identity it was spawned with, and nothing can change it
at runtime.**

- [`zc_abi::SYS_GETID`](../libs/zc-abi/src/syscall.rs) returns one word: user
  id in the high 32 bits, group id in the low 32 bits. `identity_word`,
  `identity_uid`, and `identity_gid` encode and decode it.
- The kernel answers from the running task's owner record, the same record the
  VFS permission check uses, so the two can never disagree.
- `initd` checks that it is root at startup and logs `initd: root identity`;
  the shell's `id` command prints its unprivileged `uid=1 gid=1`.
- There is no setuid, setgid, or login syscall. Changing an identity would be
  a new mechanism with its own authority story, not a flag on this one.

## Consequences

- The privilege split between `initd` and every other domain is observable in
  the boot transcript instead of being an implementation detail.
- A task cannot escalate by asking: the answer is read-only, and the kernel
  never takes an identity from a register.
- Authentication, sessions, and login are still absent. This reports the
  identity the boot chose; it does not establish one, and a session manager
  will need more than a getter.
