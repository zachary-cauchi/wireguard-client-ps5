use std::net::IpAddr;
use std::sync::Arc;

use boringtun::noise::TunnResult;
use bytes::Bytes;
use smoltcp::wire::{IpProtocol, IpVersion, Ipv4Packet, Ipv6Packet, PrettyPrinter};
use tokio_util::sync::CancellationToken;
use tracing::{Level, debug, enabled, info, instrument, trace, warn};

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
            tokio::select! {
                res = self.tunnel.endpoint.recv(&mut rx_buf) => {
                    let datagram = match res {
                        Ok(datagram) => datagram,
                        Err(e) => {
                            warn!("Error waiting for incoming datagrams. Error: {e}");
                            continue;
                        }
                    };

                    if let Err(e) = self.rx_entry(datagram, &mut tx_buf).await {
                        warn!("Error processing datagram. Error: {e}");
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

    /// Processes a new incoming datagram.
    async fn rx_entry(&self, datagram: &[u8], tx_buf: &mut [u8]) -> Result<()> {
        trace!(
            "Received {}-byte packet. Processing. Packet: {datagram:?}",
            datagram.len()
        );

        let tunn_result = self
            .tunnel
            .tunn
            .lock()
            .await
            .decapsulate(None, datagram, tx_buf);

        match tunn_result {
            TunnResult::Done => {}
            TunnResult::WriteToNetwork(new_packet) => {
                self.tunnel.endpoint.send_full_slice(new_packet).await?;
                let mut tunn = self.tunnel.tunn.lock().await;

                loop {
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
            }
            TunnResult::WriteToTunnelV4(packet, dst) => {
                self.send_packet(packet, IpAddr::V4(dst))?;
            }
            TunnResult::WriteToTunnelV6(packet, dst) => {
                self.send_packet(packet, IpAddr::V6(dst))?;
            }
            TunnResult::Err(e) => return Err(e.into()),
        }

        Ok(())
    }

    /// Send a received datagram onto the event bus.
    #[instrument(skip_all, fields(src_addr = %src_addr), err(level = Level::WARN))]
    fn send_packet(&self, pkt: &mut [u8], src_addr: IpAddr) -> Result<()> {
        let protocol = self
            .determine_transport_protocol(pkt)
            .ok_or(Error::new(ErrorKind::PacketParsing).with_msg("Packet filtered"))?;

        debug!("Forwarding received {protocol} packet.");

        let raw_packet = Bytes::copy_from_slice(pkt);

        self.bus
            .send(Event::PacketFromWG(src_addr, protocol, raw_packet))
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

                if enabled!(Level::TRACE) {
                    let printer = PrettyPrinter::<Ipv4Packet<&[u8]>>::new("", &packet);
                    trace!("Parsed IPv4 packet: {}", printer);
                }

                (IpAddr::from(packet.dst_addr()), packet.next_header())
            }
            Ok(IpVersion::Ipv6) => {
                let packet = Ipv6Packet::new_checked(packet)
                    .inspect_err(|e| warn!("IPv6 packet parsing failed. Error: {e}"))
                    .ok()?;

                if enabled!(Level::TRACE) {
                    let printer = PrettyPrinter::<Ipv6Packet<&[u8]>>::new("", &packet);
                    trace!("Parsed IPv6 packet: {}", printer);
                }

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
