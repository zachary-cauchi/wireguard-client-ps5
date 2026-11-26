use std::sync::Arc;

use boringtun::noise::TunnResult;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

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
                event = receiver.recv_or_panic() => {
                    if let Err(e) = self.process_event(event, &mut tx_buf).await {
                        warn!("Error occurred processing event. Error: {e}");
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
    async fn process_event(&self, event: Event, tx_buf: &mut [u8]) -> Result<()> {
        let Event::PacketToWG(packet) = event else {
            return Ok(());
        };

        debug!(
            "Preparing {}-byte packet for WG transmission.",
            packet.len()
        );

        let tunn_result = self.tunnel.tunn.lock().await.encapsulate(&packet, tx_buf);

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
