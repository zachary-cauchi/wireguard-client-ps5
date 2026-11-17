use std::collections::VecDeque;
use std::net::SocketAddr;
use std::ops::Range;
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::sync::RwLock;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tracing::{Level, debug, error, info, instrument, warn};

use crate::bus::{Event, EventBus, RawPacket};
use crate::config::{Protocol, VirtualPort};
use crate::result::{Error, WrapWithMsg};
use crate::{ErrorKind, Result};

/// Handles spawning of TCP streams from incoming connections and outbound events.
/// Assigns a virtual port to the connection and forwards them through the event bus.
pub struct TcpClientsSubsystem {
    socket: TcpListener,
    bus: EventBus,
    virtual_port_pool: PortPool,
}

impl TcpClientsSubsystem {
    pub async fn new(bus: EventBus, addr: impl ToSocketAddrs) -> Result<Self> {
        let socket = TcpListener::bind(addr).await?;
        let virtual_port_pool = PortPool::new(Protocol::Tcp);

        Ok(Self {
            socket,
            bus,
            virtual_port_pool,
        })
    }

    #[instrument(name = "tcp_clients", skip_all, err(level = Level::ERROR))]
    pub async fn run_loop(self, token: CancellationToken) -> Result<()> {
        info!("TCP CLIENTS - Started.");

        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    info!("Cancellation triggered. Exiting.");
                    break;
                }
                res = self.socket.accept() => {
                    let token = token.clone();
                    let process_res = match res {
                        Ok((socket, addr)) => self.process_socket(socket, addr, token).await,
                        Err(e) => {
                            warn!("Error accepting TCP connection. Error: {e}");
                            continue;
                        },
                    };

                    if let Err(e) = process_res {
                        warn!("Failed to process socket. Error: {e}");
                    }
                }
            }
        }

        info!("TCP CLIENTS - Stopped.");

        Ok(())
    }

    /// When receiving a new stream, spawn a task to handle that stream.
    #[instrument(skip_all, fields(peer = %peer))]
    async fn process_socket(
        &self,
        stream: TcpStream,
        peer: SocketAddr,
        token: CancellationToken,
    ) -> Result<()> {
        debug!("Spawning TCP stream handler");

        let assigned_virtual_port = self
            .virtual_port_pool
            .next()
            .await
            .context("Selecting virtual TCP port")?;

        debug!("Assigning virtual port {assigned_virtual_port}.");

        let handler = TcpClientHandler {
            stream,
            peer,
            bus: self.bus.clone(),
            port_pool: self.virtual_port_pool.clone(),
            virtual_port: assigned_virtual_port,
        };

        tokio::spawn(token.run_until_cancelled_owned(handler.run_loop()));

        Ok(())
    }
}

pub struct TcpClientHandler {
    stream: TcpStream,
    peer: SocketAddr,
    bus: EventBus,
    port_pool: PortPool,
    virtual_port: VirtualPort,
}

impl TcpClientHandler {
    #[instrument(name = "tcp_client", skip_all, fields(peer = %self.peer, virtual_port = %self.virtual_port))]
    async fn run_loop(mut self) {
        debug!("Handling client.");

        let mut rx_buf = [0; 1024 * 64];

        let mut bus_recv = self.bus.subscribe();

        loop {
            tokio::select! {
                res = self.stream.read(&mut rx_buf) => {
                    let packet = match res {
                        Ok(0) => {
                            debug!("Connection closed. Closing stream.");
                            break;
                        }
                        Ok(p) => &rx_buf[..p],
                        Err(e) => {
                            warn!("Error reading from stream. Error: {e}");
                            continue;
                        }
                    };
                    if let Err(e) = self.process_packet(packet).await {
                        warn!("Failed to process packet. Error: {e}");
                    }
                }
                res = bus_recv.recv() => {
                    let event = match res {
                        Ok(e) => e,
                        Err(RecvError::Closed) => {
                            error!("Event bus is closed. Closing stream.");
                            break;
                        },
                        Err(RecvError::Lagged(n)) => {
                            error!("Bus receiver lagged behind by {n} events. Packets likely dropped.");
                            continue;
                        }
                    };
                    if let Err(e) = self.process_event(event).await {
                        warn!("Error encountered when processing event. Error: {e}")
                    }
                }
            }
        }

        if let Err(e) = self.port_pool.release(self.virtual_port).await {
            warn!("Failed to release port. Error: {e}");
        }

        if let Err(e) = self.bus.send(Event::ClientDisconnected(self.virtual_port)) {
            warn!("Failed to send client disconnection event. Error: {e}");
        }
    }

    async fn process_packet(&self, packet: &[u8]) -> Result<()> {
        debug!("Received {}-byte packet: {:?}", packet.len(), packet);

        let raw_packet = RawPacket(packet.into());

        self.bus.send(Event::ClientPacket((self.peer, raw_packet)))
    }

    async fn process_event(&self, event: Event) -> Result<()> {
        debug!("Received event. Event: {event:?}");
        // TODO: Check if event is relevant to this peer and handle accordingly.

        Ok(())
    }
}

#[derive(Clone)]
pub struct PortPool {
    inner: Arc<RwLock<PortPoolInner>>,
}

impl PortPool {
    pub const PORT_RANGE: Range<u16> = 60001u16..60019u16;

    pub fn new(protocol: Protocol) -> Self {
        let inner = PortPoolInner::new(protocol, Self::PORT_RANGE);

        Self {
            inner: Arc::new(RwLock::new(inner)),
        }
    }

    pub async fn next(&self) -> Result<VirtualPort> {
        self.inner.write().await.next()
    }

    pub async fn release(&self, port: VirtualPort) -> Result<()> {
        self.inner.write().await.release(port)
    }
}

struct PortPoolInner {
    queue: VecDeque<u16>,
    port_range: Range<u16>,
    protocol: Protocol,
}

impl PortPoolInner {
    pub fn new(protocol: Protocol, range: Range<u16>) -> Self {
        Self {
            queue: VecDeque::from_iter(range.clone()),
            port_range: range,
            protocol,
        }
    }

    pub fn next(&mut self) -> Result<VirtualPort> {
        self.queue
            .pop_front()
            .map(|p| VirtualPort(self.protocol, p))
            .ok_or(Error::new(ErrorKind::PortPoolExhausted))
    }

    #[instrument(skip(self), err(level = Level::WARN))]
    pub fn release(&mut self, port: VirtualPort) -> Result<()> {
        if !self.port_range.contains(&port.1) {
            return Err(Error::new(ErrorKind::BadValue("port outside range")));
        } else if self.protocol != port.0 {
            return Err(Error::new(ErrorKind::BadValue("wrong protocol")));
        } else if self.queue.contains(&port.1) {
            return Err(Error::new(ErrorKind::BadValue("port already in pool")));
        }

        self.queue.push_back(port.1);

        Ok(())
    }
}
