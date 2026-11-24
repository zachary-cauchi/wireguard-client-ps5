use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::IpAddr;

use bytes::Bytes;
use smoltcp::iface::Config;
use smoltcp::iface::Interface;
use smoltcp::iface::PollResult;
use smoltcp::iface::SocketHandle;
use smoltcp::iface::SocketSet;
use smoltcp::socket::tcp;
use smoltcp::socket::tcp::SocketBuffer;
use smoltcp::socket::tcp::State;
use smoltcp::time::Instant;
use smoltcp::wire::HardwareAddress;
use tokio_util::sync::CancellationToken;
use tracing::error;
use tracing::trace;
use tracing::{Level, debug, info, instrument, warn};

use crate::ErrorKind;
use crate::Result;
use crate::bus::Event;
use crate::bus::EventBus;
use crate::config::PortForwardRule;
use crate::config::Protocol;
use crate::config::VirtualPort;
use crate::result::Error;
use crate::virtual_interface::InterfacePollSleep;
use crate::virtual_interface::get_addresses;
use crate::virtual_net_stack::VirtualNetStack;

const MAX_PACKET_BYTES: usize = 1024 * 64;

pub struct TcpVirtualInterface {
    /// Local address of this host (outside of the VPN).
    origin_peer_ip: IpAddr,
    bus: EventBus,
    /// Virtual device which routes packets to clients from the stack.
    device: VirtualNetStack,
    /// Set of sockets servicing clients.
    sockets: SocketSet<'static>,
    /// Underlying virtual network interface.
    interface: Interface,
    /// Mapping of virtual ports to their respective client sockets.
    virtual_port_handle_map: HashMap<VirtualPort, SocketHandle>,
    /// Queues of pending packets to their respective ports.
    packet_queues: HashMap<VirtualPort, VecDeque<Bytes>>,
    poll_sleep: InterfacePollSleep,
}

impl TcpVirtualInterface {
    #[instrument(name = "tcp_virt_int_new", skip_all, fields(origin_peer_ip = %origin_peer_ip))]
    pub fn new(
        origin_peer_ip: IpAddr,
        port_forwards: &[PortForwardRule],
        bus: EventBus,
        mut device: VirtualNetStack,
    ) -> Self {
        debug!("Initialising TCP virtual interface.");

        // Construct a list of CIDR addresses based on the port forward rules and origin address.
        let addresses = get_addresses(origin_peer_ip, port_forwards, Protocol::Tcp);
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

        Self {
            origin_peer_ip,
            bus,
            device,
            sockets: SocketSet::new([]),
            interface,
            virtual_port_handle_map: HashMap::new(),
            packet_queues: HashMap::new(),
            poll_sleep: InterfacePollSleep::NeverWake,
        }
    }

    #[instrument(name = "tcp_virt_int", skip_all, err(level = Level::ERROR))]
    pub async fn run_loop(mut self, token: CancellationToken) -> Result<()> {
        info!("TCP VIRT INTERFACE - Started.");

        let mut receiver = self.bus.subscribe();

        debug!("Spawning device feeder task.");

        self.device.spawn_feeder_task(token.clone());

        loop {
            let poll_sleep = self.poll_sleep;

            tokio::select! {
                _ = poll_sleep.sleep() => {
                    if let Err(e) = self.process_pending_stack_packets().await {
                        warn!("Failed to process virtual interface packets. Error: {e}");
                    }
                },
                res = receiver.recv() => {
                    let event = match res {
                        Ok(event) => event,
                        Err(e) => {
                            warn!("Failed to receive event. Error: {e}");
                            continue;
                        }
                    };

                    if let Err(e) = self.process_event(event).await {
                        warn!("Failed to process event. Error: {e}");
                    }
                }

                _ = token.cancelled() => {
                    debug!("Cancellation caught. exiting.");

                    break;
                }
            }
        }

        info!("TCP VIRT INTERFACE - Stopped.");

        Ok(())
    }

