// src/main.rs
#![no_std]
#![no_main]

use ps5::{Toast, hw, log};

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    let temp = hw::cpu_temperature().unwrap_or(-1);

    log!("Hello from payload. CPU temp: ", temp, "C");

    match Toast::new("Hello from Rust payload.")
        .sub("Pretty cool huh?")
        .send()
    {
        Ok(()) => 0,
        Err(e) => e.0,
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
