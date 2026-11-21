pub mod account;
pub mod audio;
pub mod transport;

use std::{fmt, io};

pub use reqwest::Url;
pub use tokio_util::sync::CancellationToken;

pub const APP_NAME: &str = "Tagcast Lite";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Failure {
    Cancelled,
    Network,
    Security,
    Auth,
    Http(u16),
    Unsupported,
    Invalid(&'static str),
    SeekUnavailable,
}

impl Failure {
    pub fn temporary(&self) -> bool {
        matches!(
            self,
            Self::Network | Self::Http(408 | 500 | 502 | 503 | 504)
        )
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Playback cancelled"),
            Self::Network => f.write_str("The iBroadcast connection was interrupted"),
            Self::Security => f.write_str("Could not verify the secure iBroadcast connection"),
            Self::Auth => f.write_str("iBroadcast refused the session. Sign in again"),
            Self::Http(status) => write!(f, "iBroadcast returned HTTP {status}"),
            Self::Unsupported => f.write_str("This audio format or stream is unsupported"),
            Self::Invalid(message) => f.write_str(message),
            Self::SeekUnavailable => f.write_str("This position is not yet seekable"),
        }
    }
}

impl std::error::Error for Failure {}

impl From<Failure> for io::Error {
    fn from(failure: Failure) -> Self {
        let kind = match failure {
            Failure::SeekUnavailable => io::ErrorKind::Unsupported,
            Failure::Cancelled => io::ErrorKind::BrokenPipe,
            _ => io::ErrorKind::Other,
        };
        Self::new(kind, failure)
    }
}

pub type Result<T> = std::result::Result<T, Failure>;
