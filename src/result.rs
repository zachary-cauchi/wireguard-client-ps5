use std::fmt::Display;

use tracing::subscriber::SetGlobalDefaultError;

#[derive(Debug)]
pub enum Error {
    GlobalLogging,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GlobalLogging => write!(f, "Global logging initialisation failed"),
        }
    }
}

impl std::error::Error for Error {}

impl From<SetGlobalDefaultError> for Error {
    fn from(_: SetGlobalDefaultError) -> Self {
        Self::GlobalLogging
    }
}

pub type Result<T> = std::result::Result<T, Error>;
