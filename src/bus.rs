use std::net::IpAddr;

use bytes::Bytes;
use tokio::sync::broadcast::{self, Receiver, Sender, error::RecvError};
use tracing::{instrument, warn};

use crate::{
    Result,
    config::{PortForwardRule, Protocol, VirtualPort},
};

/// A broadcast-based event bus for relaying network packets or events.
#[derive(Clone)]
pub struct EventBus {
    sender: Sender<Event>,
}

impl EventBus {
    pub const CHANNEL_CAPACITY: usize = 64;

    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(Self::CHANNEL_CAPACITY);

        Self { sender }
    }

    pub fn subscribe(&self) -> BusReceiver {
        BusReceiver {
            receiver: self.sender.subscribe(),
        }
    }

    pub fn send(&self, event: Event) -> Result<()> {
        self.sender.send(event)?;
        Ok(())
    }
}

pub struct BusReceiver {
    receiver: Receiver<Event>,
}

impl BusReceiver {
    /// Receive an event message, expecting a successfull response.
    /// Panics if the receiver is closed.
    /// Warns if the receiver is lagging.
    #[instrument(name = "recv", skip_all)]
    pub async fn recv_or_panic(&mut self) -> Event {
        loop {
            match self.receiver.recv().await {
                Ok(event) => return event,
                Err(RecvError::Closed) => panic!("Receiver closed."),
                Err(RecvError::Lagged(n)) => warn!("Lagged by {n} messages."),
            }
        }
    }

    pub async fn recv(&mut self) -> Result<Event> {
        Ok(self.receiver.recv().await?)
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    /// A local client connected.
    ClientConnected(PortForwardRule, VirtualPort),
    /// A local client disconnected.
    ClientDisconnected(VirtualPort),
    /// A packet from a local client was received and needs to be routed through the virtual stack.
    LocalOutboundPacket(PortForwardRule, VirtualPort, Bytes),
    /// A packet from the virtual stack was received and needs to be routed to its respective local client.
    LocalInboundPacket(VirtualPort, Bytes),
    /// Packet received from the Wireguard peer.
    PacketFromWG(IpAddr, Protocol, Bytes),
    /// Packet going out to the Wireguard peer.
    PacketToWG(Bytes),
    /// Virtual interface device received new packets.
    DeviceReceivedNewPacket(Protocol),
}

// pub struct NewEventBus {
//     inner: HashMap<EventKey, (mpsc::Sender<Event>, Option<mpsc::Receiver<Event>>)>,
// }

// #[derive(Clone, Copy, Debug, Hash)]
// pub enum EventKey {
//     TcpClientConnected,
//     TcpClientDisconnected,
//     LocalOutboundPacket,
//     LocalInboundPacket,
//     PacketFromWG,
//     PacketToWG,
//     DeviceReceivedNewPacket(Protocol),
// }
