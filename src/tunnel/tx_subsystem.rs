use std::sync::Arc;

use boringtun::noise::TunnResult;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn};

use crate::Result;
use crate::bus::{Event, EventBus};
use crate::tunnel::{MAX_BUF_SIZE, Tunnel};

pub struct TxSubsystem {
    tunnel: Arc<Tunnel>,
    bus: EventBus,
}

impl TxSubsystem {
    pub(super) fn new(tunnel: Arc<Tunnel>, bus: EventBus) -> Self {
        Self { tunnel, bus }
    }

    #[instrument(name = "tx", skip_all)]
    pub async fn run_loop(self, token: CancellationToken) -> Result<()> {
        let mut tx_buf = [0u8; MAX_BUF_SIZE];

        info!("TX - Started.");

        let mut receiver = self.bus.subscribe();

        loop {
            tx_buf.fill(0);

            tokio::select! {
                res = self.process_event(&mut receiver, &mut tx_buf) => {
                    if let Err(e) = res {
                        warn!("Error occurred in event bus handler. Error: {e}");
                        break;
                    }
                },
                _ = token.cancelled() => {
                    debug!("Cancellation caught. exiting.");

                    break;
                }
            }
        }

        info!("TX - Stopped.");

        Ok(())
    }

    #[instrument(name = "send_pkt", skip_all)]
    async fn process_event(&self, receiver: &mut Receiver<Event>, tx_buf: &mut [u8]) -> Result<()> {
        let event = match receiver.recv().await {
            Ok(e) => e,
            Err(e @ RecvError::Closed) => return Err(e.into()),
            Err(RecvError::Lagged(n)) => {
                error!("Bus receiver lagged behind by {n} events. Packets likely dropped.");
                return Ok(());
            }
        };

        let Event::PacketToWG((addr, packet)) = event else {
            debug!("Event filtered.");
            return Ok(());
        };

        debug!(
            "Preparing {}-byte packet for WG transmission.",
            packet.0.len()
        );

        let tunn_result = self.tunnel.tunn.lock().await.encapsulate(&packet.0, tx_buf);

        match tunn_result {
            TunnResult::WriteToNetwork(pkt) => {
                debug!("Packet encapsulated. Sending.");
                self.tunnel.endpoint.send_full_slice(pkt).await?;
            }
            TunnResult::Done => {
                debug!("No action taken for packet.");
            }
            TunnResult::Err(e) => {
                warn!("Error returned during encapsulation. Error: {e:?}");
            }
            _ => {
                warn!("Unexpected result from encapsulation.")
            }
        }

        Ok(())
    }
}
