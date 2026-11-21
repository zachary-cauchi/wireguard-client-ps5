use std::net::IpAddr;
use std::sync::Arc;

use boringtun::noise::TunnResult;
use bytes::Bytes;
use smoltcp::wire::{IpProtocol, IpVersion, Ipv4Packet, Ipv6Packet};
use tokio_util::sync::CancellationToken;
use tracing::{Level, debug, info, instrument, trace, warn};

use crate::bus::{Event, EventBus};
use crate::config::Protocol;
use crate::result::Error;
use crate::tunnel::{MAX_BUF_SIZE, Tunnel};
use crate::{ErrorKind, Result};

/// Subsystem for handling incoming packets from a Wireguard tunnel.
pub struct RxSubsystem {
    tunnel: Arc<Tunnel>,
    bus: EventBus,
}

impl RxSubsystem {
    pub(super) fn new(tunnel: Arc<Tunnel>, bus: EventBus) -> Self {
        Self { tunnel, bus }
    }

    #[instrument(name = "rx", skip_all)]
    pub async fn run_loop(self, token: CancellationToken) -> Result<()> {
        let mut rx_buf = [0u8; MAX_BUF_SIZE];
        let mut tx_buf = [0u8; MAX_BUF_SIZE];

        info!("RX - Started.");

        loop {
            rx_buf.fill(0);

            tokio::select! {
                res = self.rx_entry(&mut rx_buf, &mut tx_buf) => {
                    if let Err(e) = res {
                        warn!("Error from rx task. Error: {e}");
                    }
                },
                _ = token.cancelled() => {
                    debug!("Cancellation caught. exiting.");

                    break;
                }
            }
        }

        info!("RX - Stopped.");

        Ok(())
    }

    async fn rx_entry(&self, rx_buf: &mut [u8], tx_buf: &mut [u8]) -> Result<()> {
        let raw_packet = self.tunnel.endpoint.recv(rx_buf).await?;

        trace!(
            "Received {}-byte packet. Processing. Packet: {raw_packet:?}",
            raw_packet.len()
        );

        let mut tunn = self.tunnel.tunn.lock().await;

        let tunn_result = tunn.decapsulate(None, raw_packet, tx_buf);

        match tunn_result {
            TunnResult::Done => {}
            TunnResult::WriteToNetwork(new_packet) => {
                self.tunnel.endpoint.send_full_slice(new_packet).await?;
            }
            TunnResult::WriteToTunnelV4(packet, dst) => {
                self.send_packet(packet, IpAddr::V4(dst)).await?;
            }
            TunnResult::WriteToTunnelV6(packet, dst) => {
                self.send_packet(packet, IpAddr::V6(dst)).await?;
            }
            TunnResult::Err(e) => return Err(e.into()),
        }

        loop {
            tx_buf.fill(0);

            let next_result = tunn.decapsulate(None, &[], tx_buf);

            match next_result {
                TunnResult::Done => {
                    trace!("All done, no more packets.");
                    break;
                }
                TunnResult::WriteToNetwork(pkt) => {
                    self.tunnel.endpoint.send_full_slice(pkt).await?
                }
                TunnResult::Err(e) => {
                    return Err(Error::from(e).with_msg("Error from decapsulating msg"));
                }
                _ => {
                    warn!("Unexpected result type (next_result:?).");
                    break;
                }
            }
        }

        Ok(())
    }

    #[instrument(skip_all, fields(target = %addr), err(level = Level::WARN))]
    async fn send_packet(&self, pkt: &mut [u8], addr: IpAddr) -> Result<()> {
        let protocol = self
            .determine_transport_protocol(pkt)
            .ok_or(Error::new(ErrorKind::PacketParsing).with_msg("Packet filtered"))?;

        debug!("Forwarding received {protocol} packet.");

        let raw_packet = Bytes::copy_from_slice(pkt);

        self.bus
            .send(Event::PacketFromWG(addr, protocol, raw_packet))
    }

    /// Get the transport protocol of the inbound packet, checking that the packet
    /// is meant for this client and is either TCP or UDP.
    fn determine_transport_protocol(&self, packet: &[u8]) -> Option<Protocol> {
        use IpProtocol::*;

        let (dst_addr, protocol) = match IpVersion::of_packet(packet) {
            Ok(IpVersion::Ipv4) => {
                let packet = Ipv4Packet::new_checked(packet)
                    .inspect_err(|e| warn!("IPv4 packet parsing failed. Error: {e}"))
                    .ok()?;

                (IpAddr::from(packet.dst_addr()), packet.next_header())
            }
            Ok(IpVersion::Ipv6) => {
                let packet = Ipv6Packet::new_checked(packet)
                    .inspect_err(|e| warn!("IPv6 packet parsing failed. Error: {e}"))
                    .ok()?;

                (IpAddr::from(packet.dst_addr()), packet.next_header())
            }
            Err(e) => {
                warn!("Failed to parse packet as IP packet. Error: {e}");
                return None;
            }
        };

        if dst_addr != self.tunnel.local_wg_addr {
            warn!(
                "{protocol} packet has dest address {dst_addr} but expected local address {}",
                self.tunnel.local_wg_addr
            );
            return None;
        }

        match protocol {
            Tcp | Icmp | Icmpv6 => Some(Protocol::Tcp),
            Udp => Some(Protocol::Udp),
            other => {
                warn!(
                    "Packet is due for this client, but unexpected protocol found ({other}). Not routing."
                );
                None
            }
        }
    }
}