    /// Process packets to and from the interface.
    /// The following steps are done in order.
    /// * Remove any disconnected clients.
    /// * Perform a poll operation on the interface, filling and draining the rx/tx buffers.
    /// * For each virtual port and handle, try send and receive packets if the socket can send/receive.
    /// * Calculate the next advised poll deadline.
    #[instrument(skip_all, err(level = Level::WARN))]
    async fn process_pending_stack_packets(&mut self) -> Result<()> {
        let processing_start = Instant::now();

        self.remove_disconnected_clients();

        let poll_result =
            self.interface
                .poll(processing_start, &mut self.device, &mut self.sockets);

        if poll_result == PollResult::SocketStateChanged {
            trace!("Poll completed. Socket has pending state changes.");
        }

        self.send_pending_packets();

        self.receive_incoming_packets();

        // Once processing is done, calculate the next due sleep interval.
        let advisory_duration = self.interface.poll_delay(processing_start, &self.sockets);
        self.poll_sleep = InterfacePollSleep::from_advisory_duration(
            advisory_duration,
            self.virtual_port_handle_map.len(),
        );

        Ok(())
    }

    /// Send any queued packets to their relevant sockets.
    #[instrument(skip_all)]
    fn send_pending_packets(&mut self) {
        for (virtual_port, handle) in &self.virtual_port_handle_map {
            // SAFETY: We never insert non-TCP sockets into this set.
            let client_socket = self.sockets.get_mut::<tcp::Socket>(*handle);

            if !client_socket.can_send() {
                trace!("Socket not yet ready to transmit.");
                continue;
            }

            let Some(send_queue) = self.packet_queues.get_mut(&virtual_port) else {
                warn!("No packet queue for virtual port {virtual_port}.");
                continue;
            };

            let Some(packet_to_send) = send_queue.pop_front() else {
                continue;
            };

            // Try send the whole packet. If not all the packet bytes were sent,
            // the socket buffer is full. In which case, push the packet back on top.
            let packet_len = packet_to_send.len();

            match client_socket.send_slice(&packet_to_send) {
                Ok(sent_count) if sent_count < packet_len => {
                    let remainder = Bytes::copy_from_slice(&packet_to_send[sent_count..packet_len]);
                    send_queue.push_front(remainder);

                    trace!("Sent {sent_count} bytes of {packet_len}-byte packet.");
                }
                Ok(send_count) => {
                    trace!("Sent {send_count} bytes to virtual port {virtual_port}.")
                }
                Err(e) => {
                    error!("Failed to send packet to virtual port {virtual_port}. Error: {e}");
                }
            }
        }
    }

    #[instrument(skip_all)]
    fn receive_incoming_packets(&mut self) {
        for (virtual_port, handle) in &self.virtual_port_handle_map {
            // SAFETY: We never insert non-TCP sockets into this set.
            let client_socket = self.sockets.get_mut::<tcp::Socket>(*handle);

            if !client_socket.can_recv() {
                continue;
            }

            let data = match client_socket.recv(|b| (b.len(), Bytes::copy_from_slice(b))) {
                Ok(data) => data,
                Err(e) => {
                    error!("Failed to receive bytes from virtual port {virtual_port}. Error: {e}");
                    continue;
                }
            };

            trace!(
                "Received {} bytes from virtual port {virtual_port}.",
                data.len()
            );

            if !data.is_empty() {
                if let Err(e) = self
                    .bus
                    .send(Event::LocalInboundPacket(*virtual_port, data))
                {
                    error!("Failed to send LocalInboundPacket event. Error: {e}");
                }
            }
        }
    }

    fn remove_disconnected_clients(&mut self) {
        self.virtual_port_handle_map.retain(|virtual_port, handle| {
            let client_socket = self.sockets.get_mut::<tcp::Socket>(*handle);
            if client_socket.state() != State::Closed {
                return true;
            }

            let _ = self.bus.send(Event::ClientDisconnected(*virtual_port));
            let _ = self.packet_queues.remove(virtual_port);
            self.sockets.remove(*handle);

            false
        });
    }

