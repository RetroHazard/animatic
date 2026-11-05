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

/// Cooldown applied after a ban when AniDB does not tell how long it lasts.
pub const BAN_COOLDOWN: Duration = Duration::from_secs(30 * 60);
/// Cooldown applied after a transient network failure.
pub const TRANSIENT_COOLDOWN: Duration = Duration::from_secs(60);

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

    /// AniDB refuses this client: banned, outdated or rejected credentials. The whole queue stops.
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            AnidbError::Banned(_) | AnidbError::ClientOutdated | AnidbError::Auth(_)
        )
    }

    /// How long all network use must pause after this error. `None` means no automatic
    /// retry: the entry needs a correction (credentials, client version) or can never succeed.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            AnidbError::Banned(_) => Some(BAN_COOLDOWN),
            AnidbError::ServerUnavailable(d) => Some(*d),
            AnidbError::ServerError(604, _) => Some(TRANSIENT_COOLDOWN),
            AnidbError::ServerError(..) => Some(BAN_COOLDOWN),
            AnidbError::Network(_) | AnidbError::Timeout | AnidbError::Http(_) => {
                Some(TRANSIENT_COOLDOWN)
            }
            _ => None,
        }
    }
}

pub type Result<T, E = AnidbError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_failures_for_cooldown_and_blocking() {
        let banned = AnidbError::Banned("x".into());
        assert!(banned.is_fatal());
        assert_eq!(banned.retry_after(), Some(BAN_COOLDOWN));

        let wait = Duration::from_secs(123);
        assert_eq!(
            AnidbError::ServerUnavailable(wait).retry_after(),
            Some(wait)
        );
        assert_eq!(
            AnidbError::ServerError(604, String::new()).retry_after(),
            Some(TRANSIENT_COOLDOWN)
        );
        assert_eq!(
            AnidbError::ServerError(602, String::new()).retry_after(),
            Some(BAN_COOLDOWN)
        );
        for e in [
            AnidbError::Timeout,
            AnidbError::Network(String::new()),
            AnidbError::Http(String::new()),
        ] {
            assert!(e.is_transient() && !e.is_fatal());
            assert_eq!(e.retry_after(), Some(TRANSIENT_COOLDOWN));
        }
        for e in [AnidbError::ClientOutdated, AnidbError::Auth(String::new())] {
            assert!(e.is_fatal() && e.retry_after().is_none());
        }
        for e in [
            AnidbError::IllegalInput(String::new()),
            AnidbError::UnknownCommand,
            AnidbError::Unexpected(999, String::new()),
            AnidbError::Protocol(String::new()),
            AnidbError::Xml(String::new()),
        ] {
            assert!(!e.is_transient() && !e.is_fatal() && e.retry_after().is_none());
        }
    }
}
