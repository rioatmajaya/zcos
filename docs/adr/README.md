# Architecture Decision Records

Short records of decisions that shape ZC OS: what we decided, why, and what it
costs. They exist so the same debate does not happen twice (skill rule **P6**).

## Index

| ADR | Decision | Status |
|---|---|---|
| [0001](0001-microkernel-with-capabilities.md) | Capability-based microkernel | Accepted |
| [0002](0002-uefi-x86_64-first-platform.md) | x86_64 + UEFI as the first platform | Accepted |
| [0003](0003-rust-implementation-language.md) | Rust as the implementation language | Accepted |
| [0004](0004-framebuffer-first-no-3d.md) | Framebuffer-first rendering, no 3D in v1 | Accepted |
| [0005](0005-single-tss-port-bitmap.md) | One TSS, per-task port bitmap projected on switch | Accepted |
| [0006](0006-userspace-drivers-and-device-manager.md) | Userspace drivers with a device manager | Accepted |
| [0007](0007-vfs-core.md) | Read-only VFS with a trait-based filesystem | Accepted |
| [0008](0008-zc-native-log-structured-fs.md) | ZC-native log-structured filesystem | Accepted |
| [0009](0009-filesystem-rpc-replay.md) | Filesystem RPC with block-and-retry replay | Accepted |
| [0010](0010-userspace-service-supervision.md) | Userspace service supervision | Accepted |

## Adding a decision

1. Copy [`TEMPLATE.md`](TEMPLATE.md) to `NNNN-short-title.md` with the next
   number.
2. Keep it 3–10 lines of decision. Link the code and the roadmap phase.
3. Add a row to the index above.
4. Once accepted, do not rewrite it. Supersede it with a new ADR and mark the
   old one `Superseded by NNNN`.
