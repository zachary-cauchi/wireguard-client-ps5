use smoltcp::wire::{IpAddress, IpCidr, Ipv4Cidr, Ipv6Cidr};

pub mod tcp;

/// convert an address to a CIDR with only one address in the subnet.
const fn addr_to_cidr(ip: IpAddress) -> IpCidr {
    use IpAddress::*;
    match ip {
        Ipv4(addr) => IpCidr::Ipv4(Ipv4Cidr::new(addr, 32)),
        Ipv6(addr) => IpCidr::Ipv6(Ipv6Cidr::new(addr, 128)),
    }
}
