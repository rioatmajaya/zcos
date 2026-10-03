//! Build script for the freestanding window-client task.

fn main() {
    println!("cargo:rustc-link-arg=-Tuser.ld");
    println!("cargo:rerun-if-changed=user.ld");
}
