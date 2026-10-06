//! Bring-up driver-domain contract between kernel and driver tasks.
//!
//! Until capabilities convey resources directly, the kernel publishes a
//! small fixed area per driver domain: three contiguous frames for DMA at
//! [`QUEUE_VIRT`] and one descriptor page at [`INFO_VIRT`] holding their
//! physical addresses. Everything here is provisional and will move into
//! capability invocations once the object model grows.

/// User address of the driver's DMA queue area (three pages).
pub const QUEUE_VIRT: u64 = 0x45_0000;

/// User address of the driver descriptor page (one page).
pub const INFO_VIRT: u64 = 0x45_3000;

/// User address of the filesystem exchange page (one page).
///
/// The kernel filesystem proxy and the block domain share this page instead of
/// copying through IPC messages, which carry only four words. It sits directly
/// above [`INFO_VIRT`] and well below the next task image at `0x46_0000`, so a
/// driver image can grow into the queue area but never into this page without
/// tripping the bound check the block domain runs at start-up.
pub const FS_EXCHANGE_VIRT: u64 = 0x45_4000;

/// Length in bytes of the filesystem exchange page.
pub const FS_EXCHANGE_LEN: usize = 4096;

/// Offset of the request opcode or reply status (u32).
pub const FS_EXCHANGE_OP: usize = 0;
/// Offset of the request/reply sequence number (u32).
pub const FS_EXCHANGE_SEQ: usize = 4;
/// Offset of the requesting task index (u32).
pub const FS_EXCHANGE_TASK: usize = 8;
/// Offset of the payload length in bytes (u32).
pub const FS_EXCHANGE_PAYLOAD: usize = 12;
/// Offset of the node id (u64).
pub const FS_EXCHANGE_NODE: usize = 16;
/// Offset of the file offset (u64).
pub const FS_EXCHANGE_OFFSET: usize = 24;
/// Offset of the result value: bytes read or written, or a node id (u64).
pub const FS_EXCHANGE_RESULT: usize = 32;
/// Offset of the request payload or reply bytes.
pub const FS_EXCHANGE_DATA: usize = 40;

/// Largest payload that fits in the exchange page.
pub const FS_EXCHANGE_DATA_MAX: usize = FS_EXCHANGE_LEN - FS_EXCHANGE_DATA;

/// Filesystem id `SYS_MOUNT` accepts for the ZC-native zcfs volume.
pub const FS_ID_ZCFS: u64 = 1;

/// Request opcode: resolve a name inside a directory.
pub const FS_OP_LOOKUP: u32 = 1;
/// Request opcode: return a node's kind, mode, and size.
pub const FS_OP_STAT: u32 = 2;
/// Request opcode: read bytes from a node.
pub const FS_OP_READ: u32 = 3;
/// Request opcode: write bytes into a node.
pub const FS_OP_WRITE: u32 = 4;
/// Request opcode: create a name inside a directory.
pub const FS_OP_CREATE: u32 = 5;
/// Request opcode: write back and flush the device.
pub const FS_OP_FLUSH: u32 = 6;
/// Request opcode: mount the volume, replaying the log with a cold cache.
pub const FS_OP_MOUNT: u32 = 7;
/// Request opcode: flush, then drop the replayed table.
pub const FS_OP_UNMOUNT: u32 = 8;
/// Request opcode: flush and stop serving; the domain then exits.
pub const FS_OP_STOP: u32 = 9;

/// Reply status: the operation succeeded.
pub const FS_STATUS_OK: u32 = 0;
/// Reply status: the node or name does not exist.
pub const FS_STATUS_NOT_FOUND: u32 = 1;
/// Reply status: a path component is not a directory.
pub const FS_STATUS_NOT_A_DIRECTORY: u32 = 2;
/// Reply status: the path is malformed or the name is too long.
pub const FS_STATUS_BAD_PATH: u32 = 3;
/// Reply status: the filesystem does not support the operation.
pub const FS_STATUS_NOT_SUPPORTED: u32 = 4;
/// Reply status: the on-disk data is inconsistent.
pub const FS_STATUS_CORRUPT: u32 = 5;
/// Reply status: the node table has no free slot.
pub const FS_STATUS_TABLE_FULL: u32 = 6;
/// Reply status: the descriptor is invalid.
pub const FS_STATUS_BAD_FD: u32 = 7;
/// Reply status: a buffer argument is malformed.
pub const FS_STATUS_BAD_BUFFER: u32 = 8;
/// Reply status: the volume has no room for the write.
pub const FS_STATUS_NO_SPACE: u32 = 9;
/// Reply status: the block device reported an error.
pub const FS_STATUS_IO: u32 = 10;

