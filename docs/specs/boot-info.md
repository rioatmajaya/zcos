# Boot-info ABI

The loader → kernel hand-off contract, and the fixed layout the kernel then
gives every ring-3 task. Normative source of truth: `libs/zc-abi/src/lib.rs`
(`BootInfo`, `MemoryRegion`, `MemoryKind`, `FramebufferInfo`, `PixelFormat`),
`kernel/zc-kernel/src/boot.rs` (validation), `boot/uefi-loader/src/loader.rs`
(producer). See [`README.md`](README.md) for the change rules.

## Entry contract

The UEFI loader builds the kernel's page tables, switches to them, and jumps to
the kernel entry point with a pointer to a `BootInfo` structure in **`rdi`**.
All addresses inside `BootInfo` are **physical** until the kernel establishes
its own virtual layout.

Any change to `BootInfo` or its meaning increments `BOOT_PROTOCOL_VERSION`, so
a loader and kernel built apart cannot silently disagree.

## `BootInfo` (80 bytes, `#[repr(C)]`)

| Offset | Size | Field | Notes |
|---|---|---|---|
| 0 | 8 | `magic` | must equal `BOOT_INFO_MAGIC` = `0x5A43_4F53_424F_4F54` (`ZCOSBOOT`) |
| 8 | 4 | `protocol_version` | must equal `BOOT_PROTOCOL_VERSION` (currently 2) |
| 12 | 4 | `flags` | reserved; must be zero in protocol version 2 |
| 16 | 8 | `memory_map` | physical address of the `MemoryRegion` array |
| 24 | 8 | `memory_map_len` | number of entries |
| 32 | 8 | `initramfs_start` | physical address, or zero if absent |
| 40 | 8 | `initramfs_len` | length in bytes |
| 48 | 8 | `rsdp` | physical address of the ACPI RSDP, or zero |
| 56 | 24 | `framebuffer` | `FramebufferInfo` |

## `MemoryRegion` (32 bytes, `#[repr(C)]`)

| Offset | Size | Field |
|---|---|---|
| 0 | 8 | `start` — physical start address |
| 8 | 8 | `len` — length in bytes |
| 16 | 4 | `kind` — `MemoryKind` (loader-owned, not a firmware value) |
| 20 | 4 | padding (aligns `attributes`) |
| 24 | 8 | `attributes` — firmware attributes |

`MemoryKind` mirrors the UEFI memory-type numbering for the values the loader
recognises, so conversion is a direct cast:

| Value | Kind | Value | Kind |
|---|---|---|---|
| 0 | `Reserved` | 8 | `Unusable` |
| 1 | `LoaderCode` | 9 | `AcpiReclaimable` |
| 2 | `LoaderData` | 10 | `AcpiNvs` |
| 3 | `BootServicesCode` | 11 | `Mmio` |
| 4 | `BootServicesData` | 12 | `MmioPortSpace` |
| 5 | `RuntimeServicesCode` | 13 | `PalCode` |
| 6 | `RuntimeServicesData` | 14 | `Persistent` |
| 7 | `Usable` | `u32::MAX` | `Unknown` |

Only `Usable` regions may be handed out by the frame allocator
(`MemoryKind::is_usable`).

## `FramebufferInfo` (24 bytes, `#[repr(C)]`)

| Offset | Size | Field |
|---|---|---|
| 0 | 8 | `address` — physical address, or zero when unavailable |
| 8 | 4 | `width` — horizontal resolution in pixels |
| 12 | 4 | `height` — vertical resolution in pixels |
| 16 | 4 | `stride` — pixels per scan line |
| 20 | 4 | `pixel_format` — raw `PixelFormat` value |

`PixelFormat` (raw `u32`): `Bgrx8888` = 0, `Rgbx8888` = 1, `Bitmask` = 2,
`Unavailable` = `u32::MAX`. `from_gop` maps a UEFI GOP value to the on-memory
encoding (GOP names the channel order, so GOP 0 → `Rgbx8888`). A framebuffer is
usable when `address != 0` and the format is not `Unavailable`.

## Validation

The kernel checks these invariants before dereferencing any loader pointer
(`kernel/zc-kernel/src/boot.rs`), in order:

1. `magic == BOOT_INFO_MAGIC`, else `BadMagic`.
2. `protocol_version == BOOT_PROTOCOL_VERSION`, else `UnsupportedProtocol`.
3. `memory_map_len != 0` implies `memory_map != 0`, else `MissingMemoryMap`.
4. `initramfs_len != 0` implies `initramfs_start != 0`, else `MissingInitramfs`.

Shape only: after paging is up, the architecture layer owns checking that the
physical ranges are mapped and non-overlapping.

## Kernel → task layout

The loader establishes a **0–4 GiB identity map** (2 MiB huge pages) plus a
higher-half mapping for the kernel; physical frame addresses from the allocator
are therefore directly readable below 4 GiB. Each ring-3 task then sees the
fixed per-task windows listed in
[`syscall-abi.md`](syscall-abi.md#fixed-per-task-virtual-windows). The
framebuffer is remapped for tasks at `0x10_00000` (`FB_VIRT`); the kernel's own
copy of `FramebufferInfo` keeps the physical address for the frame verifier.
