# 0043 — The file manager navigates by proof

- **Status:** accepted
- **Superseded in part by:** [0045](0045-the-default-image-ships-the-apps-a-person-can-test.md)
  for the default image's contents
- **Date:** 2026-10-21
- **Affects:** `libs/zc-abi` (`fm`, `session`, `cursor`, `lib`), `user/zc-fm`,
  `kernel/zc-kernel-image`, CI, F8h

## Context

F8h-1c-2 made window 1 a file manager that lists the initramfs root. It was
deliberately static: the listing was built once, painted once, and the pointer
stream was ignored. That proves the renderer and the per-window plumbing, but it
does not prove the app can do the one thing a file manager must do — walk the
filesystem. The roadmap asks for a minimum daily app, and a listing a person
cannot open is a screenshot, not an app.

The hard part is not the click. It is keeping the frame proof honest once the
window's content depends on input again. If the client alone decided which
directory to open, the kernel could only verify that the client painted *some*
directory; the navigation would be a trusted claim, which is exactly the failure
mode the terminal's shared state machine exists to prevent.

## Decision

**The navigation is a shared state machine, not client code.** `zc_abi::fm::Fm`
gains the current path, a selected row, a row hit-test, and `click`. A click on a
directory enters it through a caller-supplied lister; a click on a file selects
it. A non-root listing starts with a synthetic `..` row that walks up. `reload`
is the one loader: it adds the parent row, calls the lister until `End` or
`Error`, and resets the path, rows, and selection together, so a navigation can
never paint the old directory's tail under a new title.

**The title bar carries the path.** The renderer draws `Files` followed by the
current directory, so the state the proof compares is visible in the pixels
rather than inferred from a log line.

**The verifier replays the click, not the result.** `zc_abi::session::replay_launch`
runs the launch script through a fresh `Wm`/`Cursor`/`Input`, adds window 1 on the
same `Kind::Launched` action the kernel mirrors, and hands each press inside
window 1's content area to a callback in surface coordinates. The client injects
`SYS_READDIR` into `Fm::click`; the verifier injects `kernel_list_dir` into the
same `Fm::click`. Both sides therefore execute the same click, row arithmetic,
path join, and list loop — only the filesystem authority differs.

**The `--fm` script ends with one click on a directory row.** After launching
window 1 it walks to `(106, 504)`, row 2 of the root listing — `dev/`, after the
`data/` and `tmp/` mount points the VFS lists first — and presses. The verifier
fails with `fm window did not navigate to /dev` if the replay does not end
there, so an ignored click cannot silently agree with a static client. The
shipped image still serves no launch suffix and both shipped checksums are
unchanged.

**Human interaction uses the same model.** `r`/Enter refreshes, Backspace walks
up, and the pointer's left press reaches `Fm::click`. Because the interactive
image serves no script, a person sees only their own input.

## Consequences

- The `--fm` boot logs `task 9: fm: pointer ok`, `task 9: fm: open ok`, and
  `fm: open ok` (verifier) before two `wm: content ok` lines and
  `fb: desktop checksum ok`.
- A client that opened another directory, ignored the click, or drew a different
  title fails the same pixel compare as before; no new trusted channel exists.
- Selection is host-tested but not part of the scripted end frame. The proof
  click opens a directory; a file click only highlights a row.
- The landing directory must be reachable through the kernel's own mount table
  without blocking, so the script chooses `dev/` (devfs) rather than `data/`
  (zcfs over IPC). The client can still open `data/` live; the scripted proof
  deliberately stays on the synchronous side.
- Scrolling, opening files, and a third app remain later F8h slices. `Fm` still
  caps at `FM_ROWS` rows and truncates a name to one row.
