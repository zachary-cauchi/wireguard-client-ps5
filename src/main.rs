use ps5::{Toast, hw, log};
use ps5_std_rt as _;

pub fn main() {
    let temp = hw::cpu_temperature().unwrap_or(-1);

    log!("Hello from payload. CPU temp: ", temp, "C");

    if let Err(e) = main_loop() {
        log!("Error from main loop. Error code: ", e.0);
    }
}

fn main_loop() -> ps5::Result<()> {
    Toast::new("Hello from STD Rust.")
        .sub("Pretty cool huh?")
        .send()?;

    Ok(())
}
