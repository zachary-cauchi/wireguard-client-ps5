pub mod config;
pub mod device;
pub mod logging;
pub mod result;
pub mod tunnel;

use ps5::Toast;
use ps5_std_rt as _;
use tracing::{error, info, level_filters::LevelFilter};

pub use crate::result::{ErrorKind, Result};
use crate::{config::get_config, logging::Ps5LoggingBuilder, result::WrapWithMsg, tunnel::Tunnel};

fn main() {
    Ps5LoggingBuilder::new()
        .with_klog_level(LevelFilter::TRACE)
        .with_std_level(LevelFilter::DEBUG)
        .install_global()
        .expect("Global logging set up successfully");

    if let Err(e) = main_loop() {
        if let ErrorKind::PendingConfig = e.kind() {
            error!("No app configuration found. Add one, then rerun.");

            let _ = Toast::new("No app config found.")
                .sub("App config needed.")
                .send();
        } else {
            error!("Error from main loop. Error: {}", e);
        }

        return;
    }
}

fn main_loop() -> Result<()> {
    info!("Initialising Wireguard client.");

    Toast::new("Initialising Wireguard client.").send()?;
    let config = get_config().context("Getting app config")?;

    info!("Config file loaded.");

    let tunnel = Tunnel::init(&config)?;

    info!("Tunnel initialised.");

    Ok(())
}
