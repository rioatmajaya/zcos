# 0044 — The file manager opens a file by proof

- **Status:** accepted
- **Date:** 2026-10-21
- **Affects:** `libs/zc-abi` (`fm`, `session`, `cursor`, `lib`), `user/zc-fm`,
  `kernel/zc-kernel-image`, CI, F8h

## Context

F8h-1c-3 gave window 1 a navigable file manager: a click on a directory walks
into it, a file click highlights the row, and the title bar carries the path.
That still stops short of the app's central job. A file manager that can name a
file but cannot open it is a browser of labels, and the proof only covered
`SYS_READDIR` — the read path was already used by the terminal, but not through
this window's own state machine.

The proof problem is the same one navigation solved, with one more input. If the
client alone chose a file and formatted its bytes, the verifier could only check
that *some* preview was painted. The file's content, the line wrapping, and the
binary/empty decisions would all be trusted claims. The terminal avoids that by
sharing its model and injecting the file reader; the file manager must do the
same.

## Decision

**Opening is part of `Fm`, and the reader is injected.** A first click on a file
row selects it; a second click on that row reads the file through a
caller-supplied reader and enters preview mode. The client passes a closure over
`SYS_OPEN`/`SYS_READ`; the verifier passes `kernel_read_file`, which walks the
kernel's own mount table. Both call the same `Fm::open`, so the path join, read
length, formatting, and bytes cannot diverge.

**The preview is a fixed, deterministic format.** The shared `Fm` wraps text at
`FM_COLS`, turns a tab into one space, drops carriage returns, and limits the
body to its content rows. A binary file (a byte outside printable ASCII, tab,
newline, or carriage return) becomes `[binary file]`; an empty read becomes
`(empty)`. The file path replaces the directory path, so the title reads
`Files /hello.txt`, and the synthetic `..` row returns to the directory listing.
A failed read leaves the selection in place rather than showing a false empty
file.

**The scripted session now proves navigation and a read.** After launching
window 1 the `--fm` script opens `dev/`, clicks its `..` row to return to the
root, then clicks `hello.txt` twice. The tenth visible row is `hello.txt` after
the `data/`, `tmp/`, and `dev/` mount points and the six alphabetically earlier
files (`blk.elf` through `font8x16.raw`), so the second click lands at
`(106, 630)`. `replay_launch` feeds every press into the same `Fm::click`; the
verifier fails with `fm window did not open hello.txt` if the replay does not
end on the file, and with `fm window read no file` if its reader never ran.

**The shipped image still serves no launch suffix.** Only the `--fm` image packs
`fm.elf` and serves the launcher and file-manager steps, so both shipped frame
checksums are unchanged.

## Consequences

- The `--fm` boot logs `task 9: fm: preview ok` and `fm: preview ok` before two
  `wm: content ok` lines and `fb: desktop checksum ok`; its frame checksum
  becomes `0x78ebd6d359914da7`.
- The shipped checksums remain `0xd616ad79aaefb099` and
  `0x6d66e82d0f2d6fb8`.
- A client that opens another file, paints different bytes, or never reads
  fails the same pixel compare as before. The read is proven in both the client
  and the verifier, not inferred from a marker.
- `hello.txt` is the last row the window can show without scrolling. A root
  listing with more early entries would push it out of view; scrolling remains
  a later slice.
- Clicking a character device opens an empty preview because a zero-length read
  is valid. A richer device view, a file editor, and a third app remain later
  F8h slices.
