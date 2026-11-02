# 0055 — The boot manifest is parsed and checked, not yet authoritative

- **Status:** accepted
- **Date:** 2026-11-02
- **Affects:** `libs/zc-supervise`, `user/zc-initd`, `initramfs/zc.manifest`, `docs/specs/boot-manifest.md`, F7f

## Context

`zc.manifest` has been in the initramfs since the first images, but nothing read
it as a manifest. The producer opened it and checked only that the first six
bytes were `name=Z`; no field was parsed, and the service list was not in the
file at all. The parity map still listed "one boot manifest that names services
and order" as open, because the compile-time catalog was the only place a
service could be named.

The kernel has to spawn the catalog before any task runs: it builds each
address space and provisions capability grants in the boot path. A userspace
file cannot drive that yet. But leaving the file unparsed means a stale or
misspelled boot file is indistinguishable from a correct one.

## Decision

**Parse the manifest at boot and check it against the compiled catalog; do not
let it drive spawn order yet.**

- [`libs/zc-supervise/src/manifest.rs`](../libs/zc-supervise/src/manifest.rs)
  defines a tiny allocation-free grammar: one `key=value` per line, blank lines
  and `#` comments ignored, keys and values trimmed, unknown or duplicate keys
  rejected. Known keys are `name`, `version`, `init`, and `services`.
- `services` is a comma-separated list of catalog **names** in boot order, not
  numeric ids. The parser bounds the list at eight entries and rejects empty or
  duplicate names.
- The shipped `initramfs/zc.manifest` gains `services=blk,kbd`. A host test
  parses the shipped file with `include_str!`, so changing the file without
  changing the parser or catalog fails `cargo test`.
- `initd` reads the file once at startup, logs `initd: manifest ok` when
  `init=/sbin/initd` and the service names match the catalog in order, and logs
  a specific mismatch or malformed line otherwise. It continues to supervise
  the compiled catalog in every case.
- The manifest is not reloaded, and it does not choose spawn or shutdown order.
  Those remain compile-time data until the kernel can read the file before it
  builds address spaces.

## Consequences

- A stale manifest is loud in the boot transcript instead of silently ignored,
  while a bad file cannot brick an otherwise working image.
- The grammar is strict on purpose: a typo is a reported error, not a silently
  dropped key.
- The file now documents the same service order the image uses, so a person can
  read the boot without reading Rust.
- Full authority is still missing. A future change can move the parse into the
  kernel boot path or make `initd` the spawner; either requires a new decision
  about where address spaces and grants are built.
