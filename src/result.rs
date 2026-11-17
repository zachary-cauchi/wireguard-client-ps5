use std::{
    borrow::Cow,
    fmt::{Debug, Display},
    io,
};

use boringtun::noise::errors::WireGuardError;
use smoltcp::socket::tcp::ConnectError;
use tokio::{
    sync::broadcast::error::{RecvError, SendError},
    task::JoinError,
};
use tracing::subscriber::SetGlobalDefaultError;

/// Root, program-wide error struct.
#[derive(Debug)]
pub struct Error {
    msg: Cow<'static, str>,
    kind: ErrorKind,
}

impl Error {
    #[inline]
    pub const fn new(kind: ErrorKind) -> Self {
        Self {
            msg: Cow::Borrowed(""),
            kind,
        }
    }

    pub fn with_msg(self, msg: impl Into<Cow<'static, str>>) -> Self {
        Self {
            msg: msg.into(),
            kind: self.kind,
        }
    }

    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "App error caused by '{}'. Context: \"{}\"",
            self.kind, self.msg
        )
    }
}

#[derive(Debug)]
pub enum ErrorKind {
    /// Errors setting up logging.
    GlobalLogging,
    /// No configuration found.
    PendingConfig,
    /// File IO-related errors.
    FileIO(io::Error),
    TomlDeserialization(Box<toml::de::Error>),
    /// Config file loading error.
    ConfigLoadError(&'static str),
    /// Bad key value/format.
    BadKey(&'static str),
    /// General PS5 library error.
    Ps5(ps5::Error),
    TaskJoining(Box<JoinError>),
    /// Wireguard-derived error.
    TunnelError(WireGuardError),
    /// Event bus is closed.
    BusClosed,
    /// Event bus receiver lagged behind.
    BusLagged(u64),
    PortPoolExhausted,
    BadValue(&'static str),
    TcpStackConnectError(ConnectError),
}

impl Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ErrorKind::*;

        match self {
            GlobalLogging => write!(f, "Global logging initialisation failed"),
            PendingConfig => write!(f, "No valid configuration found yet"),
            Ps5(e) => write!(f, "PS5 error '{}'", e.0),
            FileIO(e) => write!(f, "File I/O error ({e})"),
            BadKey(msg) => write!(f, "Bad key value or format ({msg})"),
            ConfigLoadError(_) => write!(f, "Error loading/parsing config"),
            TomlDeserialization(_) => write!(f, "TOML deserialization error"),
            TaskJoining(e) => write!(f, "Task operation failed. Error: {e}"),
            TunnelError(e) => write!(f, "Wireguard error ({e:?})"),
            BusClosed => write!(f, "Event bus is closed"),
            BusLagged(n) => write!(f, "Event bus receiver lagged by {n} messages"),
            PortPoolExhausted => write!(f, "Port pool exhausted"),
            BadValue(msg) => write!(f, "Bad value received ({msg})"),
            TcpStackConnectError(e) => write!(f, "TCP virtual stack connection error ({e})"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        use ErrorKind::*;

        match &self.kind {
            FileIO(e) => Some(e),
            TomlDeserialization(e) => Some(e),
            TaskJoining(e) => Some(e),
            _ => None,
        }
    }
}

impl From<SetGlobalDefaultError> for ErrorKind {
    fn from(_: SetGlobalDefaultError) -> Self {
        Self::GlobalLogging
    }
}

impl From<io::Error> for ErrorKind {
    fn from(value: io::Error) -> Self {
        Self::FileIO(value)
    }
}

impl From<ps5::Error> for ErrorKind {
    fn from(value: ps5::Error) -> Self {
        Self::Ps5(value)
    }
}

impl From<toml::de::Error> for ErrorKind {
    fn from(value: toml::de::Error) -> Self {
        Self::TomlDeserialization(Box::new(value))
    }
}

impl From<JoinError> for ErrorKind {
    fn from(value: JoinError) -> Self {
        Self::TaskJoining(Box::new(value))
    }
}

impl From<WireGuardError> for ErrorKind {
    fn from(value: WireGuardError) -> Self {
        Self::TunnelError(value)
    }
}

impl From<RecvError> for ErrorKind {
    fn from(value: RecvError) -> Self {
        match value {
            RecvError::Closed => Self::BusClosed,
            RecvError::Lagged(n) => Self::BusLagged(n),
        }
    }
}

impl<T> From<SendError<T>> for ErrorKind {
    fn from(_: SendError<T>) -> Self {
        Self::BusClosed
    }
}

impl From<ConnectError> for ErrorKind {
    fn from(value: ConnectError) -> Self {
        Self::TcpStackConnectError(value)
    }
}

impl<E> From<E> for Error
where
    E: Into<ErrorKind>,
{
    fn from(value: E) -> Self {
        Self::new(value.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Trait for adding a contextual message string in case of [Result::Err].
pub trait WrapWithMsg<T, Msg> {
    fn context(self, msg: Msg) -> Result<T>;
    fn with_context(self, msg_f: impl FnOnce() -> Msg) -> Result<T>;
}

impl<T, E, Msg> WrapWithMsg<T, Msg> for std::result::Result<T, E>
where
    E: Into<Error>,
    Msg: Into<Cow<'static, str>>,
{
    /// Add a context msg describing the result in case of failure.
    ///
    /// Example:
    /// ```rust
    /// std::fs::create_dir(ROOT_DIR_STR)
    ///     .context("Creating root data dir")?;
    /// ```
    fn context(self, msg: Msg) -> Result<T> {
        match self {
            Ok(val) => Ok(val),
            Err(e) => Err(e.into().with_msg(msg.into())),
        }
    }

    /// Add a context msg - supplied by the given closure -
    /// describing the result in case of failure.
    ///
    /// Example:
    /// ```rust
    /// std::fs::create_dir(ROOT_DIR_STR)
    ///     .with_context(|| "Creating root data dir")?;
    /// ```
    fn with_context(self, msg_f: impl FnOnce() -> Msg) -> Result<T> {
        match self {
            Ok(val) => Ok(val),
            Err(e) => Err(e.into().with_msg(msg_f().into())),
        }
    }
}
