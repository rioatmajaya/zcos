//! Device grant table: which roles own which hardware, as data.
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

use zc_abi::port_cap;

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

/// Port range covering a virtio BAR window once its base is known.
#[must_use]
pub const fn bar_range(base: u16) -> (u16, u16) {
    (base, 0x100)
}

/// Port grants the block driver holds at spawn: none.
///
/// Its BAR window arrives at runtime, delegated by the manager over
/// [`crate::capability::CapabilityTable::delegate`]. Starting empty is the
/// point: a driver that never receives authority must fail its claim, which
/// the boot log proves in the negative only by the claim succeeding after
/// the delegation lands.
pub const BLK_SETUP_GRANTS: usize = 0;

/// Port grants the device manager holds at spawn: config for scanning plus
/// the discovered BAR window with [`crate::capability::Rights::GRANT`] so it
/// can hand the window — and only the window — to the driver.
pub const DEVMGR_SETUP_GRANTS: usize = 2;

/// How many grants the keyboard domain always holds: one IRQ plus two ports.
pub const KBD_GRANT_COUNT: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_grants_are_exact() {
        assert_eq!(kbd_port_ranges(), [(0x60, 2), (0x64, 1)]);
        assert_eq!(pci_config_range(), (0xCF8, 8));
        assert_eq!(bar_range(0x6080), (0x6080, 0x100));
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
        // Manager: config to scan with, BAR-with-grant to hand over.
        assert_eq!(DEVMGR_SETUP_GRANTS, 2);
        let bar = bar_range(0x6080);
        assert_ne!(bar, pci_config_range());
        assert_eq!(port_grant(bar.0, bar.1).object(), port_cap(0x6080, 0x100));
    }

    #[test]
    fn grant_rights_match_the_gates() {
        // The IRQ claim path checks READ, the port claim path checks WRITE;
        // the table must agree with both or every claim fails.
        assert!(irq_grant(zc_abi::IRQ_KEYBOARD).rights().contains(Rights::READ));
        assert!(
            port_grant(0x60, 2)
                .rights()
                .contains(Rights::WRITE)
        );
        assert_eq!(KBD_GRANT_COUNT, 3);
    }
}
