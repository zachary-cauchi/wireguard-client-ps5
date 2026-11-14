# Research

## Requirements

* Find a library which implements the client-side Wireguard protocol.
* Needs to either be platform-independent, or support FreeBSD.
* No `/dev/tun` device needed.

### Planned

1. MVP which connects to the VPN network and appears as an active client.
2. Able to ping other VPN-connected clients.
3. Send HTTP/S requests to a VPN-connected server.

## Libraries and references.

### [boringtun](https://github.com/cloudflare/boringtun)

* Made by Cloudflare so has some trust behind it.
* The library implements the Wireguard tunnel.
* Userspace implementation.

### [tun-rs](https://github.com/tun-rs/tun-rs)

* Cross-platform TUN/TAP implementation.

### [ps5-tailscale docs](https://github.com/holdmysocks/ps5-tailscale/blob/main/docs/TECHNICAL.md)

* Outlines the limitations and challenges encountered on getting tailscale working on the PS5.
  * Several limitations:
    * No tunnel devices, meaning no kernel-level routing.

### [onetun](https://github.com/aramperes/onetun)

* A TUN-less Wireguard client based on `boringtun` and `smoltcp`.
  * `boringtun` handles the Wireguard protocol stuff while `smoltcp` does packet conversion and routing.
  * Seems to accomplish the same requirements and should thus serve as a good reference.

### [smoltcp](https://github.com/smoltcp-rs/smoltcp)

* A TCP/IP stack
