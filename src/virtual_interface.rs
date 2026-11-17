use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::net::IpAddr;

use bytes::Bytes;
use smoltcp::iface::Config;
use smoltcp::iface::Interface;
use smoltcp::iface::SocketHandle;
use smoltcp::iface::SocketSet;
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::HardwareAddress;
use smoltcp::wire::IpAddress;
use smoltcp::wire::IpCidr;
use smoltcp::wire::Ipv4Cidr;
use smoltcp::wire::Ipv6Cidr;
use tokio::sync::broadcast::Receiver;
use tokio_util::sync::CancellationToken;
use tracing::trace;
use tracing::{Level, debug, info, instrument, warn};

use crate::Result;
use crate::bus::Event;
use crate::bus::EventBus;
use crate::config::PortForwardRule;
use crate::config::Protocol;
use crate::config::VirtualPort;
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
}

impl TcpVirtualInterface {
    #[instrument(name = "tcp_virt_int_new", skip_all, fields(origin_peer_ip = %origin_peer_ip))]
    pub fn new(
        origin_peer_ip: IpAddr,
        port_forwards: &[PortForwardRule],
        bus: EventBus,
        mut device: VirtualNetStack,
    ) -> Self {
        // Construct a list of CIDR addresses based on the port forward rules and origin address.
        debug!("Initialising TCP virtual interface.");

        let addresses = get_addresses(origin_peer_ip, port_forwards, Protocol::Tcp);
        let config = Config::new(HardwareAddress::Ip);

        debug!("Creating interface. Addresses: {addresses:?}");

        let mut interface = Interface::new(config, &mut device, Instant::now());

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
        }
    }

    #[instrument(name = "tcp_virt_int", skip_all, err(level = Level::ERROR))]
    pub async fn run_loop(mut self, token: CancellationToken) -> Result<()> {
        info!("TCP VIRT INTERFACE - Started.");

        let mut receiver = self.bus.subscribe();

        loop {
            tokio::select! {
                res = self.recv_event(&mut receiver) => {
                    if let Err(e) = res {
                        warn!("Failed to process event. Error: {e}");
                    }
                },
                _ = token.cancelled() => {
                    debug!("Cancellation caught. exiting.");

                    break;
                }
            }
        }

        info!("TCP VIRT INTERFACE - Stopped.");

        Ok(())
    }

    async fn recv_event(&mut self, receiver: &mut Receiver<Event>) -> Result<()> {
        match receiver.recv().await? {
            Event::ClientConnected(port_forward, virtual_port) => {
                self.process_connected_client(port_forward, virtual_port)
                    .await
            }
            _ => Ok(()),
        }
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

        let client_socket = self.sockets.get_mut::<tcp::Socket>(handle);
        let context = self.interface.context();

        let local_endpoint = (self.origin_peer_ip, virtual_port.1);

        trace!(
            "Connecting client via {}->{}:{}",
            port_forward.dest_addr, local_endpoint.0, local_endpoint.1
        );
        client_socket.connect(context, port_forward.dest_addr, local_endpoint)?;

        trace!("Client socket configured.");

        Ok(())
    }

    fn create_client_socket() -> tcp::Socket<'static> {
        let tx_bytes = vec![0; MAX_PACKET_BYTES];
        let rx_bytes = vec![0; MAX_PACKET_BYTES];

        let tx_buffer = tcp::SocketBuffer::new(tx_bytes);
        let rx_buffer = tcp::SocketBuffer::new(rx_bytes);

        tcp::Socket::new(rx_buffer, tx_buffer)
    }
}

fn get_addresses(
    peer_ip: IpAddr,
    port_forwards: &[PortForwardRule],
    filter_protocol: Protocol,
) -> Vec<IpCidr> {
    let mut set = HashSet::new();

    set.insert(peer_ip);

    port_forwards
        .into_iter()
        .filter(|pf| pf.protocol == filter_protocol)
        .for_each(|pf| {
            set.insert(pf.dest_addr.ip().into());
        });

    set.into_iter()
        .map(|ip| IpAddress::from(ip))
        .map(addr_to_cidr)
        .collect()
}

/// convert an address to a CIDR with only one address in the subnet.
const fn addr_to_cidr(ip: IpAddress) -> IpCidr {
    match ip {
        IpAddress::Ipv4(addr) => IpCidr::Ipv4(Ipv4Cidr::new(addr, 32)),
        IpAddress::Ipv6(addr) => IpCidr::Ipv6(Ipv6Cidr::new(addr, 128)),
    }
}
