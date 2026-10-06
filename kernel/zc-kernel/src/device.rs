//! Spawn grant table: which roles own which hardware and services, as data.
//!
//! The kernel still provisions capabilities at spawn — a future userspace
//! device manager will serve this same table over IPC — but the grants
//! themselves live here as pure constructors, not scattered inserts. Every
//! entry is host-tested, so a wrong grant fails in `cargo test` instead of
//! as a mysterious refused claim in a boot log.
//!
//! Two namespaces share the capability object space without colliding (see
//! [`zc_abi::port_cap`]): IRQ sources are small integers, port ranges are
//! packed with the high bit set. Interrupt delivery ([`crate::irq`]) and
//! port authority ([`crate::iomap`]) therefore stay two explicit grants.
//! Service lifecycle authority lives in a third namespace, bit 30 (see
//! [`zc_abi::service_cap`]), so it collides with neither. Device memory is a
//! fourth, bit 27 (see [`zc_abi::mmio_cap`]): the kernel brokers a region it
//! validates rather than one it knows.

use zc_abi::{MemoryRegion, port_cap, service_cap};

use crate::capability::{Capability, Rights};
use crate::memory::PAGE_SIZE;

/// PCI type-1 configuration ports every bus-zero scan needs.
pub const PCI_CONFIG_START: u16 = 0xCF8;
/// Length covering both the address and data ports.
pub const PCI_CONFIG_LEN: u16 = 8;

/// 8042 data port and the bytes it spans.
pub const KBD_DATA_START: u16 = 0x60;
/// 8042 status port and the bytes it spans.
pub const KBD_STATUS_START: u16 = 0x64;

/// Capability object granting one interrupt source.
#[must_use]
pub const fn irq_grant(source: usize) -> Capability {
    Capability::new(source as u32, Rights::READ)
}

/// Capability object granting one I/O port range.
#[must_use]
pub const fn port_grant(start: u16, len: u16) -> Capability {
    Capability::new(port_cap(start, len), Rights::WRITE)
}

/// Port ranges the keyboard domain may claim, in claim order.
#[must_use]
pub const fn kbd_port_ranges() -> [(u16, u16); 2] {
    [(KBD_DATA_START, 2), (KBD_STATUS_START, 1)]
}

/// Port range any bus-zero PCI scan needs.
#[must_use]
pub const fn pci_config_range() -> (u16, u16) {
    (PCI_CONFIG_START, PCI_CONFIG_LEN)
}

/// Start of the I/O window a device manager may broker.
///
/// PCI I/O BARs are assigned by firmware above the legacy fixed devices
/// (`0x0000`–`0x03FF`: DMA, PIC, PIT, the 8042 controller). The broker window
/// starts at `0x1000` and covers the rest of the 16-bit port space, so a
/// manager can hand out a discovered BAR without the kernel knowing its
/// address — which is why the kernel no longer scans the bus to provision one.
pub const PCI_IO_WINDOW_START: u16 = 0x1000;

/// Length of the broker window; covers ports `0x1000..=0xFFFF`.
pub const PCI_IO_WINDOW_LEN: u16 = 0xF000;

/// Capability object granting a device manager broker authority over PCI I/O.
///
/// `GRANT` alone: the manager may hand a driver a narrower window it
/// discovered but cannot itself claim or touch a port in the window, so it
/// stays a pure broker and the driver's authority is exactly its own BAR. The
/// object carries no range — the window lives in [`pci_io_window_contains`],
/// because a packed port capability cannot be decoded back into a range.
#[must_use]
pub const fn pci_io_broker_grant() -> Capability {
    Capability::new(zc_abi::PORT_BROKER_OBJECT, Rights::GRANT)
}

/// Returns whether a raw `(start, len)` range lies inside the broker window.
///
/// The manager names the range it discovered as `(start << 16) | len`; the
/// kernel validates it here before minting the driver's port capability, so a
/// manager can never widen its authority past the window it was granted.
#[must_use]
pub const fn pci_io_window_contains(start: u16, len: u16) -> bool {
    if len == 0 {
        return false;
    }
    let end = start as u32 + len as u32;
    let window_end = PCI_IO_WINDOW_START as u32 + PCI_IO_WINDOW_LEN as u32;
    (start as u32) >= (PCI_IO_WINDOW_START as u32) && end <= window_end
}

/// Smallest physical address an MMIO region may name.
///
/// The low 1 MiB holds legacy devices, the BIOS, and the VGA window, none of
/// which is a PCI BAR, so a brokered range must start above it.
pub const MMIO_MIN_BASE: u64 = 0x10_0000;

/// Capability object granting a device manager broker authority over MMIO.
///
/// `GRANT` alone, mirroring [`pci_io_broker_grant`]: the manager may hand a
/// driver a device memory window it discovered but cannot itself map one, so
/// it stays a pure broker and the driver's authority is exactly its own BAR.
#[must_use]
pub const fn mmio_broker_grant() -> Capability {
    Capability::new(zc_abi::MMIO_BROKER_OBJECT, Rights::GRANT)
}

