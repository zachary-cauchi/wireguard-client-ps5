use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
    ops::Range,
    sync::{Arc, RwLock},
    time::Instant,
};

use bytes::Bytes;
use priority_queue::DoublePriorityQueue;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;
use tracing::{Level, info, instrument, trace, warn};

use crate::{
    ErrorKind, Result,
    bus::{Event, EventBus},
    config::{PortForwardRule, Protocol, VirtualPort},
    result::{Error, WrapWithMsg},
};

pub struct UdpClientsSubystem {
    port_forward_rule: PortForwardRule,
    socket: UdpSocket,
    bus: EventBus,
    port_pool: UdpPortPool,
}

impl UdpClientsSubystem {
    pub async fn new(bus: EventBus, port_forward_rule: PortForwardRule) -> Result<Self> {
        if port_forward_rule.protocol != Protocol::Udp {
            return Err(Error::new(ErrorKind::BadValue("Non-UDP rule supplied")));
        }

        let socket = UdpSocket::bind(port_forward_rule.source_addr).await?;
        let port_pool = UdpPortPool::new();

        Ok(Self {
            port_forward_rule,
            socket,
            bus,
            port_pool,
        })
    }

    #[instrument(name = "udp_clients", skip_all, err(level = Level::ERROR))]
    pub async fn run_loop(mut self, token: CancellationToken) -> Result<()> {
        info!("UDP CLIENTS - Started.");

        let mut rx_buf = [0u8; 64 * 1024];

        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    info!("Cancellation triggered. Exiting.");
                    break;
                }
                res = self.socket.recv_from(&mut rx_buf) => {
                    let (byte_count, peer) = match res {
                        Ok(ok) => ok,
                        Err(e) => {
                            warn!("Failed to receive datagram from peer. Error: {e}");
                            continue;
                        }
                    };

                    let datagram = &rx_buf[0..byte_count];

                    if let Err(e) = self.recv_datagram(datagram, peer).await {
                        warn!("Failed to process datagram from peer {peer}. Error: {e}");
                        continue;
                    }
                }
            }
        }

        info!("UDP CLIENTS - Stopped.");
        Ok(())
    }

    #[instrument(skip(self, datagram), err(level = Level::WARN))]
    async fn recv_datagram(&mut self, datagram: &[u8], peer: SocketAddr) -> Result<()> {
        let virtual_port = self
            .port_pool
            .next(&peer)
            .context("Assigning virtual port for peer")?;

        trace!(
            "Received {}-byte datagram. Assigned port: {virtual_port}",
            datagram.len()
        );

        self.port_pool
            .update_last_transmit(virtual_port)
            .context("Updating virtual port stats")?;

        self.bus.send(Event::LocalOutboundPacket(
            self.port_forward_rule.clone(),
            virtual_port,
            Bytes::copy_from_slice(datagram),
        ))?;

        Ok(())
    }
}

/// A port pool for assigning virtual UDP ports.
/// Unlike the TCP port pool, since UDP is stateless and does not 'release' ports,
/// the pool also tracks the usage history of the port and chooses the next port
/// based on temporal factors or peer-port overassignment.
#[derive(Clone)]
pub struct UdpPortPool {
    inner: Arc<RwLock<UdpPortPoolInner>>,
}

impl UdpPortPool {
    /// The range of available ports to assign.
    pub const PORT_RANGE: Range<u16> = 60001u16..60019u16;
    /// How long, in seconds, a port can go unused before becoming available.
    pub const PORT_AGE_TILL_RELEASE: u64 = 60;
    /// How many UDP ports an IP address can have before that port is released.
    pub const MAX_PORTS_PER_IP: usize = 64;

    pub fn new() -> Self {
        let inner = UdpPortPoolInner::new(Self::PORT_RANGE);

        Self {
            inner: Arc::new(RwLock::new(inner)),
        }
    }

