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
| [0011](0011-crash-recovery-clamps-log-head.md) | Crash recovery clamps the log head | Accepted |
| [0012](0012-fsck-repairs-the-durable-prefix.md) | fsck repairs the durable prefix | Accepted |
| [0013](0013-permissions-and-ownership.md) | File permissions and ownership in the VFS | Accepted |
| [0014](0014-linux-driver-domain.md) | Reuse Linux drivers in a userspace driver domain | Accepted |
| [0015](0015-compositor-surface-model.md) | Compositor surface model and damage tracking | Accepted |
| [0016](0016-window-client-delegation.md) | Window clients and delegated surfaces | Accepted |
| [0017](0017-input-stream-with-mouse.md) | Input stream with a PS/2 mouse | Accepted |
| [0018](0018-window-placement-proof.md) | Window placement proof for interactive pixels | Accepted |
| [0019](0019-port-broker-capability.md) | Port broker capability for ring-3 PCI discovery | Accepted |
| [0020](0020-device-memory-broker.md) | Device memory broker capability for ring-3 drivers | Accepted |
| [0021](0021-coherent-dma-window.md) | Coherent DMA window for driver domains | Accepted |

## Adding a decision

1. Copy [`TEMPLATE.md`](TEMPLATE.md) to `NNNN-short-title.md` with the next
   number.
2. Keep it 3–10 lines of decision. Link the code and the roadmap phase.
3. Add a row to the index above.
4. Once accepted, do not rewrite it. Supersede it with a new ADR and mark the
   old one `Superseded by NNNN`.
