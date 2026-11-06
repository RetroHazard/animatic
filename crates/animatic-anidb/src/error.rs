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
    #[error(
        "client version outdated - update Animatic, or report this if it is already the latest version"
    )]
    ClientOutdated,
    /// Reply 504: AniDB refuses this client name and version for good. Like
    /// [`AnidbError::ClientOutdated`] it needs a correction, never a retry.
    #[error(
        "AniDB refuses Animatic's built-in client registration (reply 504): update Animatic, or report this if it is already the latest version"
    )]
    ClientBanned,
    /// An `AUTH` (or the `ENCRYPT` before it) got no reply. The duration is the escalating,
    /// persisted backoff that now applies (see [`crate::limits::AUTH_BACKOFF`]).
    #[error("no reply to the AniDB login (AUTH); the next try is allowed in {}", wait_text(*.0))]
    AuthTimeout(Duration),
    /// The request state that must be saved before a request is sent could not be saved, so
    /// nothing was sent.
    #[error("cannot save the AniDB request state, nothing was sent: {0}")]
    Storage(String),
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

/// Cooldown applied after a ban when AniDB does not tell how long it lasts. AniDB bans last
/// "usually about 30 min"; see the UDP API rules in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits).
pub const BAN_COOLDOWN: Duration = Duration::from_secs(30 * 60);
/// Cooldown applied after a transient network failure. A design choice: AniDB publishes no
/// figure (see
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits)).
pub const TRANSIENT_COOLDOWN: Duration = Duration::from_secs(60);

/// "30 s", "2 min", "2 h": a wait in the unit people read it in.
fn wait_text(wait: Duration) -> String {
    let secs = wait.as_secs().max(1);
    if secs < 120 {
        format!("{secs} s")
    } else if secs < 2 * 3600 {
        format!("{} min", secs.div_ceil(60))
    } else {
        format!("{} h", secs.div_ceil(3600))
    }
}

impl AnidbError {
    /// Errors after which retrying the same command later may succeed.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            AnidbError::Network(_)
                | AnidbError::Timeout
                | AnidbError::AuthTimeout(_)
                | AnidbError::Storage(_)
                | AnidbError::ServerError(..)
                | AnidbError::ServerUnavailable(_)
                | AnidbError::Http(_)
        )
    }

    /// AniDB refuses this client: banned, outdated or rejected credentials. The whole queue stops.
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            AnidbError::Banned(_)
                | AnidbError::ClientOutdated
                | AnidbError::ClientBanned
                | AnidbError::Auth(_)
        )
    }

    /// How long all network use must pause after this error. `None` means no automatic
    /// retry: the entry needs a correction (credentials, client version) or can never succeed.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            AnidbError::Banned(_) => Some(BAN_COOLDOWN),
            AnidbError::ServerUnavailable(d) | AnidbError::AuthTimeout(d) => Some(*d),
            AnidbError::ServerError(604, _) => Some(TRANSIENT_COOLDOWN),
            AnidbError::ServerError(..) => Some(BAN_COOLDOWN),
            AnidbError::Network(_)
            | AnidbError::Timeout
            | AnidbError::Storage(_)
            | AnidbError::Http(_) => Some(TRANSIENT_COOLDOWN),
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
        for e in [
            AnidbError::ClientOutdated,
            AnidbError::ClientBanned,
            AnidbError::Auth(String::new()),
        ] {
            assert!(e.is_fatal() && !e.is_transient() && e.retry_after().is_none());
        }
        assert!(AnidbError::ClientBanned.to_string().contains("refuses"));
        let unanswered = AnidbError::AuthTimeout(Duration::from_secs(300));
        assert!(unanswered.is_transient() && !unanswered.is_fatal());
        assert_eq!(unanswered.retry_after(), Some(Duration::from_secs(300)));
        let storage = AnidbError::Storage("synthetic".into());
        assert!(storage.is_transient() && !storage.is_fatal());
        assert_eq!(storage.retry_after(), Some(TRANSIENT_COOLDOWN));
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

    #[test]
    fn waits_are_shown_in_a_readable_unit() {
        for (secs, text) in [
            (30, "30 s"),
            (119, "119 s"),
            (120, "2 min"),
            (600, "10 min"),
            (1800, "30 min"),
            (7200, "2 h"),
        ] {
            let shown = AnidbError::AuthTimeout(Duration::from_secs(secs)).to_string();
            assert!(shown.ends_with(text), "{shown}");
        }
    }
}