/// Interrupt source index of the PS/2 keyboard line.
///
/// Sources are indices into the kernel's IRQ table, not CPU vectors: the
/// vector that carries the interrupt is a kernel implementation detail.
pub const IRQ_KEYBOARD: usize = 0;

/// Interrupt source index of the PS/2 mouse line.
///
/// The mouse shares the 8042 controller with the keyboard, so the same input
/// domain claims both sources; bytes are tagged by the controller's aux bit.
pub const IRQ_MOUSE: usize = 1;

/// Total number of interrupt sources the kernel routes.
pub const IRQ_SOURCES: usize = 5;

/// Sentinel source for [`SYS_IRQ_WAIT`] meaning "wake on any source I own".
///
/// A driver that claims several lines (such as the input domain, which owns
/// both the keyboard and mouse) has no use for a per-line wakeup: the 8042 is
/// drained wholesale on every interrupt, so whichever line fired, the work is
/// the same. Passing `IRQ_ANY` blocks until any owned source has a pending
/// interrupt and clears every owned count at once, so the next call blocks
/// again until genuinely new input arrives. It is `u64::MAX` so it can never
/// collide with a real source index.
pub const IRQ_ANY: u64 = u64::MAX;

/// User address of the input driver domain's ring.
///
/// One page mapped into that domain only, past the last task image so the
/// two can never share a page. The driver appends translated ASCII here; the
/// kernel's serial-read path drains it, so the shell sees one input stream
/// regardless of which device the keystroke arrived on. Mouse reports travel
/// in the same ring as tag-framed bytes the drain strips before the shell.
pub const INPUT_RING_VIRT: u64 = 0x47_0000;

/// Packs an I/O port range into a capability object id.
///
/// The high bit tags the port namespace so a port grant can never collide
/// with an IRQ source index (which is a small integer): `port_cap` values
/// always have bit 31 set, IRQ sources never do. The kernel provisions one
/// such object per granted range at spawn, and `SYS_PORT_CLAIM` checks the
/// caller's table for the exact packed value before touching the bitmap.
#[must_use]
pub const fn port_cap(start: u16, len: u16) -> u32 {
    0x8000_0000 | ((start as u32) << 16) | (len as u32)
}

/// Capability object id granting authority to broker I/O port windows.
///
/// A device manager holds this object with `GRANT` rights and hands a driver
/// a range it discovered. It lives in its own namespace (bit 28), so it can
/// never be mistaken for an IRQ source, a port range, a service, or a surface.
///
/// The brokered range travels as a raw `(start << 16) | len` word rather than
/// a packed [`port_cap`], because a port capability's bit 31 doubles as both
/// the namespace tag and the top bit of `start`, so it cannot be decoded back
/// into a range. The kernel validates the raw range against the broker window
/// and constructs the driver's capability itself.
pub const PORT_BROKER_OBJECT: u32 = 0x1000_0001;

/// Offset of the first queue-frame physical address in the descriptor.
pub const INFO_QUEUE0: usize = 0;
/// Offset of the second queue-frame physical address.
pub const INFO_QUEUE1: usize = 8;
/// Offset of the third queue-frame physical address.
pub const INFO_QUEUE2: usize = 16;
/// Size of the descriptor page payload.
pub const INFO_LEN: usize = 24;

/// User address of a driver domain's coherent DMA window.
///
/// Starts exactly at [`crate::SURFACE_END`], so it clears the framebuffer and
/// surface windows and stays inside the first page directory. The kernel backs
/// it with physically contiguous frames at spawn and publishes the device
/// address at [`DMA_INFO_VIRT`], which is what a driver hands to hardware.
pub const DMA_VIRT: u64 = 0x24_00000;

