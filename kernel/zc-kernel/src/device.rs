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
//! [`zc_abi::service_cap`]), so it collides with neither.

use zc_abi::{port_cap, service_cap};

use crate::capability::{Capability, Rights};

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

/// Port grants the device manager holds at spawn: config for scanning plus
/// the broker window ([`pci_io_broker_grant`], `GRANT` only) so it can hand
/// the discovered BAR — and only the BAR — to the driver.
pub const DEVMGR_SETUP_GRANTS: usize = 2;

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
        // Manager: config to scan with, the broker window to hand over.
        assert_eq!(DEVMGR_SETUP_GRANTS, 2);
        // The broker object is its own namespace and carries GRANT without
        // WRITE, so a manager can broker a device window but never claim one.
        let broker = pci_io_broker_grant();
        assert_eq!(broker.object(), zc_abi::PORT_BROKER_OBJECT);
        assert!(broker.rights().contains(Rights::GRANT));
        assert!(!broker.rights().contains(Rights::WRITE));
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
