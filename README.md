# wireguard-client-ps5

A Wireguard client for the PS5.

## To Do list

* [x] Load config from file.
* [x] Establish connection with Wireguard peer.
* [x] Keep the connection alive via handshake.
* [x] Accept TCP connections from clients.
* [ ] Send packets onto Wireguard peer.
* [ ] Route packets from connecting clients to the Wireguard network.
* [ ] Configurable routes.
* [ ] Accept UDP connections from clients.
* [ ] Resolve DNS queries.

# References

* [rust-overlay](https://github.com/oxalica/rust-overlay)
* [wireguard-rs](https://github.com/DefGuard/wireguard-rs)
* [ps5-rust](https://github.com/neokuze/ps5-rust)
* [ps5-payload-sdk](https://github.com/ps5-payload-dev/sdk)
