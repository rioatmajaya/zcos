//! Application-processor boot trampoline as position-independent bytes.
//!
//! An AP starts in 16-bit real mode at the SIPI vector page and must reach
//! 64-bit long mode on its own: the trampoline below enters protected mode,
//! enables PAE, long mode, and paging on the BSP's tables, loads a 64-bit
//! code segment, then jumps to a Rust entry point. The BSP copies [`CODE`]
//! into a sub-megabyte frame and fills the data fields at the [`OFF_*`]
//! offsets; nothing in the blob depends on where the frame lands except
//! RIP-relative references the CPU resolves at run time.
//!
//! Layout inside the frame (all offsets from the frame base):
//! `0x00` 16-bit entry, `0x20` 32-bit entry, `0x56` 64-bit entry,
//! `0x6C` long-mode fixups, data at `0x100`, temporary 32-bit GDT at
//! `0x128`. Total footprint [`FRAME_USE`].

/// First byte the AP executes.
pub const ENTRY_16: usize = 0x00;

/// 32-bit protected-mode entry, target of the first far jump.
pub const ENTRY_32: usize = 0x21;

/// 64-bit long-mode entry, target of the second far jump.
pub const ENTRY_64: usize = 0x57;

/// Offset of the BSP's page-table root (u32).
pub const OFF_CR3: usize = 0x100;

/// Offset of the 10-byte kernel GDTR (limit u16, base u64).
pub const OFF_GDTR64: usize = 0x104;

/// Offset of the AP stack top (u64).
pub const OFF_STACK: usize = 0x110;

/// Offset of the Rust AP entry point (u64).
pub const OFF_ENTRY: usize = 0x118;

/// Offset of the 6-byte temporary GDTR (limit u16, base u32).
pub const OFF_GDTR32: usize = 0x120;

/// Offset of the temporary 32-bit GDT (four u64 entries).
pub const OFF_GDT32: usize = 0x128;

/// Bytes the frame uses in total (code, data, and temporary GDT).
pub const FRAME_USE: usize = 0x148;

/// Machine code from [`ENTRY_16`] through the end of the fixups.
pub const CODE: &[u8] = &[
    0xFA, // 0x00 cli
    0x8C, 0xC8, // mov ax,cs
    0x8E, 0xD8, // mov ds,ax
    0x0F, 0x01, 0x16, 0x20, 0x01, // lgdt [0x120]
    0x66, 0x8B, 0x1E, 0x00, 0x01, // mov ebx,[0x100]
    0x0F, 0x20, 0xC0, // mov eax,cr0
    0x66, 0x83, 0xC8, 0x01, // or eax,1
    0x0F, 0x22, 0xC0, // mov cr0,eax
    0x66, 0xEA, 0x21, 0x00, 0x00, 0x00, 0x08, 0x00, // jmp 0x08:0x21
    0x66, 0xB8, 0x10, 0x00, // 0x21 mov ax,0x10
    0x8E, 0xD8, // mov ds,ax
    0x8E, 0xC0, // mov es,ax
    0x8E, 0xD0, // mov ss,ax
    0x0F, 0x22, 0xD8, // mov cr3,ebx
    0x0F, 0x20, 0xE0, // mov eax,cr4
    0x83, 0xC8, 0x20, // or eax,0x20 (PAE)
    0x0F, 0x22, 0xE0, // mov cr4,eax
    0xB9, 0x80, 0x00, 0x00, 0xC0, // mov ecx,0xC0000080
    0x0F, 0x32, // rdmsr
    0x0D, 0x00, 0x01, 0x00, 0x00, // or eax,0x100 (LME)
    0x0F, 0x30, // wrmsr
    0x0F, 0x20, 0xC0, // mov eax,cr0
    0x0D, 0x01, 0x00, 0x00, 0x80, // or eax,0x80000001 (PG+PE)
    0x0F, 0x22, 0xC0, // mov cr0,eax
    0xEA, 0x57, 0x00, 0x00, 0x00, 0x18, 0x00, // jmp 0x18:0x57
    0x48, 0x8D, 0x05, 0xA6, 0x00, 0x00, 0x00, // 0x57 lea rax,[rel 0x104]
    0x0F, 0x01, 0x00, // lgdt [rax]
    0x6A, 0x08, // push 0x08
    0x48, 0x8D, 0x05, 0x04, 0x00, 0x00, 0x00, // lea rax,[rel 0x6d]
    0x50, // push rax
    0x48, 0xCB, // retfq
    0x66, 0xB8, 0x10, 0x00, // 0x6d mov ax,0x10
    0x8E, 0xD8, // mov ds,ax
    0x8E, 0xC0, // mov es,ax
    0x8E, 0xD0, // mov ss,ax
    0x8E, 0xE0, // mov fs,ax
    0x8E, 0xE8, // mov gs,ax
    0x48, 0x8B, 0x25, 0x8E, 0x00, 0x00, 0x00, // mov rsp,[rel 0x110]
    0xFF, 0x25, 0x90, 0x00, 0x00, 0x00, // jmp [rel 0x118]
];

