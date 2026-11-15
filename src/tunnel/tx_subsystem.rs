use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use crate::Result;
use crate::tunnel::{MAX_BUF_SIZE, Tunnel};

pub struct TxSubsystem {
    tunnel: Arc<Tunnel>,
}

impl TxSubsystem {
    pub(super) fn new(tunnel: Arc<Tunnel>) -> Self {
        Self { tunnel }
    }

    #[instrument(name = "tx", skip_all)]
    pub async fn run_loop(self, token: CancellationToken) -> Result<()> {
        let mut tx_buf = [0u8; MAX_BUF_SIZE];

        info!("TX - Started.");

        loop {
            tx_buf.fill(0);

            tokio::select! {
                res = self.tx_entry(&mut tx_buf) => {
                    if let Err(e) = res {
                        warn!("Error from maintenance task. Error: {e}");
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

    async fn tx_entry(&self, tx_buf: &mut [u8]) -> Result<()> {
        let mut tunn = self.tunnel.tunn.lock().await;

        Ok(())
    }
}
