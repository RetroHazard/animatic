use std::time::Duration;

#[derive(Debug, Clone, thiserror::Error)]
pub enum AnidbError {
    #[error("network error: {0}")]
    Network(String),
    #[error("no reply from AniDB (timeout)")]
    Timeout,
    #[error("authentication: {0}")]
    Auth(String),
    #[error("client banned by AniDB: {0}")]
    Banned(String),
    #[error("client version outdated - check the AniDB client registration")]
    ClientOutdated,
    #[error("AniDB server error {0}: {1}")]
    ServerError(u16, String),
    #[error("AniDB unavailable, retry in {} min", .0.as_secs().div_ceil(60))]
    ServerUnavailable(Duration),
    #[error("illegal input: {0}")]
    IllegalInput(String),
    #[error("unknown command")]
    UnknownCommand,
    #[error("unexpected reply {0}: {1}")]
    Unexpected(u16, String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("XML error: {0}")]
    Xml(String),
}

impl AnidbError {
    /// Errors after which retrying the same command later may succeed.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            AnidbError::Network(_)
                | AnidbError::Timeout
                | AnidbError::ServerError(..)
                | AnidbError::ServerUnavailable(_)
                | AnidbError::Http(_)
        )
    }
}

pub type Result<T, E = AnidbError> = std::result::Result<T, E>;
