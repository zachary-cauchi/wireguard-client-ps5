use std::collections::VecDeque;
use std::net::SocketAddr;
use std::ops::Range;
use std::sync::{Arc, RwLock};

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use tracing::{Level, debug, info, instrument, trace, warn};

use crate::bus::{Event, EventBus};
use crate::config::{PortForwardRule, Protocol, VirtualPort};
use crate::result::{Error, WrapWithMsg};
use crate::{ErrorKind, Result};

/// Handles spawning of TCP streams from incoming connections and outbound events.
/// Assigns a virtual port to the connection and forwards them through the event bus.
pub struct TcpClientsSubsystem {
    /// Modify to either manage more than one rule, or spawn tasks for each.
    port_forward_rule: PortForwardRule,
    socket: TcpListener,
    bus: EventBus,
    virtual_port_pool: TcpPortPool,
}

impl TcpClientsSubsystem {
    pub async fn new(bus: EventBus, port_forward_rule: PortForwardRule) -> Result<Self> {
        if port_forward_rule.protocol != Protocol::Tcp {
            return Err(Error::new(ErrorKind::BadValue("Non-TCP rule supplied")));
        }

        let socket = TcpListener::bind(port_forward_rule.source_addr).await?;
        let virtual_port_pool = TcpPortPool::new();

        Ok(Self {
            socket,
            bus,
            port_forward_rule,
            virtual_port_pool,
        })
    }

    #[instrument(name = "tcp_clients", skip_all, err(level = Level::ERROR))]
    pub async fn run_loop(self, token: CancellationToken) -> Result<()> {
        info!("TCP CLIENTS - Started.");

        loop {
            tokio::select! {
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
                _ = token.cancelled() => {
                    info!("Cancellation triggered. Exiting.");
                    break;
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
            .context("Selecting virtual TCP port")?;

        debug!("Assigning virtual port {assigned_virtual_port}.");

        let handler = TcpClientHandler {
            stream,
            peer,
            bus: self.bus.clone(),
            port_pool: self.virtual_port_pool.clone(),
            virtual_port: assigned_virtual_port,
            port_forward_rule: self.port_forward_rule.clone(),
            is_closed: false,
        };

        tokio::spawn(token.run_until_cancelled_owned(handler.run_loop()));

        Ok(())
    }
}

/// Wraps and manages a client TCP stream, routing packets to/from the client.
struct TcpClientHandler {
    stream: TcpStream,
    peer: SocketAddr,
    bus: EventBus,
    port_pool: TcpPortPool,
    virtual_port: VirtualPort,
    port_forward_rule: PortForwardRule,
    is_closed: bool,
}

impl TcpClientHandler {
    #[instrument(name = "tcp_client", skip_all, fields(peer = %self.peer, virtual_port = %self.virtual_port))]
    async fn run_loop(mut self) {
        info!(
            "Client started with port forwarding rule '{}'.",
            self.port_forward_rule
        );

        if let Err(e) = self.bus.send(Event::ClientConnected(
            self.port_forward_rule.clone(),
            self.virtual_port,
        )) {
            warn!("Failed to send ClientConnected notif. Error: {e}");
            return;
        };

        let mut rx_buf = [0; 1024 * 64];

        let mut bus_recv = self.bus.subscribe();

        loop {
            if self.is_closed {
                debug!("Stream is closed.");
                break;
            }

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
                event = bus_recv.recv_or_panic() => {
                    if let Err(e) = self.process_event(event).await {
                        warn!("Error encountered when processing event. Error: {e}")
                    }
                }
            }
        }

        if let Err(e) = self.port_pool.release(self.virtual_port) {
            warn!("Failed to release port. Error: {e}");
        }

        if let Err(e) = self.bus.send(Event::ClientDisconnected(self.virtual_port)) {
            warn!("Failed to send client disconnection event. Error: {e}");
        }

        info!("Client connection closed.");
    }

    async fn process_packet(&self, packet: &[u8]) -> Result<()> {
        debug!("Received {}-byte packet: {:?}", packet.len(), packet);

        let raw_packet = Bytes::copy_from_slice(packet);

        self.bus.send(Event::LocalOutboundPacket(
            self.port_forward_rule.clone(),
            self.virtual_port,
            raw_packet,
        ))
    }

    async fn process_event(&mut self, event: Event) -> Result<()> {
        debug!("Received event. Event: {event:?}");
        match event {
            Event::ClientDisconnected(vp) if vp == self.virtual_port => {
                trace!("Client disconnected from virtual interface. Closing TCP connection.");

                self.is_closed = true;
                self.stream.shutdown().await?;
            }
            Event::LocalInboundPacket(vp, packet) if vp == self.virtual_port => {
                trace!(
                    "Sending packet from virtual port {vp} to client ({}).",
                    self.peer
                );

                self.stream
                    .writable()
                    .await
                    .context("Waiting to write packet to peer stream")?;

                self.stream
                    .write_all(&packet)
                    .await
                    .context("Writing the packet to the peer stream")?;
            }
            _ => {}
        }

        Ok(())
    }
}

/// Represents a pool of available ports for assignment.
#[derive(Clone)]
pub struct TcpPortPool {
    inner: Arc<RwLock<TcpPortPoolInner>>,
}

impl TcpPortPool {
    pub const PORT_RANGE: Range<u16> = 60001u16..60019u16;

    pub fn new() -> Self {
        let inner = TcpPortPoolInner::new(Self::PORT_RANGE);

        Self {
            inner: Arc::new(RwLock::new(inner)),
        }
    }

    pub fn next(&self) -> Result<VirtualPort> {
        self.inner.write()?.next()
    }

    pub fn release(&self, port: VirtualPort) -> Result<()> {
        self.inner.write()?.release(port)
    }
}

struct TcpPortPoolInner {
    queue: VecDeque<u16>,
    port_range: Range<u16>,
}

impl TcpPortPoolInner {
    pub fn new(range: Range<u16>) -> Self {
        Self {
            queue: VecDeque::from_iter(range.clone()),
            port_range: range,
        }
    }

    pub fn next(&mut self) -> Result<VirtualPort> {
        self.queue
            .pop_front()
            .map(|p| VirtualPort(Protocol::Tcp, p))
            .ok_or(Error::new(ErrorKind::PortPoolExhausted))
    }

    #[instrument(skip(self), err(level = Level::WARN))]
    pub fn release(&mut self, port: VirtualPort) -> Result<()> {
        if !self.port_range.contains(&port.1) {
            return Err(Error::new(ErrorKind::BadValue("port outside range")));
        } else if port.0 != Protocol::Tcp {
            return Err(Error::new(ErrorKind::BadValue("wrong protocol")));
        } else if self.queue.contains(&port.1) {
            return Err(Error::new(ErrorKind::BadValue("port already in pool")));
        }

        self.queue.push_back(port.1);

        Ok(())
    }
}
