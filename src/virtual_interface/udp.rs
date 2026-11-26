use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
};

use bytes::Bytes;
use smoltcp::{
    iface::{Config, Interface, PollResult, SocketHandle, SocketSet},
    socket::udp::{self, PacketMetadata, UdpMetadata},
    time::Instant,
    wire::{HardwareAddress, IpAddress},
};
use tokio_util::sync::CancellationToken;
use tracing::{Level, debug, error, info, instrument, trace, warn};

use crate::{
    Result,
    bus::{
        Event::{self},
        EventBus,
    },
    config::{PortForwardRule, Protocol, VirtualPort},
    virtual_interface::{InterfacePollSleep, get_addresses},
    virtual_net_stack::VirtualNetStack,
};

const DATA_BUFFER_SIZE: usize = 64 * 1024;
const META_BUFFER_SIZE: usize = 16;

pub struct UdpVirtualInterface {
    origin_peer_ip: IpAddr,
    bus: EventBus,
    device: VirtualNetStack,
    sockets: SocketSet<'static>,
    interface: Interface,
    virtual_port_handle_map: HashMap<VirtualPort, SocketHandle>,
    packet_queues: HashMap<VirtualPort, VecDeque<(PortForwardRule, Bytes)>>,
    poll_sleep: InterfacePollSleep,
}

impl UdpVirtualInterface {
    #[instrument(name = "udp_virt_int_new", skip_all, fields(origin_peer_ip = %origin_peer_ip))]
    pub fn new(
        origin_peer_ip: IpAddr,
        port_forwards: &[PortForwardRule],
        bus: EventBus,
        mut device: VirtualNetStack,
    ) -> Result<Self> {
        debug!("Initialising UDP virtual interface.");

        // Construct a list of CIDR addresses based on the port forward rules and origin address.
        let addresses = get_addresses(origin_peer_ip, port_forwards, Protocol::Udp);
        let config = Config::new(HardwareAddress::Ip);

        debug!("Creating interface. Addresses: {addresses:?}");

        let instant = Instant::now();
        let mut interface = Interface::new(config, &mut device, instant);

        interface.update_ip_addrs(|ip_addresses| {
            addresses.into_iter().for_each(|addr| {
                // SAFETY: Unlikely to exceed capacity (requires 2^64+ addresses).
                ip_addresses
                    .push(addr)
                    .expect("Pushing address into vec succeeds")
            })
        });

        let mut sockets = SocketSet::new([]);

        for port_forward in port_forwards {
            debug!("Initialising server socket with port forward rule '{port_forward}'.");
            sockets.add(Self::create_server_socket(port_forward)?);
        }

        Ok(Self {
            origin_peer_ip,
            bus,
            device,
            sockets: SocketSet::new([]),
            interface,
            virtual_port_handle_map: HashMap::new(),
            packet_queues: HashMap::new(),
            poll_sleep: InterfacePollSleep::NeverWake,
        })
    }

    #[instrument(name = "udp_virt_int", skip_all, err(level = Level::ERROR))]
    pub async fn run_loop(mut self, token: CancellationToken) -> Result<()> {
        info!("UDP VIRT INTERFACE - Started.");

        let mut receiver = self.bus.subscribe();

        debug!("Spawning device feeder task.");

        self.device.spawn_feeder_task(token.clone());

        loop {
            let poll_sleep = self.poll_sleep;

            // The `biased` ensures we always poll the receiver first.
            // Since the `poll_sleep` can always return, it takes away from the fairness
            // of the receiver.
            tokio::select! {
                biased;
                event = receiver.recv_or_panic() => {
                    if let Err(e) = self.process_event(event).await {
                        warn!("Failed to process event. Error: {e}");
                    }
                },
                _ = poll_sleep.sleep() => {
                    if let Err(e) = self.process_pending_stack_packets().await {
                        warn!("Failed to process virtual interface packets. Error: {e}");
                    }
                },
                _ = token.cancelled() => {
                    debug!("Cancellation caught. exiting.");

                    break;
                }
            }
        }

        info!("UDP VIRT INTERFACE - Stopped.");

        Ok(())
    }

