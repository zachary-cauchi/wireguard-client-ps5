pub mod bus;
pub mod client_acceptor;
pub mod config;
pub mod logging;
pub mod result;
pub mod tunnel;
pub mod virtual_interface;
pub mod virtual_net_stack;

use std::sync::Arc;

use ps5::{Toast, payload::exit};
use ps5_std_rt as _;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

pub use crate::result::{ErrorKind, Result};
use crate::{
    bus::EventBus,
    client_acceptor::tcp::TcpClientsSubsystem,
    config::{Config, Protocol, get_config},
    logging::{Ps5LoggingBuilder, klog_log},
    result::WrapWithMsg,
    tunnel::Tunnel,
    virtual_interface::TcpVirtualInterface,
    virtual_net_stack::VirtualNetStack,
};

#[tokio::main]
async fn main() {
    let config = match get_config() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            klog_log(format_args!("Config parse failed. Error: {e}"));

            Toast::new("Config parse failed.")
                .sub("Check config and rerun.")
                .send()
                .unwrap();

            exit(1);
        }
    };

    Ps5LoggingBuilder::new()
        .with_klog_level(config.general.klog_level)
        .with_std_level(config.general.std_level)
        .install_global()
        .expect("Global logging set up successfully");

    if let Err(e) = main_loop(config).await {
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

async fn main_loop(config: Arc<Config>) -> Result<()> {
    info!("Initialising Wireguard client.");

    Toast::new("Initialising Wireguard client.").send()?;

    info!("Config file loaded.");

    let sample_rule = config.port_forward_rule.clone();

    let bus = EventBus::new();
    let tunnel = Tunnel::init(config.clone(), bus.clone())
        .await
        .context("Initialising Wireguard tunnel")?;
    let tunnel = Arc::new(tunnel);

    info!("Tunnel initialised.");

    let tcp_client_subsystem = TcpClientsSubsystem::new(bus.clone(), sample_rule.clone()).await?;
    let virtual_device = VirtualNetStack::new(Protocol::Tcp, 1500, bus.clone());
    let virtual_interface = TcpVirtualInterface::new(
        config.interface.address.ip_addr,
        &[sample_rule],
        bus.clone(),
        virtual_device,
    );

    let token = CancellationToken::new();

    let mut tasks = spawn_tasks(
        &tunnel,
        tcp_client_subsystem,
        virtual_interface,
        token.clone(),
    )
    .await?;

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
    tcp_client_subsystem: TcpClientsSubsystem,
    virtual_interface: TcpVirtualInterface,
    token: CancellationToken,
) -> Result<JoinSet<Result<()>>> {
    let mut tasks = JoinSet::new();

    {
        let tunnel = tunnel.clone();
        let token = token.clone();
        tasks.spawn(async move { tunnel.maintenance_subsystem().run_loop(token).await });
    }

    {
        let tunnel = tunnel.clone();
        let token = token.clone();
        tasks.spawn(async move { tunnel.rx_subsystem().run_loop(token).await });
    }

    {
        let tunnel = tunnel.clone();
        let token = token.clone();
        tasks.spawn(async move { tunnel.tx_subsystem().run_loop(token).await });
    }

    {
        let token = token.clone();
        tasks.spawn(async move { cancellation_task(token).await });
    }

    {
        let token = token.clone();
        tasks.spawn(async move { tcp_client_subsystem.run_loop(token).await });
    }

    {
        let token = token.clone();
        tasks.spawn(async move { virtual_interface.run_loop(token).await });
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
    }
    Ok(())
}
