//! Build script for the freestanding file-manager task.

fn main() {
    println!("cargo:rustc-link-arg=-Tuser.ld");
    println!("cargo:rerun-if-changed=user.ld");
}