    /// Perform a poll on the virtual interface stack, sending and receiving any due packets.
    #[instrument(skip_all, err(level = Level::WARN))]
    async fn process_pending_stack_packets(&mut self) -> Result<()> {
        let processing_start = Instant::now();

        let poll_result =
            self.interface
                .poll(processing_start, &mut self.device, &mut self.sockets);

        if poll_result == PollResult::SocketStateChanged {
            trace!("Poll completed. Socket has pending state changes.");
        }

        self.send_pending_packets();

        self.recv_pending_packets();

        // Once processing is done, calculate the next due sleep interval.
        let advisory_duration = self
            .interface
            .poll_delay(processing_start, &mut self.sockets);
        self.poll_sleep = InterfacePollSleep::from_advisory_duration(
            advisory_duration,
            self.virtual_port_handle_map.len(),
        );

        Ok(())
    }

    fn send_pending_packets(&mut self) {
        for (virtual_port, handle) in &self.virtual_port_handle_map {
            let client_socket = self.sockets.get_mut::<udp::Socket>(*handle);

            if !client_socket.can_send() {
                continue;
            }

            let Some(send_queue) = self.packet_queues.get_mut(virtual_port) else {
                warn!("No packet queue for virtual port {virtual_port}");
                continue;
            };

            let Some((forward_rule, packet_to_send)) = send_queue.pop_front() else {
                continue;
            };

            // Try send the whole packet. If not all the packet bytes were sent,
            // the socket buffer is full. In which case, push the packet back on top.
            let metadata = UdpMetadata::from(forward_rule.dest_addr);

            if let Err(e) = client_socket.send_slice(&packet_to_send, metadata) {
                warn!(
                    "Failed to send {}-byte datagram. Error: {e}",
                    packet_to_send.len()
                );
            }
        }
    }

    fn recv_pending_packets(&mut self) {
        for (virtual_port, handle) in &self.virtual_port_handle_map {
            let client_socket = self.sockets.get_mut::<udp::Socket>(*handle);

            if !client_socket.can_recv() {
                continue;
            }

            let (packet, _metadata) = match client_socket.recv() {
                Ok(received) => received,
                Err(e) => {
                    error!("Failed to receive packet. Error: {e}");
                    continue;
                }
            };

            if let Err(e) = self.bus.send(Event::LocalInboundPacket(
                *virtual_port,
                Bytes::copy_from_slice(packet),
            )) {
                error!("Failed to send packet. Error: {e}");
            }
        }
    }

    /// Queue events to be handled by the virtual interface.
    async fn process_event(&mut self, event: Event) -> Result<()> {
        match event {
            Event::LocalOutboundPacket(forward_rule, virtual_port, datagram) => {
                if let Some(packet_queue) = self.packet_queues.get_mut(&virtual_port) {
                    packet_queue.push_back((forward_rule, datagram));
                } else {
                    let client_socket = self.create_client_socket(virtual_port)?;
                    let client_handle = self.sockets.add(client_socket);

                    self.virtual_port_handle_map
                        .insert(virtual_port, client_handle);
                    self.packet_queues.insert(virtual_port, VecDeque::new());
                }

                self.poll_sleep.set_immediate();
                Ok(())
            }
            Event::DeviceReceivedNewPacket(Protocol::Udp) => {
                self.poll_sleep.set_immediate();
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn create_server_socket(forward_rule: &PortForwardRule) -> Result<udp::Socket<'static>> {
        let mut socket = Self::create_socket(META_BUFFER_SIZE, DATA_BUFFER_SIZE);

        socket.bind((forward_rule.dest_addr.ip(), forward_rule.dest_addr.port()))?;

        Ok(socket)
    }

    fn create_client_socket(&mut self, virtual_port: VirtualPort) -> Result<udp::Socket<'static>> {
        let mut socket = Self::create_socket(META_BUFFER_SIZE, DATA_BUFFER_SIZE);

        socket.bind((IpAddress::from(self.origin_peer_ip), virtual_port.1))?;

        Ok(socket)
    }

    /// Create a UDP socket with the given metadata buffer size and data buffer size.
    fn create_socket(meta_buff_size: usize, data_buff_size: usize) -> udp::Socket<'static> {
        let tx_meta = vec![PacketMetadata::EMPTY; meta_buff_size];
        let tx_data = vec![0u8; data_buff_size];
        let rx_meta = vec![PacketMetadata::EMPTY; meta_buff_size];
        let rx_data = vec![0u8; data_buff_size];

        let rx_buffer = udp::PacketBuffer::new(rx_meta, rx_data);
        let tx_buffer = udp::PacketBuffer::new(tx_meta, tx_data);

        udp::Socket::new(rx_buffer, tx_buffer)
    }
}
