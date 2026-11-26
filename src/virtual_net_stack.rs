use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use bytes::{Bytes, BytesMut};
use smoltcp::phy::{DeviceCapabilities, Medium};
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument, warn};

use crate::Result;
use crate::bus::{Event, EventBus};
use crate::config::Protocol;
use crate::result::WrapWithMsg;

/// Defines a Virtual network device which routes IP packets.
pub struct VirtualNetStack {
    /// Support protocol of the stack.
    protocol: Protocol,
    /// Max Transmission Unit in bytes.
    mtu: usize,

    bus: EventBus,

    packet_queue: Arc<Mutex<VecDeque<Bytes>>>,
}

impl VirtualNetStack {
    pub fn new(protocol: Protocol, mtu: usize, bus: EventBus) -> Self {
        let packet_queue = Arc::new(Mutex::new(VecDeque::new()));

        Self {
            protocol,
            mtu,
            bus,
            packet_queue,
        }
    }

    /// Spawns a [VirtualNetStackFeeder] task bound to this device and token.
    pub fn spawn_feeder_task(&self, token: CancellationToken) {
        let feeder = VirtualNetStackFeeder {
            protocol: self.protocol,
            packet_queue: self.packet_queue.clone(),
            bus: self.bus.clone(),
        };

        tokio::spawn(token.run_until_cancelled_owned(async move {
            if let Err(e) = feeder.run_loop().await {
                warn!("Device feeder task exited. Error: {e}");
            }
        }));
    }
}

/// A Handles receiving of Wireguard peer packets meant for local clients and queues them.
struct VirtualNetStackFeeder {
    protocol: Protocol,
    packet_queue: Arc<Mutex<VecDeque<Bytes>>>,
    bus: EventBus,
}

impl VirtualNetStackFeeder {
    #[instrument(name = "device_feeder", skip_all, fields(proto = %self.protocol))]
    async fn run_loop(&self) -> Result<()> {
        let mut receiver = self.bus.subscribe();

        loop {
            if let Event::PacketFromWG(source_addr, protocol, packet) =
                receiver.recv_or_panic().await
                && protocol == self.protocol
            {
                debug!(
                    "Received {}-byte {protocol} packet from {source_addr}",
                    packet.len()
                );

                self.packet_queue.lock()?.push_back(packet);

                self.bus
                    .send(Event::DeviceReceivedNewPacket(protocol))
                    .context("Sending notif of received packet")?;
            }
        }
    }
}

impl smoltcp::phy::Device for VirtualNetStack {
    type RxToken<'a>
        = RxToken
    where
        Self: 'a;

    type TxToken<'a>
        = TxToken
    where
        Self: 'a;

    fn receive(
        &mut self,
        _: smoltcp::time::Instant,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let packet = self
            .packet_queue
            .lock()
            .inspect_err(|e| warn!("Packet queue is poisoned. Error: {e}"))
            .ok()?
            .pop_front()?;

        let tx_token = TxToken {
            bus: self.bus.clone(),
        };
        let rx_token = RxToken {
            buffer: BytesMut::from(packet),
        };

        Some((rx_token, tx_token))
    }

    fn transmit(&mut self, _: smoltcp::time::Instant) -> Option<Self::TxToken<'_>> {
        Some(TxToken {
            bus: self.bus.clone(),
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut cap = DeviceCapabilities::default();
        cap.max_transmission_unit = self.mtu;
        cap.medium = Medium::Ip;
        cap
    }
}

/// A struct for receiving an IP packet into the virtual network device.
pub struct RxToken {
    buffer: BytesMut,
}

impl smoltcp::phy::RxToken for RxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.buffer)
    }
}

/// A struct for transmitting an IP packet from the virtual network device onto the event bus.
pub struct TxToken {
    bus: EventBus,
}

impl smoltcp::phy::TxToken for TxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buffer = vec![0; len];

        let res = f(&mut buffer);

        if let Err(e) = self.bus.send(Event::PacketToWG(Bytes::from(buffer))) {
            warn!("Failed to send outbound packet {e}");
        }

        res
    }
}
