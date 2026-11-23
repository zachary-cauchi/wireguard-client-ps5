use std::{
    fmt::Display,
    fs::File,
    io::Write,
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    str::FromStr,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, de};
use tracing::{debug, instrument, level_filters::LevelFilter, warn};

use crate::{
    ErrorKind, Result,
    result::{Error, WrapWithMsg},
};

/// Defines a port forwarding rule from the given source socket
/// to the destination socket, using the specified transport protocol.
#[derive(Debug, Clone, Deserialize)]
pub struct PortForwardRule {
    pub source_addr: SocketAddr,
    pub dest_addr: SocketAddr,
    pub protocol: Protocol,
}

impl Display for PortForwardRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}:{}",
            self.source_addr, self.dest_addr, self.protocol
        )
    }
}

impl PortForwardRule {
    pub fn try_new(
        source_addr: impl ToSocketAddrs,
        dest_addr: impl ToSocketAddrs,
        protocol: Protocol,
    ) -> Result<Self> {
        let source_addr =
            source_addr
                .to_socket_addrs()?
                .next()
                .ok_or(Error::new(ErrorKind::BadValue(
                    "No source address returned",
                )))?;

        let dest_addr =
            dest_addr
                .to_socket_addrs()?
                .next()
                .ok_or(Error::new(ErrorKind::BadValue(
                    "No destination address returned",
                )))?;

        Ok(Self {
            source_addr,
            dest_addr,
            protocol,
        })
    }
}

/// Packet/transport protocol type.
/// TODO Find better place for this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        };

        f.write_str(msg)
    }
}

/// TODO Find better place for this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VirtualPort(pub Protocol, pub u16);

impl Display for VirtualPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.0, self.1)
    }
}

pub const ROOT_DIR_STR: &str = "/data/wireguard-client";
pub const CFG_FILENAME: &str = "wg0.toml";

/// Load the config file, returning an error if the config file/data dir
/// are not yet created.
pub fn get_config() -> Result<Config> {
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
fn load_config_file(path: &Path) -> Result<Config> {
    let contents = std::fs::read_to_string(path).context("Opening config file")?;

    toml::from_str(&contents)
        .with_context(|| format!("Deserialising config file '{}'", path.display()))
}

#[derive(Deserialize)]
pub struct Config {
    pub general: General,
    pub interface: Interface,
    pub peer: Peer,
    pub port_forward_rule: PortForwardRule,
}

#[derive(Deserialize)]
pub struct General {
    #[serde(deserialize_with = "deserialize_levelfilter")]
    pub klog_level: LevelFilter,
    #[serde(deserialize_with = "deserialize_levelfilter")]
    pub std_level: LevelFilter,
}

#[derive(Deserialize)]
pub struct Interface {
    pub private_key: PrivateKey,
    pub address: IpCidr,
    pub dns: String,
}

#[derive(Deserialize)]
pub struct Peer {
    pub public_key: PublicKey,
    pub preshared_key: PresharedKey,
    pub endpoint: String,
    pub allowed_ips: Vec<IpCidr>,
}

fn deserialize_levelfilter<'de, D>(deserializer: D) -> std::result::Result<LevelFilter, D::Error>
where
    D: Deserializer<'de>,
{
    let str = <&str>::deserialize(deserializer)?;

    LevelFilter::from_str(str).map_err(de::Error::custom)
}

fn deserialize_base64_str<'de, D>(deserializer: D) -> std::result::Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let str_value = <&str>::deserialize(deserializer)?;

    STANDARD.decode(str_value).map_err(de::Error::custom)
}

pub struct IpCidr {
    pub ip_addr: IpAddr,
    pub subnet: u8,
}

pub struct PrivateKey(pub Vec<u8>);
pub struct PresharedKey(pub Vec<u8>);
pub struct PublicKey(pub Vec<u8>);

macro_rules! impl_base64_key_newtype {
    ($newtype:ident, $len:literal) => {
        impl<'de> Deserialize<'de> for $newtype {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let val = deserialize_base64_str(deserializer)?;

                if val.len() != $len {
                    return Err(de::Error::invalid_length(
                        val.len(),
                        &stringify!(key $len bytes long),
                    ));
                }

                Ok(Self(val))
            }
        }
    };
}

impl_base64_key_newtype!(PrivateKey, 32);
impl_base64_key_newtype!(PresharedKey, 32);
impl_base64_key_newtype!(PublicKey, 32);

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
