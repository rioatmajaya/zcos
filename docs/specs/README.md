# ZC OS interface specifications

These are the **normative** interfaces that cross the kernel/userspace boundary.
Every server, driver, and client depends on them, so they are frozen: a change
here is an ABI change, not a refactor. The Rust code in `libs/zc-abi` is the
single source of truth; these documents describe it and must be updated in the
same change that touches it.

| Spec | Fixes | Depends on it |
|---|---|---|
| [`syscall-abi.md`](syscall-abi.md) | syscall numbers, arguments, return codes, capability rights, object namespaces | every ring-3 task |
| [`ipc-abi.md`](ipc-abi.md) | `Message` layout, channels, endpoint semantics, window/supervision word encodings | every server and client |
| [`boot-info.md`](boot-info.md) | loader→kernel `BootInfo`, `MemoryRegion`, `FramebufferInfo`, validation rules | the loader and the kernel entry |
| [`server-protocol.md`](server-protocol.md) | service table, discovery handshake, filesystem exchange page and opcodes | `initd`, `zc-blk`, `zc-devmgr`, the kernel proxy |

## Rules

1. **Numbers are never reused.** Retire a syscall or channel by leaving it
   unused; do not renumber.
2. **Change the spec and the code together.** A change to `libs/zc-abi`, the
   syscall dispatch table (`kernel/zc-kernel/src/syscall.rs`), or the service
   table (`kernel/zc-kernel/src/service.rs`) must update the matching document
   in the same commit.
3. **Boot-contract changes bump the version.** Any change to `BootInfo` or its
   meaning increments `BOOT_PROTOCOL_VERSION` (`libs/zc-abi/src/lib.rs`) and is
   recorded in an ADR, so a loader and kernel built apart cannot silently
   disagree.
4. **Cross-namespace collisions are forbidden.** Object ids in the IRQ, port,
   service, and surface namespaces must never collide (see
   [`syscall-abi.md`](syscall-abi.md#capability-object-namespaces)); the
   `zc-abi` host tests assert this.
5. **An interface change needs an ADR when it changes a decision**, not just a
   value. See [`../adr/`](../adr/).
