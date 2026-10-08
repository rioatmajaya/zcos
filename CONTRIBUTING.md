# Contributing to ZC OS

Thanks for helping build ZC OS. This document is short on purpose: build it,
test it, commit it, and record it.

## Prerequisites

```sh
./tools/verify-host.sh      # reports every missing tool with an install hint
```

You need a Rust toolchain (pinned in `rust-toolchain.toml`), the
`x86_64-unknown-uefi` and `x86_64-unknown-none` targets, `objdump`, `mtools`,
`qemu-system-x86_64`, and an OVMF firmware package.

## Build and test

```sh
cargo test --workspace        # host unit tests for every pure module
./tools/build-efi.sh          # build/zcos.img: loader + kernel + tasks + initramfs
./tools/run-qemu.sh           # boot it under QEMU + OVMF (interactive)

./tools/build-efi.sh --test   # CI path: build with isa-debug-exit
./tools/run-qemu.sh --test    # headless boot test; exits 0 on success

# The second CI run. The two frame proofs need different final frames, so CI
# proves each in its own boot (ADR 0029).
./tools/build-efi.sh --test-close && ./tools/run-qemu.sh --test-close
```

**The green path is a rule.** `main` must always build, pass host tests, and
boot in QEMU. If your change breaks the boot test, fix it before anything else.

## Repository layout

| Path | Contents |
|---|---|
| `boot/uefi-loader` | UEFI application loader |
| `kernel/zc-kernel` | `no_std` microkernel mechanisms (host-tested) |
| `kernel/zc-kernel-image` | Freestanding bootable kernel binary |
| `libs/zc-abi` | Versioned loader-to-kernel ABI, syscall numbers |
| `libs/zc-elf` | Shared ELF parser |
| `libs/zc-storage` | Storage/filesystem parsers linked only by the block domain |
| `user/*` | Ring-3 domains (drivers, shell, framebuffer, device manager) |
| `tools/` | Build, image, and QEMU scripts |
| `docs/` | Roadmap, architecture, ADRs, blocked components |

## Code conventions

- Pure logic (parsers, allocators, tables, policy) lives in host-testable
  modules in `zc-kernel` or a `libs/` crate. Test it there, not only on bare
  metal.
- `unsafe` is denied workspace-wide. Keep the small set of exceptions confined
  to page tables, GDT/TSS, APIC, and assembly stubs, and document why.
- Public items need docs (`missing_docs = "warn"`).
- No boot service may be called between the final `GetMemoryMap` and
  `ExitBootServices`.
- A change to the kernel/loader ABI must bump the protocol version and update
  `libs/zc-abi`.
- A change to a frozen interface (syscall numbers, IPC channels, the boot
  contract, the server protocol) must update `libs/zc-abi`, the matching
  document in [`docs/specs/`](docs/specs/README.md), and — when it changes a
  decision — an ADR, all in the same commit.

## Commit convention

Use [Conventional Commits](https://www.conventionalcommits.org/). The subject
is imperative and lower-case; the body explains *why*, not *what*.

```
<type>(<scope>): <subject>

<body>

<footer>
```

Types: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`,
`chore`, `revert`.

Scopes mirror the layout: `loader`, `kernel`, `abi`, `elf`, `blk`, `kbd`, `fb`,
`devmgr`, `shell`, `user`, `tools`, `ci`, `docs`, `roadmap`.

Examples:

```
feat(blk): claim the BAR window via runtime delegation
fix(kernel): rebuild the port bitmap on the claim path
docs(roadmap): add phases F7-F9 up to daily driver
```

Do not use vague subjects like `Update file terbaru`. If a commit spans
several unrelated changes, split it.

## Changelog policy

Every user-visible change gets an entry in [`CHANGELOG.md`](CHANGELOG.md) under
`## [Unreleased]`, in the right category (`Added`, `Changed`, `Deprecated`,
`Removed`, `Fixed`, `Security`). The CI `changelog` job checks that the section
exists.

- Reference the roadmap phase or sub-task where it helps (`F6i`).
- Describe the effect, not the diff.
- At release time, rename `[Unreleased]` to the new version and date, and add a
  fresh empty `[Unreleased]` section on top.

## Versioning

SemVer on the `0.x` line, aligned with roadmap phases: F7 → `0.2.0`,
F8 → `0.3.0`, F9 → `0.4.0`–`0.9.0` release candidates, and `1.0.0` for the
first desktop daily-driver release. Breaking an ABI, syscall number, or on-disk
format requires at least a minor bump and a changelog `Changed`/`Removed` note.

## Branch and pull request flow

1. One phase, one branch (skill rule **P2**): branch from `main`, name it
   `feat/f6i-port-claims` or `fix/...`.
2. Keep the branch focused. If a component stalls for two working weeks, park
   it per [`docs/blocked/TEMPLATE.md`](docs/blocked/TEMPLATE.md) and move on.
3. Before opening a PR: `cargo test --workspace`, then
   `./tools/build-efi.sh --test && ./tools/run-qemu.sh --test`, then the
   `--test-close` pair.
4. Update the changelog and, if you made an architectural choice, add an ADR
   under [`docs/adr/`](docs/adr/README.md).
5. A PR is mergeable when the `host`, `boot`, and `changelog` CI jobs are
   green.

## Definition of done

Code on `main` + pass criteria proven + evidence recorded in the changelog + no
dangling TODO on that path. See the pass criteria for the current phase in
[`docs/roadmap.md`](docs/roadmap.md).

## License

By contributing you agree your work is dual-licensed under MIT or Apache-2.0,
matching the project.