/// Returns whether two half-open ranges overlap.
#[must_use]
const fn overlaps(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> bool {
    a_start < b_end && b_start < a_end
}

/// Returns whether a discovered `(base, len)` range may be brokered as MMIO.
///
/// The kernel cannot know which BAR a device has — that is the point of
/// brokering — so it checks the property that matters instead: the range must
/// look like device memory and not like memory the kernel owns. It must be
/// page-aligned, non-empty, no larger than [`zc_abi::MMIO_MAX_BYTES`], start at
/// or above [`MMIO_MIN_BASE`], and overlap neither usable RAM nor the
/// framebuffer. A manager that names a RAM range is refused, so brokering can
/// never hand a driver the kernel's or another task's memory.
///
/// Non-usable regions other than the framebuffer are permitted: they are not
/// allocator memory, so mapping one leaks no other task's data. The device
/// manager already holds PCI config and is trusted to scan the bus.
#[must_use]
pub fn mmio_range_allowed(
    base: u64,
    len: u64,
    usable: &[MemoryRegion],
    fb_base: u64,
    fb_len: u64,
) -> bool {
    if len == 0 || len > zc_abi::MMIO_MAX_BYTES {
        return false;
    }
    if base % PAGE_SIZE != 0 || base < MMIO_MIN_BASE {
        return false;
    }
    let Some(end) = base.checked_add(len) else {
        return false;
    };
    for region in usable {
        if !region.kind.is_usable() {
            continue;
        }
        let region_end = region.start.saturating_add(region.len);
        if overlaps(base, end, region.start, region_end) {
            return false;
        }
    }
    if fb_base != 0 && fb_len != 0 {
        let fb_end = fb_base.saturating_add(fb_len);
        if overlaps(base, end, fb_base, fb_end) {
            return false;
        }
    }
    true
}

/// Capability object granting lifecycle control of one supervised service.
///
/// `READ` authorizes a status query; `WRITE` authorizes start and stop. The
/// service namespace sets bit 30 (see [`zc_abi::service_cap`]), so this
/// object cannot be confused with an IRQ source or a port range.
#[must_use]
pub const fn service_grant(id: u32) -> Capability {
    Capability::new(service_cap(id), Rights::READ.union(Rights::WRITE))
}

/// Port grants the block driver holds at spawn: none.
///
/// Its BAR window arrives at runtime, delegated by the manager over
/// [`crate::capability::CapabilityTable::delegate`]. Starting empty is the
/// point: a driver that never receives authority must fail its claim, which
/// the boot log proves in the negative only by the claim succeeding after
/// the delegation lands.
pub const BLK_SETUP_GRANTS: usize = 0;

/// Service grants the `initd` supervisor holds at spawn.
///
/// Two: authority over the block service and the keyboard service. The
/// supervisor names a service by id in the lifecycle syscalls, and the kernel
/// checks this grant before touching the slot, so a task without it cannot
/// start or stop a domain.
pub const INITD_SETUP_GRANTS: usize = 2;

/// Port and memory grants the device manager holds at spawn.
///
/// Three: config ports to scan with, the I/O broker window
/// ([`pci_io_broker_grant`]) and the MMIO broker window ([`mmio_broker_grant`]),
/// both `GRANT` only, so it can hand the discovered BARs — and only the BARs —
/// to a driver.
pub const DEVMGR_SETUP_GRANTS: usize = 3;

/// How many grants the keyboard domain always holds: two IRQs plus two ports.
pub const KBD_GRANT_COUNT: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_grants_are_exact() {
        assert_eq!(kbd_port_ranges(), [(0x60, 2), (0x64, 1)]);
        assert_eq!(pci_config_range(), (0xCF8, 8));
    }

    #[test]
    fn irq_and_port_namespaces_stay_disjoint() {
        let irq = irq_grant(zc_abi::IRQ_KEYBOARD);
        assert_eq!(irq.object(), zc_abi::IRQ_KEYBOARD as u32);
        for (start, len) in kbd_port_ranges() {
            let port = port_grant(start, len);
            assert_ne!(port.object(), irq.object());
        }
        let cfg = pci_config_range();
        assert_ne!(
            port_grant(cfg.0, cfg.1).object(),
            irq_grant(zc_abi::IRQ_KEYBOARD).object()
        );
    }

    #[test]
    fn setup_tables_match_the_boot() {
        // Block driver: nothing at spawn; its single window arrives by
        // delegation before its claim runs, or the claim fails closed.
        assert_eq!(BLK_SETUP_GRANTS, 0);
        // Manager: config to scan with, plus the I/O and MMIO broker windows.
        assert_eq!(DEVMGR_SETUP_GRANTS, 3);
        // Both broker objects are their own namespaces and carry GRANT
        // without WRITE, so a manager can broker a device window but never
        // claim or map one itself.
        let io = pci_io_broker_grant();
        assert_eq!(io.object(), zc_abi::PORT_BROKER_OBJECT);
        assert!(io.rights().contains(Rights::GRANT));
        assert!(!io.rights().contains(Rights::WRITE));
        let mmio = mmio_broker_grant();
        assert_eq!(mmio.object(), zc_abi::MMIO_BROKER_OBJECT);
        assert!(mmio.rights().contains(Rights::GRANT));
        assert!(!mmio.rights().contains(Rights::WRITE));
        assert_ne!(io.object(), mmio.object());
    }

    /// Builds a memory region for the MMIO validation tests.
    fn region(start: u64, len: u64, kind: zc_abi::MemoryKind) -> MemoryRegion {
        MemoryRegion {
            start,
            len,
            kind,
            attributes: 0,
        }
    }

    #[test]
    fn mmio_validation_refuses_anything_memory_shaped() {
        let usable = [
            region(0x10_0000, 0x1C0_0000, zc_abi::MemoryKind::Usable),
            region(0x1_0000_0000, 0x4000_0000, zc_abi::MemoryKind::Usable),
        ];
        let fb_base = 0x8000_0000;
        let fb_len = 1280 * 800 * 4;
        // A device BAR above RAM and clear of the display is accepted.
        assert!(mmio_range_allowed(0x8100_0000, 0x2_0000, &usable, fb_base, fb_len));
        assert!(mmio_range_allowed(0xE000_0000, 0x1000, &usable, fb_base, fb_len));
        // Exactly the maximum length is allowed, one byte past is not.
        assert!(mmio_range_allowed(
            0x8100_0000,
            zc_abi::MMIO_MAX_BYTES,
            &usable,
            fb_base,
            fb_len
        ));
        assert!(!mmio_range_allowed(
            0x8100_0000,
            zc_abi::MMIO_MAX_BYTES + 1,
            &usable,
            fb_base,
            fb_len
        ));
        // Usable RAM is refused, including a range that only touches its end.
        assert!(!mmio_range_allowed(0x10_0000, 0x1000, &usable, fb_base, fb_len));
        assert!(!mmio_range_allowed(0x1BF_0000, 0x2_0000, &usable, fb_base, fb_len));
        assert!(!mmio_range_allowed(0x1_0000_0000, 0x1000, &usable, fb_base, fb_len));
        // The framebuffer is refused; the byte after it is not.
        assert!(!mmio_range_allowed(fb_base, 0x1000, &usable, fb_base, fb_len));
        assert!(!mmio_range_allowed(fb_base + 0x2_0000, 0x1000, &usable, fb_base, fb_len));
        assert!(mmio_range_allowed(
            fb_base + fb_len,
            0x1000,
            &usable,
            fb_base,
            fb_len
        ));
        // Unaligned, empty, below the low 1 MiB, and overflowing are refused.
        assert!(!mmio_range_allowed(0x8100_0800, 0x1000, &usable, fb_base, fb_len));
        assert!(!mmio_range_allowed(0x8100_0000, 0, &usable, fb_base, fb_len));
        assert!(!mmio_range_allowed(0x8_0000, 0x1000, &usable, fb_base, fb_len));
        assert!(!mmio_range_allowed(u64::MAX - 0xFFF, 0x1000, &usable, fb_base, fb_len));
        // A machine with no framebuffer still validates against RAM.
        assert!(mmio_range_allowed(0x8100_0000, 0x1000, &usable, 0, 0));
    }

    #[test]
    fn the_broker_window_bounds_a_discovered_bar() {
        // A discovered BAR inside the window is accepted, its exact end too.
        assert!(pci_io_window_contains(0xC000, 0x100));
        assert!(pci_io_window_contains(0x1000, 0x100));
        assert!(pci_io_window_contains(0xFF00, 0x100));
        // Below the window, reaching past it, or empty is refused.
        assert!(!pci_io_window_contains(0x0F00, 0x100));
        assert!(!pci_io_window_contains(0xFF80, 0x100));
        assert!(!pci_io_window_contains(0xC000, 0));
        // The config ports the manager also holds are outside the window, so
        // it can never broker bus access.
        assert!(!pci_io_window_contains(PCI_CONFIG_START, PCI_CONFIG_LEN));
    }

    #[test]
    fn grant_rights_match_the_gates() {
        // The IRQ claim path checks READ, the port claim path checks WRITE;
        // the table must agree with both or every claim fails.
        assert!(irq_grant(zc_abi::IRQ_KEYBOARD).rights().contains(Rights::READ));
        assert!(irq_grant(zc_abi::IRQ_MOUSE).rights().contains(Rights::READ));
        assert!(
            port_grant(0x60, 2)
                .rights()
                .contains(Rights::WRITE)
        );
        assert_eq!(KBD_GRANT_COUNT, 4);
    }

    #[test]
    fn service_namespace_stays_disjoint() {
        let service = service_grant(0);
        assert_eq!(service.object(), service_cap(0));
        // Bit 30 keeps a service id apart from both small IRQ sources and
        // bit-31 port ranges, so one object can never gate another path.
        assert_ne!(service.object(), irq_grant(zc_abi::IRQ_KEYBOARD).object());
        assert_ne!(service.object(), port_grant(0x60, 2).object());
        // The supervisor may query status and start/stop its services.
        assert!(service.rights().contains(Rights::READ));
        assert!(service.rights().contains(Rights::WRITE));
        // One grant per supervised service: block and keyboard.
        assert_eq!(INITD_SETUP_GRANTS, 2);
    }
}