    /// Get the next port to assign to a client peer. A port is chosen in descending order:
    ///
    /// * A port already assigned to this specific peer.
    /// * One of the ports this peer is assigned (if he already has too many ports).
    /// * An unassigned port from the queue.
    /// * A port which hasn't been used for a long enough while.
    ///
    /// If a new/re-used port is assigned, it's assignment is recorded.
    pub fn next(&self, peer: &SocketAddr) -> Result<VirtualPort> {
        let port_reuse_opt = self.check_if_port_reusable(peer)?;

        let mut inner = self.inner.write().context("Locking port pool for write")?;

        // If a client-assigned port is to be reused, return that.
        // Else, pop a fresh one from the queue.
        // Else, Reuse the oldest port.
        // If all else fails, return an error.
        let port = port_reuse_opt
            .or_else(|| inner.queue.pop_front())
            .or_else(|| {
                // SAFETY: If `queue` is empty, then `port_usage` is guaranteed to be non-empty.
                let last_port = inner.port_usage.peek_min().unwrap();
                let time_since = Instant::now().duration_since(*last_port.1).as_secs();

                if time_since >= Self::PORT_AGE_TILL_RELEASE {
                    warn!("Virtual port '{}' eligible for re-use.", last_port.0);
                    Some(*last_port.0)
                } else {
                    None
                }
            })
            .ok_or_else(|| Error::new(ErrorKind::PortPoolExhausted))?;

        inner.peer_to_port_map.insert(*peer, port);
        inner.port_to_peer_map.insert(port, *peer);

        Ok(VirtualPort(Protocol::Udp, port))
    }

    /// Get the port currently assigned to the peer.
    /// If no port is available, check how many ports were previously assigned to the peer.
    /// If the peer has too many ports, reuse it's oldest assigned port.
    /// Else, no port can be reused so return none.
    fn check_if_port_reusable(&self, peer: &SocketAddr) -> Result<Option<u16>> {
        let inner = self.inner.read().context("Locking port pool")?;

        // Check if there's already a port assigned to this peer.
        if let Some(port) = inner.peer_to_port_map.get(peer) {
            return Ok(Some(*port));
        }

        let peer_ip = peer.ip();
        // Find out how many ports are currently in use by that IP.
        let peer_port_count = inner
            .peer_port_usage
            .get(&peer_ip)
            .map(|port_usage| port_usage.len())
            .unwrap_or_default();

        if peer_port_count >= Self::MAX_PORTS_PER_IP {
            let port = *inner
                .peer_port_usage
                .get(&peer_ip)
                .unwrap()
                .peek_min()
                .unwrap()
                .0;

            warn!(
                "Peer '{peer_ip}' reached max amount of ports. Reusing port '{}'",
                port
            );

            Ok(Some(port))
        } else {
            Ok(None)
        }
    }

    /// Record the current time as the last time this port was used.
    pub fn update_last_transmit(&self, port: VirtualPort) -> Result<()> {
        let mut inner = self.inner.write()?;
        let now = Instant::now();

        inner.port_usage.push(port.1, now);

        if let Some(peer) = inner.port_to_peer_map.get(&port.1).copied() {
            let priority_queue = inner.peer_port_usage.entry(peer.ip()).or_default();

            priority_queue.push(port.1, now);
        }

        Ok(())
    }

    /// Get the peer who is currently assigned to this port, if any.
    pub fn get_peer_address(&self, port: u16) -> Result<Option<SocketAddr>> {
        Ok(self.inner.read()?.port_to_peer_map.get(&port).copied())
    }
}

struct UdpPortPoolInner {
    /// Queue of available ports.
    queue: VecDeque<u16>,
    /// Mapping of how client peers to their assigned port.
    peer_to_port_map: HashMap<SocketAddr, u16>,
    /// Mapping of ports to their owning client peers.
    port_to_peer_map: HashMap<u16, SocketAddr>,
    /// Tracks how many ports were assigned to this peer in the past.
    peer_port_usage: HashMap<IpAddr, DoublePriorityQueue<u16, Instant>>,
    /// Tracks the per-port assignment history.
    port_usage: DoublePriorityQueue<u16, Instant>,
}

impl UdpPortPoolInner {
    pub fn new(range: Range<u16>) -> Self {
        let queue = VecDeque::from_iter(range);

        Self {
            queue,
            port_to_peer_map: HashMap::new(),
            peer_to_port_map: HashMap::new(),
            peer_port_usage: HashMap::new(),
            port_usage: DoublePriorityQueue::new(),
        }
    }
}
