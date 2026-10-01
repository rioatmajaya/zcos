//! The first executable loaded by UEFI for ZC OS.
//!
//! The loader intentionally starts without a third-party runtime so its UEFI
//! boundary remains explicit. On the `x86_64-unknown-uefi` target it disables
//! the firmware watchdog, discovers the GOP framebuffer, captures the memory
//! map, locates the ACPI RSDP, and assembles a [`zc_abi::BootInfo`]. It stops
//! before `ExitBootServices`, which the next increment performs.
//!
//! On the host the crate builds as an ordinary test binary so the UEFI struct
//! layouts and the memory-map conversion can be checked without firmware.

#![cfg_attr(target_os = "uefi", no_main)]
#![no_std]
#![allow(unsafe_code)] // UEFI protocol calls require raw firmware pointers.
#![cfg_attr(not(target_os = "uefi"), allow(dead_code, unused_imports))]

#[cfg(not(target_os = "uefi"))]
extern crate std;

mod elf;
mod memmap;
mod serial;
mod uefi;

#[cfg(target_os = "uefi")]
mod loader;

/// Host-only executable entry point so the crate can be unit-tested normally.
#[cfg(not(target_os = "uefi"))]
fn main() {}

/// UEFI firmware has no standard runtime to print a panic.
#[cfg(target_os = "uefi")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
