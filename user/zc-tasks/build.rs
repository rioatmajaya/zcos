//! Build script for the freestanding userspace tasks.
//!
//! Both binaries link at the user base with our own linker script so the
//! kernel can map them without relocating.

fn main() {
    println!("cargo:rustc-link-arg=-Tuser.ld");
    println!("cargo:rerun-if-changed=user.ld");
}
