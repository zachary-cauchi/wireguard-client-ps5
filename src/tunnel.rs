use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use boringtun::noise::TunnResult;
use boringtun::noise::errors::WireGuardError;
use boringtun::{noise::Tunn, x25519::StaticSecret};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument, trace, warn};

use crate::config::{Config, PresharedKey, PrivateKey, PublicKey};
use crate::result::{Error, WrapWithMsg};
use crate::{ErrorKind, Result};

const MAX_BUF_SIZE: usize = 1024 * 64;

/// Represents a single-peer WireGuard tunnel connection.
/// Contains all the necessary logic to transmit and receive from the tunnel,
/// and maintain an active tunnel.
pub struct Tunnel {
    config: Arc<Config>,

    /// Actual Wireguard protocol-handling logic.
    tunn: Mutex<Tunn>,

    /// Socket used to communicate to the peer.
    udp_socket: UdpSocket,
}

impl Tunnel {
    /// Initialize the WG tunnel.
    #[instrument(name = "tunnel_init", skip_all)]
    pub async fn init(config: Arc<Config>) -> Result<Self> {
        const KEEPALIVE: u16 = 30;
        debug!("Initialising tunnel.");

        let static_secret = try_get_static_secret(&config.interface.private_key)?;
        let public_key = try_get_public_key(&config.peer.public_key)?;
        let preshared_key = try_get_preshared_key(&config.peer.preshared_key)?;

        let peer_index = ps5::random::u64().map(|val| val as u32)?;

        let tunn = Tunn::new(
            static_secret,
            public_key,
            Some(preshared_key),
            Some(KEEPALIVE),
            peer_index,
            None,
        );

        let socket_addr = get_or_resolve_peer_endpoint_addr(&config)
            .await
            .context("Getting or resolving peer endpoint addr")?;

        debug!("Building UDP socket.");

        let udp = UdpSocket::bind("0.0.0.0:0").await?;
        udp.connect(&socket_addr).await?;

        debug!("Socket connected to peer {socket_addr}.");

        Ok(Self {
            config,
            tunn: Mutex::new(tunn),
            udp_socket: udp,
        })
    }

    /// Maintains the Wireguard tunnel by running routine steps handled by `boringtun`.
    /// A buffer is kept on-stack and reused throughout the lifetime of this function.
    /// Returns on token cancellation.
    pub async fn maintenance_task(&self, token: CancellationToken) -> Result<()> {
        let mut buffer = [0u8; MAX_BUF_SIZE];

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

                    return Ok(());
                }
            }
        }
    }

    #[instrument(name = "tunnel_maintenance", skip_all)]
    async fn maintenance_entry(&self, mut buf: &mut [u8]) -> Result<()> {
        let tunn_result = self.tunn.lock().await.update_timers(&mut buf);

        self.process_tunnel_result(tunn_result).await?;

        Ok(())
    }

    pub async fn rx_task(&self, token: CancellationToken) -> Result<()> {
        let mut rx_buf = [0u8; MAX_BUF_SIZE];

        loop {
            rx_buf.fill(0);

            tokio::select! {
                res = self.rx_entry(&mut rx_buf) => {
                    if let Err(e) = res {
                        warn!("Error from maintenance task. Error: {e}");
                    }
                },
                _ = token.cancelled() => {
                    debug!("Cancellation caught. exiting.");

                    return Ok(());
                }
            }
        }
    }

    async fn rx_entry(&self, rx_buf: &mut [u8]) -> Result<()> {
        let total_rx_bytes = self.udp_socket.recv(rx_buf).await?;

        let received = &rx_buf[0..total_rx_bytes];

        trace!("Received {total_rx_bytes}-byte packet. Packet: {received:?}");

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

                self.send_to_endpoint(pkt).await?;
            }
            TunnResult::WriteToTunnelV4(pkt, addr) => {
                debug!("Sending pkt to '{addr}' (TO BE IMPLEMENTED). Packet: {pkt:?}")
            }
            TunnResult::WriteToTunnelV6(pkt, addr) => {
                debug!("Sending pkt to '{addr}' (TO BE IMPLEMENTED). Packet: {pkt:?}")
            }
            // If the connection expired, re-run the handshake procedure.
            TunnResult::Err(WireGuardError::ConnectionExpired) => {
                debug!("New connection needed. Sending handshake.");
                let mut buf = vec![0u8; MAX_BUF_SIZE];
                let handshake_res = self
                    .tunn
                    .lock()
                    .await
                    .format_handshake_initiation(&mut buf[..], false);

                // The boxed pin allows for the async recursion.
                Box::pin(async move { self.process_tunnel_result(handshake_res).await }).await?;
            }
            TunnResult::Err(e) => warn!("Error received from tunnel. Error: {e:?}"),
        }

        Ok(())
    }

    /// Send all bytes in the given slice to the Wireguard endpoint.
    /// If not all the packet was sent, the a sub-slice is taken over the remaining bytes
    /// and that slice is sent, repeating as needed until the full original slice was sent.
    async fn send_to_endpoint(&self, pkt: &[u8]) -> Result<()> {
        let mut pending_bytes = pkt;

        loop {
            let sent_bytes = self.udp_socket.send(pending_bytes).await?;

            trace!("Sent {sent_bytes} bytes.");

            if sent_bytes == pending_bytes.len() {
                trace!("Packet sent successfully.");
                return Ok(());
            } else {
                pending_bytes = &pending_bytes[sent_bytes..];
            }
        }
    }
}

/// Get the peer endpoint socket address or resolve it if it's a hostname.
async fn get_or_resolve_peer_endpoint_addr(config: &Arc<Config>) -> Result<SocketAddr> {
    let endpoint = config.peer.endpoint.clone();

    let mut socket_addr_iter = endpoint.to_socket_addrs()?;

    socket_addr_iter
        .next()
        .ok_or(Error::new(ErrorKind::ConfigLoadError(
            "Bad or empty peer endpoint address",
        )))
}

fn try_get_static_secret(private_key: &PrivateKey) -> Result<StaticSecret> {
    // SAFETY: Key length validated during type construction.
    let arr_ref = private_key.0.as_array::<32>().unwrap();

    Ok((*arr_ref).into())
}

fn try_get_public_key(public_key: &PublicKey) -> Result<boringtun::x25519::PublicKey> {
    // SAFETY: Key length validated during type construction.
    let arr_ref = public_key.0.as_array::<32>().unwrap();

    Ok((*arr_ref).into())
}

fn try_get_preshared_key(preshared_key: &PresharedKey) -> Result<[u8; 32]> {
    // SAFETY: Key length validated during type construction.
    let arr_ref = preshared_key.0.as_array::<32>().unwrap();

    Ok(*arr_ref)
}
