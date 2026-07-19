use std::{fmt, process::ExitCode};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("usage: {0}")]
    Usage(String),
    #[error("configuration: {0}")]
    Config(String),
    #[error("authentication failed")]
    Authentication,
    #[error("remote request failed: {0}")]
    Remote(RemoteError),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("interrupted")]
    Interrupted,
}

impl CliError {
    pub fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Usage(_) | Self::Config(_) => ExitCode::from(2),
            Self::Authentication => ExitCode::from(3),
            Self::Remote(_) => ExitCode::from(4),
            Self::Protocol(_) => ExitCode::from(5),
            Self::Io(_) => ExitCode::from(6),
            Self::Interrupted => ExitCode::from(130),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteError {
    pub status: Option<u16>,
    pub code: Option<String>,
    pub message: String,
}

impl fmt::Display for RemoteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(status) = self.status {
            write!(formatter, "HTTP {status}")?;
        } else {
            formatter.write_str("transport")?;
        }
        if let Some(code) = &self.code {
            write!(formatter, " ({code})")?;
        }
        write!(formatter, ": {}", self.message)
    }
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}
