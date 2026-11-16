use std::net::SocketAddr;

use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn};

use crate::Result;
use crate::bus::{Event, EventBus, RawPacket};

/// Handles spawning of TCP streams from incoming connections and outbound events.
pub struct TcpClientsSubsystem {
    socket: TcpListener,
    bus: EventBus,
}

impl TcpClientsSubsystem {
    pub async fn new(bus: EventBus, addr: impl ToSocketAddrs) -> Result<Self> {
        let socket = TcpListener::bind(addr).await?;

        Ok(Self { socket, bus })
    }

    #[instrument(name = "tcp_clients", skip_all)]
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
                    match res {
                        Ok((socket, addr)) => self.spawn_socket_task(socket, addr, token),
                        Err(e) => warn!("Error accepting TCP connection. Error: {e}"),
                    }
                }
            }
        }

        info!("TCP CLIENTS - Stopped.");

        Ok(())
    }

    /// When receiving a new stream, spawn a task to handle that stream.
    fn spawn_socket_task(&self, stream: TcpStream, peer: SocketAddr, token: CancellationToken) {
        debug!("Spawning TCP stream handler. Peer address: {peer}");

        let handler = TcpPeerHandler {
            stream,
            peer,
            bus: self.bus.clone(),
        };

        tokio::spawn(token.run_until_cancelled_owned(handler.run_loop()));
    }
}

pub struct TcpPeerHandler {
    stream: TcpStream,
    peer: SocketAddr,
    bus: EventBus,
}

impl TcpPeerHandler {
    #[instrument(name = "tcp_peer", skip_all, fields(peer = %self.peer))]
    async fn run_loop(mut self) {
        debug!("Handling peer.");
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
