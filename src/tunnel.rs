use std::sync::Arc;

use boringtun::noise::TunnResult;
use boringtun::{noise::Tunn, x25519::StaticSecret};
use tokio::net::UdpSocket;
use tracing::{debug, instrument, trace, warn};

use crate::Result;
use crate::config::{Config, PresharedKey, PrivateKey, PublicKey};

/// Represents a single-peer WireGuard tunnel connection.
/// Contains all the necessary logic to transmit and receive from the tunnel,
/// and maintain an active tunnel.
pub struct Tunnel {
    config: Arc<Config>,

    /// Actual Wireguard protocol-handling logic.
    tunn: Tunn,

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

        debug!("Building UDP socket.");

        let udp = UdpSocket::bind("0.0.0.0:0").await?;

        debug!("Done, returning.");

        Ok(Self {
            config,
            tunn,
            udp_socket: udp,
        })
    }

    pub async fn first_handshake(&mut self) -> Result<()> {
        let mut buf = [0u8; 1024 * 64];

        let handshake = self.tunn.format_handshake_initiation(&mut buf, false);

        self.process_tunnel_result(handshake).await?;

        Ok(())
    }

    pub async fn runtime_loop(&mut self) -> Result<()> {
        let mut buf = [0u8; 1024 * 64];

        // let handshake = self.tunn.format_handshake_initiation(&mut buf, false);

        debug!("Updating timers.");

        let tunn_result = self.tunn.update_timers(&mut buf);

        self.process_tunnel_result(tunn_result).await?;

        Ok(())
    }

    async fn process_tunnel_result<'a>(&'a mut self, res: TunnResult<'a>) -> Result<()> {
        match res {
            TunnResult::Done => debug!("Nothing to send right now."),
            TunnResult::WriteToNetwork(pkt) => {
                debug!("Writing {}-byte to network. Packet: {pkt:?}", pkt.len());

                let mut pending_bytes = pkt;

                loop {
                    let sent_bytes = self
                        .udp_socket
                        .send_to(pending_bytes, &self.config.peer.endpoint)
                        .await?;

                    if sent_bytes == pending_bytes.len() {
                        trace!("Packet sent.");
                        break;
                    } else {
                        trace!("Sent {sent_bytes} bytes.");
                        pending_bytes = &mut pending_bytes[sent_bytes..];
                    }
                }
            }
            TunnResult::WriteToTunnelV4(pkt, addr) => {
                debug!("Sending pkt to '{addr}'. Packet: {pkt:?}")
            }
            TunnResult::WriteToTunnelV6(pkt, addr) => {
                debug!("Sending pkt to '{addr}'. Packet: {pkt:?}")
            }
            TunnResult::Err(e) => warn!("Error received updating timers. Error: {e:?}"),
        }

        Ok(())
    }
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
