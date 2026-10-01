//! The first executable loaded by UEFI for ZC OS.
//!
//! The loader intentionally starts without a third-party runtime so its UEFI
//! boundary remains explicit. It currently verifies that firmware can invoke
//! the EFI entry point and prints an early diagnostic. Kernel ELF loading,
//! memory-map capture, and `ExitBootServices` follow in the next increment.

#![cfg_attr(target_os = "uefi", no_main)]
#![no_std]
#![allow(unsafe_code)] // UEFI protocol calls require raw firmware pointers.

#[cfg(not(target_os = "uefi"))]
extern crate std;

use core::ffi::c_void;

use zc_abi::BOOT_PROTOCOL_VERSION;

/// UEFI status values use the most significant bit to mark an error.
type EfiStatus = usize;

/// EFI handle type. The value is opaque to the loader.
type EfiHandle = *mut c_void;

/// UEFI status-returning function used by the simple text output protocol.
type OutputString =
    unsafe extern "efiapi" fn(*mut SimpleTextOutputProtocol, *const u16) -> EfiStatus;

/// Common header included at the start of UEFI tables.
#[repr(C)]
struct TableHeader {
    signature: u64,
    revision: u32,
    header_size: u32,
    crc32: u32,
    reserved: u32,
}

/// The UEFI Simple Text Output Protocol fields used for early diagnostics.
#[repr(C)]
struct SimpleTextOutputProtocol {
    reset: *const c_void,
    output_string: OutputString,
    test_string: *const c_void,
    query_mode: *const c_void,
    set_mode: *const c_void,
    set_attribute: *const c_void,
    clear_screen: *const c_void,
    set_cursor_position: *const c_void,
    enable_cursor: *const c_void,
    mode: *mut c_void,
}

/// Prefix of `EFI_SYSTEM_TABLE` through the console-output pointer.
///
/// The UEFI specification fixes this layout. Fields not needed before kernel
/// loading are represented as opaque pointers to avoid exposing them early.
#[repr(C)]
struct SystemTable {
    header: TableHeader,
    firmware_vendor: *const u16,
    firmware_revision: u32,
    console_in_handle: EfiHandle,
    con_in: *mut c_void,
    console_out_handle: EfiHandle,
    con_out: *mut SimpleTextOutputProtocol,
}

/// UEFI entry point used by firmware to start the ZC OS loader.
///
/// This is deliberately the only exported symbol. Returning an EFI error lets
/// firmware surface an invalid system-table pointer instead of dereferencing it.
///
/// # Safety
///
/// The caller must follow the UEFI specification: `system_table` must point to
/// a valid `EFI_SYSTEM_TABLE` that remains valid while this function executes.
#[unsafe(no_mangle)]
#[allow(private_interfaces)] // UEFI calls this symbol, not Rust callers.
pub unsafe extern "efiapi" fn efi_main(
    _image_handle: EfiHandle,
    system_table: *mut SystemTable,
) -> EfiStatus {
    if system_table.is_null() {
        return 0x8000_0000_0000_0002;
    }

    // SAFETY: UEFI invokes `efi_main` with a valid EFI_SYSTEM_TABLE pointer.
    let con_out = unsafe { (*system_table).con_out };
    if con_out.is_null() {
        return 0x8000_0000_0000_0002;
    }

    let message = boot_banner();
    // SAFETY: `con_out` is validated above, and `message` is NUL-terminated
    // UTF-16 storage that remains live for the duration of this call.
    unsafe { ((*con_out).output_string)(con_out, message.as_ptr()) }
}

/// Returns the NUL-terminated UTF-16 diagnostic shown before boot services end.
fn boot_banner() -> [u16; 38] {
    utf16z("ZC OS UEFI loader; boot protocol v1\r\n")
}

/// Encodes an ASCII string as a fixed-size NUL-terminated UTF-16 array.
///
/// The loader's first diagnostic is deliberately ASCII, which avoids requiring
/// an allocator or a Unicode encoder before boot services are established.
fn utf16z<const N: usize>(value: &str) -> [u16; N] {
    let bytes = value.as_bytes();
    assert!(
        bytes.len() + 1 == N,
        "UTF-16 literal length must match its array"
    );

    let mut result = [0_u16; N];
    let mut index = 0;
    while index < bytes.len() {
        assert!(bytes[index].is_ascii(), "early boot text must be ASCII");
        result[index] = bytes[index] as u16;
        index += 1;
    }
    result
}

/// Host-only executable entry point so the crate can be unit-tested normally.
#[cfg(not(target_os = "uefi"))]
fn main() {
    let _ = BOOT_PROTOCOL_VERSION;
}

/// UEFI firmware has no standard runtime to print a panic.
#[cfg(target_os = "uefi")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_is_nul_terminated_utf16() {
        let banner = boot_banner();
        assert_eq!(banner.last(), Some(&0));
        assert_eq!(banner[0], u16::from(b'Z'));
    }

    #[test]
    fn utf16_encoder_preserves_ascii() {
        assert_eq!(utf16z::<3>("OK"), [u16::from(b'O'), u16::from(b'K'), 0]);
    }
}
