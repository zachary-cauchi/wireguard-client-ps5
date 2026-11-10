pub mod device;
pub mod logging;
pub mod result;
pub mod tunnel;

use ps5::Toast;
use ps5_std_rt as _;
use tracing::{info, instrument, level_filters::LevelFilter, trace};

use crate::logging::Ps5LoggingBuilder;
pub use crate::result::{Error, Result};

pub fn main() {
    Ps5LoggingBuilder::new()
        .with_klog_level(LevelFilter::TRACE)
        .with_std_level(LevelFilter::INFO)
        .install_global()
        .expect("Global logging set up successfully");

    if let Err(e) = main_loop() {
        klog_log!("Error from main loop. Error code: {}", e.0);
    }
}

#[instrument()]
fn main_loop() -> ps5::Result<()> {
    info!("Wireguard client initialised.");
    trace!("Testing trace");

    Toast::new("Hello from STD Rust.")
        .sub("Pretty cool huh?")
        .send()?;

    Ok(())
}
