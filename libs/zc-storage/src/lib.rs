//! Storage and filesystem parsing shared across ZC OS protection boundaries.
//!
//! Every module here is pure: it never touches hardware, only a caller-supplied
//! 512-byte sector reader or a byte slice. The userspace block domain links
//! them directly, and the same code runs on the host under test.
//!
//! They live outside the privileged `zc-kernel` crate on purpose — the kernel
//! schedules, maps memory, and moves messages; it does not parse filesystems.
//! Keeping the parsers here lets the block domain be the only linker of this
//! code, so ring 0 carries none of it.
//!
//! - [`virtio`]: legacy virtio-over-PIO register offsets and ring math.
//! - [`block_cache`]: the write-back sector cache.
//! - [`mbr`]: the MBR partition table.
//! - [`zcfs`]: the ZC-native log-structured filesystem.
//! - [`ext2`], [`fat32`]: read-only foreign filesystem parsers.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod block_cache;
pub mod ext2;
pub mod fat32;
pub mod mbr;
pub mod virtio;
pub mod zcfs;
