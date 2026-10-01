//! Framebuffer task: paints the shared test pattern, then exits.
//!
//! The kernel maps the display framebuffer into userspace and describes it
//! through the framebuffer-info syscall. Unsupported pixel formats exit
//! quietly so text-mode firmware never fails the boot.

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use zc_abi::{FramebufferInfo, bar_at, bar_color, encode};
use zc_user::{framebuffer_info, log, task_exit};

/// Task entry point; the kernel provides a fresh user stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    let mut info = FramebufferInfo::UNAVAILABLE;
    if !framebuffer_info(&mut info) || !info.is_available() {
        log("fb unavailable\n");
        task_exit()
    }
    paint(&info);
    log("fb painted\n");
    task_exit()
}

/// Fills the framebuffer with vertical color bars.
fn paint(info: &FramebufferInfo) {
    let width = u64::from(info.width);
    let height = u64::from(info.height);
    let stride = u64::from(info.stride);
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            let (red, green, blue) = bar_color(bar_at(x, width));
            if let Some(pixel) = encode(info.pixel_format, red, green, blue) {
                // SAFETY: the kernel mapped `stride * height` pixels at the
                // reported address with user permissions.
                unsafe {
                    (info.address as *mut u32)
                        .add((y * stride + x) as usize)
                        .write_volatile(pixel);
                }
            }
            x += 1;
        }
        y += 1;
    }
}
