use std::{
    fs::File,
    io::Write,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    str::FromStr,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, de};
use tracing::{Level, debug, instrument, warn};

use crate::{
    ErrorKind, Result,
    result::{Error, WrapWithMsg},
};

pub const ROOT_DIR_STR: &str = "/data/wireguard-client";
pub const CFG_FILENAME: &str = "wg0.toml";

/// Load the config file, returning an error if the config file/data dir
/// are not yet created.
pub fn get_config() -> Result<WgQuickConfig> {
    if !Path::new(ROOT_DIR_STR).exists() {
        warn!("Data dir not yet initialised.");

        setup_empty_dir().context("Setting up root data dir")?;

        return Err(Error::new(crate::ErrorKind::PendingConfig));
    }

    debug!("Reading config file.");

    let cfg_file_path = PathBuf::from_iter([ROOT_DIR_STR, CFG_FILENAME]);
    let config = load_config_file(&cfg_file_path).map_err(|e| {
        if let ErrorKind::TomlDeserialization(de_err) = e.kind() {
            warn!(
                "Config file parse error. Error: {}",
                de_err.message().trim()
            );
        }

        Error::new(ErrorKind::ConfigLoadError(CFG_FILENAME))
    })?;

    Ok(config)
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

#[instrument()]
fn load_config_file(path: &Path) -> Result<WgQuickConfig> {
    let contents = std::fs::read_to_string(path).context("Opening config file")?;

    toml::from_str(&contents)
        .with_context(|| format!("Deserialising config file '{}'", path.display()))
}

#[derive(Deserialize)]
pub struct WgQuickConfig {
    interface: Interface,
    peer: Peer,
}

#[derive(Deserialize)]
pub struct Interface {
    private_key: PrivateKey,
    address: IpCidr,
    dns: String,
}

#[derive(Deserialize)]
pub struct Peer {
    public_key: PublicKey,
    preshared_key: PresharedKey,
    endpoint: String,
    allowed_ips: Vec<IpCidr>,
}

fn deserialize_base64_str<'de, D>(deserializer: D) -> std::result::Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let str_value = <&str>::deserialize(deserializer)?;

    STANDARD.decode(str_value).map_err(de::Error::custom)
}

pub struct IpCidr {
    ip_addr: IpAddr,
    subnet: u8,
}

pub struct PrivateKey(Vec<u8>);
pub struct PresharedKey(Vec<u8>);
pub struct PublicKey(Vec<u8>);

macro_rules! impl_base64_newtype {
    ($newtype:ident) => {
        impl<'de> Deserialize<'de> for $newtype {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let val = deserialize_base64_str(deserializer)?;
                Ok(Self(val))
            }
        }
    };
}

impl_base64_newtype!(PrivateKey);
impl_base64_newtype!(PresharedKey);
impl_base64_newtype!(PublicKey);

impl<'de> Deserialize<'de> for IpCidr {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let str_value = <&str>::deserialize(deserializer)?;
        let (ip_str, subnet_str) = str_value.split_once('/').ok_or(de::Error::invalid_value(
            de::Unexpected::Other("end of string"),
            &"A forward slash with the subnet",
        ))?;

        let ip_addr = IpAddr::from_str(ip_str).map_err(de::Error::custom)?;
        let subnet: u8 = subnet_str.parse().map_err(de::Error::custom)?;

        if subnet > 32 {
            return Err(de::Error::invalid_value(
                de::Unexpected::Unsigned(subnet as u64),
                &"unsigned integer <= 32",
            ));
        }

        Ok(Self { ip_addr, subnet })
    }
}