/// Size in bytes of the delegated DMA window (64 KiB, 16 frames).
///
/// Deliberately modest: the allocator hands out a contiguous run at spawn, and
/// a smaller run is far less likely to fail on fragmentation. It is ample for
/// the descriptor rings and packet buffers a bring-up driver needs.
pub const DMA_WINDOW_BYTES: u64 = 0x1_0000;

/// User address of the DMA descriptor page (one page).
///
/// Sits directly above the filesystem exchange page and below the keyboard
/// image at `0x46_0000`, matching the layout of the other driver pages.
pub const DMA_INFO_VIRT: u64 = 0x45_5000;

/// Describes a delegated DMA window to its holder.
///
/// `phys` is the window's physical base, which a driver programs into a device
/// as the DMA target; the same bytes are reachable through [`DMA_VIRT`]. The
/// two are the same frames, so a buffer written through the virtual alias is
/// what the device reads.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaInfo {
    /// Physical base address of the window (device-visible).
    pub phys: u64,
    /// Length of the window in bytes.
    pub len: u64,
}

impl DmaInfo {
    /// An empty descriptor used when no window was delegated.
    pub const UNAVAILABLE: Self = Self { phys: 0, len: 0 };

    /// Returns whether this descriptor names a delegated window.
    #[must_use]
    pub const fn is_available(self) -> bool {
        self.phys != 0 && self.len != 0
    }
}

/// First word a driver writes into its DMA window for the coherence proof.
///
/// The driver writes it through the window's virtual alias and the kernel
/// reads it back through the window's physical address; a match proves the
/// alias and the device-visible address are the same frames.
pub const DMA_MAGIC0: u32 = 0xD0A0_0001;

