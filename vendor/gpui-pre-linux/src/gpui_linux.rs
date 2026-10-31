#![cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;

pub use linux::{current_platform, linux_platform};
#[cfg(feature = "x11")]
pub use linux::x11_scale_factor;
