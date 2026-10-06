# 0022 — Terminal commands execute over the VFS

- **Status:** Accepted
- **Date:** 2026-10-06
- **Phase:** F8 (see [`../roadmap.md`](../roadmap.md))

## Context

F8d made the client window a live terminal. The kernel serves a fixed keystroke
script over `SYS_TERM_READ` and replays the same script through the shared
`zc_abi::terminal::Term` to recompute the window pixels (`wm: content ok`), so a
client that drops a key or mis-renders fails the frame checksum. The roadmap's
remaining F8d-3 item asks the terminal's commands to act on the F7 VFS — but
`Term` executed commands *inside* the state machine the kernel replays, so a
`cat` would make the verifier run filesystem operations it must not trust.

## Decision

**Editing and execution are separate.** `Term::push_key` only edits: Enter
finalizes the input line and returns a `Line`, and it appends neither output nor
the next prompt. One shared `run_command(term, line, read_file)` dispatches the
commands (`help`, `echo`, `cat`) and appends their output plus a fresh prompt.

**The caller injects the filesystem.** `cat` reads through `read_file`, which
the caller supplies. The window client passes a syscall-backed reader (the VFS);
the kernel's frame verifier passes a VFS-backed reader (its own mount table).
Both run the *same* `run_command` over the *same* script, so they derive the
same screen while each reaches the filesystem its own way.

**The boot script exercises the VFS.** `SCRIPT` is `help\ncat hello.txt\n`. The
kernel's replay reads `hello.txt` from the initramfs and logs `wm: vfs content
ok`; `wm: content ok` then proves the client's window equals that VFS-derived
screen, so the client's read is proven end to end rather than logged and
trusted. The client logs `client: vfs ok` on its first successful read.

## Consequences

- The window content proof now covers a real filesystem read: a client that
  fails to open the file paints `cat: cannot open` and fails `wm: content ok`.
- The reader is read-only, so the verifier runs no side effects on the
  surface-destroy path; a filesystem served over IPC returns `WouldBlock` and
  reports unavailable, so the scripted session stays on the initramfs.
- The frame hash changes: the window content now includes the file bytes. CI
  greps the literal `fb: desktop checksum ok`, and the roadmap allows a hash
  change when it is deliberate.
- `cat` is the only filesystem command today; `stat`/`ls`/`write` are
  follow-ups, and a write path needs a proof that can account for its effects.