/// Second proof word, one page into the window.
///
/// Written and read exactly like [`DMA_MAGIC0`] but at a different frame, so a
/// match also proves the whole run — not just its first frame — is contiguous.
pub const DMA_MAGIC1: u32 = 0xD0A0_0002;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_areas_do_not_overlap() {
        assert_eq!(QUEUE_VIRT + 3 * 4096, INFO_VIRT);
        assert!(INFO_LEN <= 4096);
        assert_eq!(INFO_QUEUE2 + 8, INFO_LEN);
    }

    #[test]
    fn the_exchange_page_clears_every_neighbour() {
        // Directly above the descriptor page, and below the keyboard image.
        assert_eq!(INFO_VIRT + 4096, FS_EXCHANGE_VIRT);
        assert!(FS_EXCHANGE_VIRT + FS_EXCHANGE_LEN as u64 <= 0x46_0000);
        // Every field, and the payload, stays inside the page.
        assert!(FS_EXCHANGE_RESULT + 8 <= FS_EXCHANGE_DATA);
        assert_eq!(FS_EXCHANGE_DATA_MAX, 4056);
        assert_eq!(FS_EXCHANGE_DATA + FS_EXCHANGE_DATA_MAX, FS_EXCHANGE_LEN);
    }

    #[test]
    fn fs_opcodes_and_statuses_are_distinct() {
        assert_eq!(FS_ID_ZCFS, 1);
        let ops = [
            FS_OP_LOOKUP,
            FS_OP_STAT,
            FS_OP_READ,
            FS_OP_WRITE,
            FS_OP_CREATE,
            FS_OP_FLUSH,
            FS_OP_MOUNT,
            FS_OP_UNMOUNT,
            FS_OP_STOP,
        ];
        for (index, op) in ops.iter().enumerate() {
            assert_eq!(*op as usize, index + 1);
        }
        let statuses = [
            FS_STATUS_OK,
            FS_STATUS_NOT_FOUND,
            FS_STATUS_NOT_A_DIRECTORY,
            FS_STATUS_BAD_PATH,
            FS_STATUS_NOT_SUPPORTED,
            FS_STATUS_CORRUPT,
            FS_STATUS_TABLE_FULL,
            FS_STATUS_BAD_FD,
            FS_STATUS_BAD_BUFFER,
            FS_STATUS_NO_SPACE,
            FS_STATUS_IO,
        ];
        for (index, status) in statuses.iter().enumerate() {
            assert_eq!(*status as usize, index);
        }
    }

    #[test]
    fn input_ring_sits_past_every_task_image() {
        // Task images are linked every 64 KiB from 0x400000; the ring must
        // clear the keyboard domain at 0x460000 so a large image cannot reach
        // the shared page. (The device manager links higher at 0x480000 and
        // grows upward, away from the ring, toward its own discovery page.)
        assert!(INPUT_RING_VIRT >= 0x47_0000);
        assert!(INPUT_RING_VIRT + 4096 <= 0x60_0000);
    }

    #[test]
    fn keyboard_source_is_inside_the_table() {
        assert!(IRQ_KEYBOARD < IRQ_SOURCES);
        assert!(IRQ_MOUSE < IRQ_SOURCES);
        assert_ne!(IRQ_KEYBOARD, IRQ_MOUSE);
    }

    #[test]
    fn irq_any_can_never_collide_with_a_real_source() {
        assert_eq!(IRQ_ANY, u64::MAX);
        assert!((IRQ_ANY as usize) >= IRQ_SOURCES);
    }

    #[test]
    fn port_caps_never_collide_with_irq_sources() {
        // Every IRQ source index must stay outside the port namespace, even
        // for the degenerate (0, 0) range.
        for source in 0..IRQ_SOURCES as u32 {
            assert_ne!(port_cap(0, 0), source);
            assert_ne!(port_cap(0, 1), source);
            assert_ne!(port_cap(0x60, 2), source);
            assert_ne!(port_cap(0xCF8, 8), source);
        }
        // Packing is injective over the ranges the kernel actually grants.
        assert_ne!(port_cap(0x60, 2), port_cap(0x64, 1));
        assert_ne!(port_cap(0xCF8, 8), port_cap(0xC000, 0x100));
        assert_eq!(port_cap(0x60, 2), 0x8060_0002);
    }

    #[test]
    fn broker_object_stays_in_its_own_namespace() {
        // Bit 28 only: clear of IRQ sources, the port tag (31), services (30),
        // and surfaces (29), so the broker can never collide with a grant.
        assert_eq!(PORT_BROKER_OBJECT & 0x8000_0000, 0);
        assert_eq!(PORT_BROKER_OBJECT & 0x4000_0000, 0);
        assert_eq!(PORT_BROKER_OBJECT & 0x2000_0000, 0);
        assert_ne!(PORT_BROKER_OBJECT, port_cap(0, 1));
        assert_ne!(PORT_BROKER_OBJECT, crate::service_cap(0));
        assert_ne!(PORT_BROKER_OBJECT, crate::SURFACE_FACTORY);
    }

    #[test]
    fn the_dma_window_clears_its_neighbours() {
        // Starts where the surface window ends, and is page-sized.
        assert_eq!(DMA_VIRT, crate::SURFACE_END);
        assert_eq!(DMA_VIRT % 4096, 0);
        assert_eq!(DMA_WINDOW_BYTES % 4096, 0);
        assert_eq!(DMA_WINDOW_BYTES / 4096, 16);
        // The descriptor page follows the exchange page and stays inside the
        // image window, below the keyboard image at 0x46_0000.
        assert_eq!(FS_EXCHANGE_VIRT + FS_EXCHANGE_LEN as u64, DMA_INFO_VIRT);
        assert!(DMA_INFO_VIRT + 4096 <= 0x46_0000);
    }

    #[test]
    fn dma_proof_words_are_distinct_and_nonzero() {
        // A zero word would match a freshly zeroed window, so the proof must
        // use non-zero words; distinct words also catch a write that lands in
        // the wrong frame.
        assert_ne!(DMA_MAGIC0, 0);
        assert_ne!(DMA_MAGIC1, 0);
        assert_ne!(DMA_MAGIC0, DMA_MAGIC1);
    }

    #[test]
    fn dma_info_layout_is_stable() {
        use core::mem::{offset_of, size_of};
        assert_eq!(size_of::<DmaInfo>(), 16);
        assert_eq!(offset_of!(DmaInfo, phys), 0);
        assert_eq!(offset_of!(DmaInfo, len), 8);
        assert!(!DmaInfo::UNAVAILABLE.is_available());
        let delegated = DmaInfo {
            phys: 0x1000,
            len: DMA_WINDOW_BYTES,
        };
        assert!(delegated.is_available());
    }
}
