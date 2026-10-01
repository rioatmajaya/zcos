//! Build script for the freestanding kernel image.
//!
//! The kernel is linked at its final higher-half address with our own linker
//! script so the loader can place it without relocating.

fn main() {
    println!("cargo:rustc-link-arg=-Tkernel.ld");
    println!("cargo:rerun-if-changed=kernel.ld");
}
