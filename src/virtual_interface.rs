use std::{collections::HashSet, net::IpAddr};

use smoltcp::{
    time::Duration,
    wire::{IpAddress, IpCidr, Ipv4Cidr, Ipv6Cidr},
};

use crate::config::{PortForwardRule, Protocol};

pub mod tcp;
pub mod udp;

/// Wrapper for sleeping at according to the advised delays from the virtual interface.
#[derive(Clone, Copy, Debug)]
enum InterfacePollSleep {
    /// Sleep forever.
    NeverWake,
    /// Don't sleep at all.
    Immediate,
    /// Sleep until this deadline.
    Until(tokio::time::Instant),
}

impl InterfacePollSleep {
    /// Create [Self] from the an optional duration
    /// (the output of `smoltcp::iface::interface::Interface::poll_delay`)
    /// and the number of assigned virtual ports.
    pub fn from_advisory_duration(advisory: Option<Duration>, assigned_ports_count: usize) -> Self {
        match (advisory, assigned_ports_count) {
            (None, 0) => Self::NeverWake,
            (None, _) => Self::Immediate,
            (Some(Duration::ZERO), _) => Self::Immediate,
            (Some(duration), _) => {
                // let duration = std::time::Duration::from_millis(duration.total_millis());
                Self::Until(tokio::time::Instant::now() + duration.into())
            }
        }
    }

    pub fn set_immediate(&mut self) {
        *self = Self::Immediate;
    }

    pub async fn sleep(&self) {
        match self {
            Self::Immediate => (),
            Self::NeverWake => tokio::time::sleep(Duration::MAX.into()).await,
            Self::Until(deadline) => tokio::time::sleep_until(*deadline).await,
        }
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
    use IpAddress::*;
    match ip {
        Ipv4(addr) => IpCidr::Ipv4(Ipv4Cidr::new(addr, 32)),
        Ipv6(addr) => IpCidr::Ipv6(Ipv6Cidr::new(addr, 128)),
    }
}
