//! CPU trap vectors and IDT entry construction.
//!
//! This module models traps as data: vector numbers, which vectors push an
//! error code, human-readable names, and the 16-byte IDT gate layout. Writing
//! the IDT and executing `lidt` stays in the bare-metal image, which owns
//! all privileged instructions.

/// Timer interrupt vector used by the APIC.
pub const TIMER_VECTOR: u8 = 32;

/// Spurious-interrupt vector programmed into the local APIC.
pub const SPURIOUS_VECTOR: u8 = 0xFF;

/// Keyboard interrupt vector for ISA IRQ1 through the I/O APIC.
pub const KBD_VECTOR: u8 = 0x21;

/// Mouse interrupt vector for ISA IRQ12 through the I/O APIC.
pub const MOUSE_VECTOR: u8 = 0x22;

/// Software-interrupt vector userspace raises for syscalls.
///
/// Its gate uses DPL 3 so ring-3 code may invoke it; every other gate stays
/// at DPL 0.
pub const SYSCALL_VECTOR: u8 = 0x80;

/// Vectors whose handlers receive a CPU-pushed error code.
pub const ERROR_CODE_VECTORS: [u8; 8] = [8, 10, 11, 12, 13, 14, 17, 21];

/// Returns whether the CPU pushes an error code for `vector`.
#[must_use]
pub const fn has_error_code(vector: u8) -> bool {
    let mut index = 0;
    while index < ERROR_CODE_VECTORS.len() {
        if ERROR_CODE_VECTORS[index] == vector {
            return true;
        }
        index += 1;
    }
    false
}

/// Returns the conventional name of a CPU exception vector.
#[must_use]
pub const fn name(vector: u8) -> &'static str {
    match vector {
        0 => "divide error",
        1 => "debug",
        2 => "non-maskable interrupt",
        3 => "breakpoint",
        4 => "overflow",
        5 => "bound range exceeded",
        6 => "invalid opcode",
        7 => "device not available",
        8 => "double fault",
        9 => "coprocessor segment overrun",
        10 => "invalid TSS",
        11 => "segment not present",
        12 => "stack-segment fault",
        13 => "general protection",
        14 => "page fault",
        15 => "reserved",
        16 => "x87 floating-point",
        17 => "alignment check",
        18 => "machine check",
        19 => "SIMD floating-point",
        20 => "virtualization",
        21 => "control protection",
        22..=31 => "reserved",
        _ => "external interrupt",
    }
}

/// IDT gate type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateType {
    /// Interrupt gate: the CPU clears IF on entry.
    Interrupt,
    /// Trap gate: the CPU leaves IF unchanged on entry.
    Trap,
}

impl GateType {
    /// Returns the type-attribute field value.
    #[must_use]
    pub const fn bits(self) -> u64 {
        match self {
            Self::Interrupt => 0xE,
            Self::Trap => 0xF,
        }
    }
}

/// One 16-byte Interrupt Descriptor Table entry.
///
/// Stored as two `u64` halves so the bare-metal image can write it with two
/// volatile stores.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdtEntry {
    halves: [u64; 2],
}

impl IdtEntry {
    /// A non-present entry that the CPU treats as unconfigured.
    pub const EMPTY: Self = Self { halves: [0, 0] };

    /// Builds a gate for `handler` with the given segment and attributes.
    ///
    /// `ist` selects the Interrupt Stack Table slot (0 means the current
    /// stack); `dpl` is the minimum privilege level allowed to invoke the
    /// gate.
    #[must_use]
    pub const fn new(
        handler: u64,
        selector: u16,
        ist: u8,
        gate: GateType,
        dpl: u8,
        present: bool,
    ) -> Self {
        let present_bit = if present { 1u64 } else { 0 };
        let attribute = (present_bit << 7) | (((dpl as u64) & 3) << 5) | gate.bits();
        let low = (handler & 0xFFFF)
            | ((selector as u64) << 16)
            | (((ist as u64) & 0x7) << 32)
            | (attribute << 40)
            | (((handler >> 16) & 0xFFFF) << 48);
        let high = (handler >> 32) & 0xFFFF_FFFF;
        Self { halves: [low, high] }
    }

    /// Returns the handler address encoded in this entry.
    #[must_use]
    pub const fn handler(self) -> u64 {
        (self.halves[0] & 0xFFFF)
            | ((self.halves[0] >> 48) << 16)
            | (self.halves[1] << 32)
    }

    /// Returns the code-segment selector encoded in this entry.
    #[must_use]
    pub const fn selector(self) -> u16 {
        ((self.halves[0] >> 16) & 0xFFFF) as u16
    }

    /// Returns whether the present bit is set.
    #[must_use]
    pub const fn is_present(self) -> bool {
        (self.halves[0] >> 47) & 1 == 1
    }

    /// Returns the gate type encoded in this entry.
    #[must_use]
    pub const fn gate(self) -> u64 {
        (self.halves[0] >> 40) & 0xF
    }

    /// Returns the raw halves for volatile stores.
    #[must_use]
    pub const fn halves(self) -> [u64; 2] {
        self.halves
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_table_covers_protected_mode_faults() {
        for vector in [8u8, 10, 11, 12, 13, 14, 17, 21] {
            assert!(has_error_code(vector), "vector {vector}");
        }
        assert!(!has_error_code(0));
        assert!(!has_error_code(6));
        assert!(!has_error_code(7));
        assert!(!has_error_code(32));
        assert!(!has_error_code(255));
        assert_eq!(ERROR_CODE_VECTORS.len(), 8);
    }

    #[test]
    fn vector_names_cover_cpu_exceptions() {
        assert_eq!(name(0), "divide error");
        assert_eq!(name(6), "invalid opcode");
        assert_eq!(name(13), "general protection");
        assert_eq!(name(14), "page fault");
        assert_eq!(name(21), "control protection");
        assert_eq!(name(22), "reserved");
        assert_eq!(name(31), "reserved");
        assert_eq!(name(32), "external interrupt");
    }

    #[test]
    fn idt_entry_round_trips_handler_and_attributes() {
        let entry = IdtEntry::new(
            0xFFFF_FFFF_8001_2345,
            0x08,
            0,
            GateType::Interrupt,
            0,
            true,
        );
        assert_eq!(entry.handler(), 0xFFFF_FFFF_8001_2345);
        assert_eq!(entry.selector(), 0x08);
        assert!(entry.is_present());
        assert_eq!(entry.gate(), 0xE);
        assert_eq!(entry.halves().len(), 2);
    }

    #[test]
    fn empty_entry_is_not_present() {
        assert!(!IdtEntry::EMPTY.is_present());
        assert_eq!(IdtEntry::EMPTY.handler(), 0);
    }

    #[test]
    fn trap_gate_and_dpl_are_encoded() {
        let entry = IdtEntry::new(0x1000, 0x1B, 1, GateType::Trap, 3, true);
        assert_eq!(entry.gate(), 0xF);
        // type_attr byte: P=1 DPL=3 gate=0xF -> 0xEF.
        let attribute = (entry.halves()[0] >> 40) & 0xFF;
        assert_eq!(attribute, 0xEF);
    }

    #[test]
    fn well_known_vectors_are_stable() {
        assert_eq!(TIMER_VECTOR, 32);
        assert_eq!(KBD_VECTOR, 0x21);
        assert_eq!(MOUSE_VECTOR, 0x22);
        assert_eq!(SPURIOUS_VECTOR, 0xFF);
        assert_eq!(SYSCALL_VECTOR, 0x80);
        assert!(!has_error_code(SYSCALL_VECTOR));
    }
}
