//! Original Rust Windows audio miniport implementation. See drivers/windows.
#![cfg_attr(all(feature = "kernel", not(test)), no_std)]

pub mod abi;
pub mod descriptors;
pub mod transport;

#[cfg(any(all(feature = "kernel", target_os = "windows"), test))]
mod kernel;

#[cfg(all(feature = "kernel", target_os = "windows", not(test)))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    unsafe { kernel::bugcheck() }
}
