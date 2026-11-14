pub mod config;
pub mod logging;
pub mod result;
pub mod tunnel;

use std::{sync::Arc, time::Duration};

use ps5::Toast;
use ps5_std_rt as _;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, level_filters::LevelFilter};

pub use crate::result::{ErrorKind, Result};
use crate::{config::get_config, logging::Ps5LoggingBuilder, result::WrapWithMsg, tunnel::Tunnel};

#[tokio::main]
async fn main() {
    Ps5LoggingBuilder::new()
        .with_klog_level(LevelFilter::TRACE)
        .with_std_level(LevelFilter::TRACE)
        .install_global()
        .expect("Global logging set up successfully");

    if let Err(e) = main_loop().await {
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

async fn main_loop() -> Result<()> {
    info!("Initialising Wireguard client.");

    Toast::new("Initialising Wireguard client.").send()?;
    let config = Arc::new(get_config().context("Getting app config")?);

    info!("Config file loaded.");

    let tunnel = Tunnel::init(config.clone())
        .await
        .context("Initialising Wireguard tunnel")?;
    let tunnel = Arc::new(tunnel);

    info!("Tunnel initialised.");

    let token = CancellationToken::new();

    let mut tasks = spawn_tasks(&tunnel, token.clone()).await?;

    info!("Tasks started.");

    if let Err(e) = tasks.join_next().await.unwrap() {
        error!("Error occurred in task. Error: {e}");
    }

    token.cancel();

    tasks
        .join_all()
        .await
        .into_iter()
        .filter_map(Result::err)
        .for_each(|e| error!("One task returned an error. Error: {e}"));

    info!("All tasks concluded. Exiting.");

    Ok(())
}

/// Spawn all main tasks.
async fn spawn_tasks(
    tunnel: &Arc<Tunnel>,
    token: CancellationToken,
) -> Result<JoinSet<Result<()>>> {
    let mut tasks = JoinSet::new();

    {
        let tunnel = tunnel.clone();
        let token = token.clone();
        tasks.spawn(async move { tunnel.maintenance_task(token).await });
    }

    {
        let token = token.clone();
        tasks.spawn(async move { cancellation_task(token).await });
    }

    Ok(tasks)
}

/// Trigger cancellation if the program is alive for too long or on CTRL-C.
/// TODO: Replace `ctrl_c` detection with alternative.
async fn cancellation_task(token: CancellationToken) -> Result<()> {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("CTRL-C captured. Beginning shutdown.");
            token.cancel();
        }
        _ = token.cancelled() => {
            info!("Cancellation received. Shutting down.");
        }
        _ = tokio::time::sleep(Duration::from_secs(300)) => {
            info!("Task has lived too long. Triggering cancellation.");
            token.cancel();
        }
    }
    Ok(())
}
