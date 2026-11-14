use std::sync::Arc;
use std::time::Duration;

use boringtun::noise::TunnResult;
use boringtun::noise::errors::WireGuardError;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use crate::Result;
use crate::tunnel::{MAX_BUF_SIZE, Tunnel};

/// Subsystem responsible for maintaining an active Wireguard connection.
pub struct MaintenanceSubsystem {
    tunnel: Arc<Tunnel>,
}

impl MaintenanceSubsystem {
    pub(super) fn new(tunnel: Arc<Tunnel>) -> Self {
        Self { tunnel }
    }

    /// Maintains the Wireguard tunnel by running routine steps handled by `boringtun`.
    /// A buffer is kept on-stack and reused throughout the lifetime of this function.
    /// Returns on token cancellation.
    #[instrument(name = "maintenance", skip_all)]
    pub async fn run_loop(self, token: CancellationToken) -> Result<()> {
        let mut buffer = [0u8; MAX_BUF_SIZE];

        info!("MAINTENANCE - Started.");

        loop {
            buffer.fill(0);

            tokio::select! {
                res = self.maintenance_entry(&mut buffer) => {
                    match res {
                        Ok(_) => tokio::time::sleep(Duration::from_millis(250)).await,
                        Err(e) => warn!("Error from maintenance task. Error: {e}"),
                    }
                },
                _ = token.cancelled() => {
                    debug!("Cancellation caught. exiting.");

                    break;
                }
            }
        }

        info!("MAINTENANCE - Stopped.");
        Ok(())
    }

    async fn maintenance_entry(&self, mut buf: &mut [u8]) -> Result<()> {
        let tunn_result = self.tunnel.tunn.lock().await.update_timers(&mut buf);

        self.process_tunnel_result(tunn_result).await?;

        Ok(())
    }

    /// Processes a `TunnResult` returned by `tunn`.
    /// * Sends pending packets to the Wireguard endpoint.
    /// * Sends handshake packets on initial connection or expired connections.
    ///     * Does so recursively.
    async fn process_tunnel_result<'a>(&'a self, res: TunnResult<'a>) -> Result<()> {
        match res {
            TunnResult::Done => {}
            TunnResult::WriteToNetwork(pkt) => {
                debug!("Writing {}-byte to network. Packet: {pkt:?}", pkt.len());

                self.tunnel.endpoint.send_full_slice(pkt).await?;
            }
            // If the connection expired, re-run the handshake procedure.
            TunnResult::Err(WireGuardError::ConnectionExpired) => {
                debug!("New connection needed. Sending handshake.");
                let mut buf = vec![0u8; MAX_BUF_SIZE];
                let handshake_res = self
                    .tunnel
                    .tunn
                    .lock()
                    .await
                    .format_handshake_initiation(&mut buf[..], false);

                // The boxed pin allows for the async recursion.
                Box::pin(async move { self.process_tunnel_result(handshake_res).await }).await?;
            }
            TunnResult::Err(e) => warn!("Error received from tunnel. Error: {e:?}"),
            res => warn!("Unexpected result : {res:?}"),
        }

        Ok(())
    }
}
