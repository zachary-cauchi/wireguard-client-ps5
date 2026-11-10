use std::{
    fs::File,
    io::Write,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

use tracing::{debug, instrument, warn};

use crate::{
    Result,
    result::{Error, WrapWithMsg},
};

pub const ROOT_DIR_STR: &str = "/data/wireguard-client";
pub const CFG_FILENAME: &str = "wg0.conf";

/// Load the config file, returning an error if the config file/data dir
/// are not yet created.
pub fn get_config() -> Result<()> {
    if !Path::new(ROOT_DIR_STR).exists() {
        warn!("Data dir not yet initialised.");

        setup_empty_dir().context("Setting up root data dir")?;

        return Err(Error::new(crate::ErrorKind::PendingConfig));
    }

    debug!("Reading config file.");

    Ok(())
}

// Initialise an empty data directory with an empty config file.
#[instrument()]
pub fn setup_empty_dir() -> Result<()> {
    debug!("Initialising data dir '{ROOT_DIR_STR}'.");

    std::fs::create_dir(ROOT_DIR_STR).context("Creating root data dir")?;

    let cfg_file_path = PathBuf::from_iter([ROOT_DIR_STR, CFG_FILENAME]);

    let mut cfg_file = File::create(cfg_file_path).context("Creating empty cfg file")?;

    cfg_file.write_all("# Enter a wg-quick config here.".as_bytes())?;

    Ok(())
}

pub struct WgQuickConfig {
    interface: Interface,
    peer: Peer,
}

pub struct Interface {
    private_key: Vec<u8>,
    address: IpV4Cidr,
    dns: Ipv4Addr,
}

pub struct Peer {
    public_key: Vec<u8>,
    preshared_key: Vec<u8>,
    endpoint: (String, u16),
    allowed_ips: Vec<IpV4Cidr>,
}

struct IpV4Cidr {
    inner: [u8; 5],
}
