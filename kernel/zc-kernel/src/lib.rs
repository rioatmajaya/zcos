//! Privileged, policy-free mechanisms for the ZC OS microkernel.
//!
//! This crate is `no_std` so the same code can execute after the UEFI loader
//! has exited boot services. Early code validates its boot contract and builds
//! a physical-memory allocator before enabling higher-level subsystems.

#![no_std]

pub mod boot;
pub mod capability;
pub mod memory;
