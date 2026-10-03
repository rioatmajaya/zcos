//! Privileged, policy-free mechanisms for the ZC OS microkernel.
//!
//! This crate is `no_std` so the same code can execute after the UEFI loader
//! has exited boot services. Early code validates its boot contract and builds
//! a physical-memory allocator before enabling higher-level subsystems.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod acpi;
pub mod addrspace;
pub mod block_cache;
pub mod boot;
pub mod capability;
pub mod cpio;
pub mod device;
pub mod devfs;
pub mod ext2;
pub mod fat32;
pub mod gdt;
pub mod ipc;
pub mod iomap;
pub mod irq;
pub mod kbd;
pub mod mbr;
pub mod memory;
pub mod pci;
pub mod perms;
pub mod ramfs;
pub mod sched;
pub mod service;
pub mod surface;
pub mod syscall;
pub mod task;
pub mod timer;
pub mod tmpfs;
pub mod tramp;
pub mod trap;
pub mod vfs;
pub mod virtio;
pub mod vm;
pub mod zcfs;
