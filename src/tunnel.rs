pub mod maintenance_subsystem;
pub mod rx_subsystem;
pub mod tx_subsystem;

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use boringtun::{noise::Tunn, x25519::StaticSecret};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tracing::{debug, instrument, trace};

use crate::bus::EventBus;
use crate::config::{Config, PresharedKey, PrivateKey, PublicKey};
use crate::result::{Error, WrapWithMsg};
use crate::tunnel::maintenance_subsystem::MaintenanceSubsystem;
use crate::tunnel::rx_subsystem::RxSubsystem;
use crate::tunnel::tx_subsystem::TxSubsystem;
use crate::{ErrorKind, Result};

const MAX_BUF_SIZE: usize = 1024 * 64;

/// Represents a single-peer WireGuard tunnel connection.
/// Delegates tunnel management to its subsystems.
pub struct Tunnel {
    config: Arc<Config>,

    /// Actual Wireguard protocol-handling logic.
    tunn: Mutex<Tunn>,

    /// Socket used to communicate to the peer.
    endpoint: UdpEndpoint,

    local_wg_addr: IpAddr,

    bus: EventBus,
}

impl Tunnel {
    /// Initialize the WG tunnel.
    #[instrument(name = "tunnel_init", skip_all)]
    pub async fn init(config: Arc<Config>, bus: EventBus) -> Result<Self> {
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
        let local_wg_addr = config.interface.address.ip_addr;

        debug!("Building UDP socket.");

        let endpoint = UdpEndpoint::new(socket_addr).await?;

        debug!("Socket connected to peer {socket_addr}.");

        Ok(Self {
            config,
            tunn: Mutex::new(tunn),
            endpoint,
            bus,
            local_wg_addr,
        })
    }

    pub fn maintenance_subsystem(self: &Arc<Self>) -> MaintenanceSubsystem {
        MaintenanceSubsystem::new(self.clone())
    }

    pub fn rx_subsystem(self: &Arc<Self>) -> RxSubsystem {
        RxSubsystem::new(self.clone(), self.bus.clone())
    }

    pub fn tx_subsystem(self: &Arc<Self>) -> TxSubsystem {
        TxSubsystem::new(self.clone(), self.bus.clone())
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

/// A UDP socket pointing to a Wireguard endpoint.
pub struct UdpEndpoint {
    socket: UdpSocket,
}

impl UdpEndpoint {
    /// Connect to the target Wireguard socket.
    pub async fn new(socket_addr: SocketAddr) -> Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(&socket_addr).await?;

        Ok(Self { socket })
    }

    /// Receive a packet, saving it to the provided byte buffer, and return a slice over the exact-sized packet.
    pub async fn recv<'a>(&'a self, buf: &'a mut [u8]) -> Result<&'a mut [u8]> {
        let total_rx_bytes = self.socket.recv(buf).await?;

        let received = &mut buf[0..total_rx_bytes];

        Ok(received)
    }

    /// Send all bytes in the given slice to the Wireguard endpoint.
    /// If not all the packet was sent, then a sub-slice is made over the remaining bytes
    /// and that slice is sent, repeating this process until the full original slice is sent.
    async fn send_full_slice(&self, packet: &[u8]) -> Result<()> {
        let mut pending_bytes = packet;

        while !pending_bytes.is_empty() {
            let sent_bytes_count = self.socket.send(pending_bytes).await?;

            trace!("Sent {sent_bytes_count} bytes.");

            pending_bytes = pending_bytes.split_at(sent_bytes_count).1;
        }

        trace!("Packet sent successfully.");

        Ok(())
    }
}
