use boringtun::{noise::Tunn, x25519::StaticSecret};

use crate::Result;
use crate::config::{Config, PresharedKey, PrivateKey, PublicKey};

/// Represents a single-peer tunnel connection.
pub struct Tunnel {
    tunnel: Tunn,
}

impl Tunnel {
    pub fn init(config: &Config) -> Result<Self> {
        let static_secret = try_get_static_secret(&config.interface.private_key)?;
        let public_key = try_get_public_key(&config.peer.public_key)?;
        let preshared_key = try_get_preshared_key(&config.peer.preshared_key)?;

        let peer_index = ps5::random::u64().map(|val| val as u32)?;

        let tunnel = Tunn::new(
            static_secret,
            public_key,
            Some(preshared_key),
            Some(30),
            peer_index,
            None,
        );

        Ok(Self { tunnel })
    }
}

fn try_get_static_secret(private_key: &PrivateKey) -> Result<StaticSecret> {
    // SAFETY: Key length validated during type construction.
    let arr_ref = private_key.0.as_array::<32>().unwrap();

    Ok((*arr_ref).into())
}

fn try_get_public_key(public_key: &PublicKey) -> Result<boringtun::x25519::PublicKey> {
    // SAFETY: Key length validated during type construction.
    let arr_ref = public_key.0.as_array::<32>().unwrap();

    Ok((*arr_ref).into())
}

fn try_get_preshared_key(preshared_key: &PresharedKey) -> Result<[u8; 32]> {
    // SAFETY: Key length validated during type construction.
    let arr_ref = preshared_key.0.as_array::<32>().unwrap();

    Ok(*arr_ref)
}