/// 32-bit code segment for the temporary GDT.
pub const GDT32_CODE: u64 = 0x00CF_9A00_0000_FFFF;

/// 32-bit data segment for the temporary GDT.
pub const GDT32_DATA: u64 = 0x00CF_9200_0000_FFFF;

/// 64-bit code segment for the temporary GDT (used for the long jump).
pub const GDT64_CODE: u64 = 0x00AF_9A00_0000_FFFF;

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads a little-endian `u32` from the blob.
    const fn read_u32(offset: usize) -> u32 {
        (CODE[offset] as u32)
            | ((CODE[offset + 1] as u32) << 8)
            | ((CODE[offset + 2] as u32) << 16)
            | ((CODE[offset + 3] as u32) << 24)
    }

    #[test]
    fn blob_layout_matches_data_offsets() {
        assert_eq!(CODE.len(), 0x88);
        assert!(OFF_ENTRY + 8 <= OFF_GDTR32);
        assert!(OFF_GDT32 + 32 <= FRAME_USE);
        assert_eq!(ENTRY_16, 0);
    }

    #[test]
    fn real_mode_entry_disables_interrupts_first() {
        assert_eq!(CODE[ENTRY_16], 0xFA);
    }

    #[test]
    fn far_jumps_target_the_mode_entries() {
        // jmp 0x08:off32 at the end of the 16-bit part.
        let at = 0x19;
        assert_eq!(CODE[at], 0x66);
        assert_eq!(CODE[at + 1], 0xEA);
        assert_eq!(read_u32(at + 2), ENTRY_32 as u32);
        assert_eq!(CODE[at + 6], 0x08);
        // jmp 0x18:off32 at the end of the 32-bit part.
        let at = 0x50;
        assert_eq!(CODE[at], 0xEA);
        assert_eq!(read_u32(at + 1), ENTRY_64 as u32);
        assert_eq!(CODE[at + 5], 0x18);
        assert_eq!(CODE[at + 6], 0x00);
    }

    #[test]
    fn rip_relative_loads_resolve_to_data() {
        // lea rax,[rel gdtr64] at 0x57 spans seven bytes.
        let end = 0x57 + 7;
        let disp = read_u32(0x57 + 3);
        assert_eq!(end + disp as usize, OFF_GDTR64);
        // mov rsp,[rel stack] at 0x7b spans seven bytes.
        let end = 0x7B + 7;
        let disp = read_u32(0x7B + 3);
        assert_eq!(end + disp as usize, OFF_STACK);
        // jmp [rel entry] at 0x82 spans six bytes.
        let end = 0x82 + 6;
        let disp = read_u32(0x82 + 2);
        assert_eq!(end + disp as usize, OFF_ENTRY);
    }

    #[test]
    fn temporary_gdt_covers_all_modes() {
        assert_eq!(GDT32_CODE >> 53 & 1, 0);
        assert_eq!(GDT64_CODE >> 53 & 1, 1);
        assert_eq!(GDT32_DATA >> 45 & 3, 0);
    }
}
