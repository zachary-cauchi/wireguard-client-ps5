use std::net::IpAddr;

use bytes::Bytes;
use tokio::sync::broadcast::{self, Receiver, Sender};

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
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(32);

        Self { sender }
    }

    pub fn subscribe(&self) -> Receiver<Event> {
        self.sender.subscribe()
    }

    pub fn send(&self, event: Event) -> Result<()> {
        self.sender.send(event)?;
        Ok(())
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
    DeviceReceivedNewPacket(Protocol),
}