    async fn process_event(&mut self, event: Event) -> Result<()> {
        use Event::*;
        match event {
            ClientConnected(port_forward, virtual_port) => {
                self.process_connected_client(port_forward, virtual_port)
                    .await
            }
            LocalOutboundPacket(_, virtual_port, packet)
                if self.virtual_port_handle_map.contains_key(&virtual_port) =>
            {
                self.process_local_outbound_packet(virtual_port, packet)
                    .await
            }
            ClientDisconnected(virtual_port) => {
                self.process_disconnected_client(virtual_port).await
            }
            DeviceReceivedNewPacket(Protocol::Tcp) => self.process_device_received_new_packet(),
            _ => Ok(()),
        }
    }

    /// The virtual device received new packets.
    /// Set the next sleep to `Immediate` so we process them immediately.
    fn process_device_received_new_packet(&mut self) -> Result<()> {
        self.poll_sleep.set_immediate();
        Ok(())
    }

    /// Queue a given outbound packet for routing.
    #[instrument("outbound_packet", skip(self, packet), err(level = Level::ERROR))]
    async fn process_local_outbound_packet(
        &mut self,
        virtual_port: VirtualPort,
        packet: Bytes,
    ) -> Result<()> {
        trace!("Received outbound packet. Packet: {packet:?}");

        let queue = self
            .packet_queues
            .get_mut(&virtual_port)
            .ok_or(Error::new(ErrorKind::BadValue("Missing packet queue")))?;

        queue.push_back(packet);
        self.poll_sleep.set_immediate();

        Ok(())
    }

    /// Process a newly-connected client socke and assigned virtual port.
    /// Registers a socket in the virtual network interface and keeps track of the virtual socket.
    #[instrument(skip_all, fields(virtual_port = %virtual_port, dest_addr = %port_forward.dest_addr) )]
    async fn process_connected_client(
        &mut self,
        port_forward: PortForwardRule,
        virtual_port: VirtualPort,
    ) -> Result<()> {
        debug!("Processing connected client event. Applicable rule: {port_forward}");

        let socket = Self::create_client_socket();
        let handle = self.sockets.add(socket);

        trace!("Created and inserted socket handle.");

        self.virtual_port_handle_map.insert(virtual_port, handle);
        self.packet_queues.insert(virtual_port, VecDeque::new());

        let client_socket = self.sockets.get_mut::<tcp::Socket>(handle);
        let context = self.interface.context();

        let local_endpoint = (self.origin_peer_ip, virtual_port.1);

        trace!(
            "Connecting client via {}->{}:{}",
            port_forward.dest_addr, local_endpoint.0, local_endpoint.1
        );
        client_socket.connect(context, port_forward.dest_addr, local_endpoint)?;

        trace!("Client socket configured.");

        self.poll_sleep.set_immediate();

        Ok(())
    }

    /// Find a client socket by the given socket and close it on the virtual interface.
    /// This also queues the socket to be removed and the virtual port freed.
    #[instrument(skip(self), err(level = Level::ERROR))]
    async fn process_disconnected_client(&mut self, virtual_port: VirtualPort) -> Result<()> {
        let Some(handle) = self.virtual_port_handle_map.get(&virtual_port) else {
            // Socket must already have been disconnected.
            return Ok(());
        };

        trace!("Closing socket with handle {handle}.");

        let client_socket = self.sockets.get_mut::<tcp::Socket<'_>>(*handle);
        client_socket.close();

        self.poll_sleep.set_immediate();

        Ok(())
    }

    fn create_client_socket() -> tcp::Socket<'static> {
        let tx_bytes = vec![0; MAX_PACKET_BYTES];
        let rx_bytes = vec![0; MAX_PACKET_BYTES];

        let tx_buffer = SocketBuffer::new(tx_bytes);
        let rx_buffer = SocketBuffer::new(rx_bytes);

        tcp::Socket::new(rx_buffer, tx_buffer)
    }
}
