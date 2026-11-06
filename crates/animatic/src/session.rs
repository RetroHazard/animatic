//! The AniDB session the worker keeps between queue runs (roadmap M5.6).
//!
//! AniDB asks clients not to log in and out around every command and expires a session after 35
//! idle minutes. While automatic queue runs are on ([`AnidbSettings::auto_run_queue`]) the worker
//! therefore keeps its one [`UdpClient`], and with it the login session and the bound local
//! port, from one run to the next. Without automatic runs nothing changes: every run logs in
//! when it needs to and logs out when it ends.
//!
//! A kept session is looked after in the gaps between jobs by [`SessionSlot::maintain`]: a `PING
//! nat=1` while a router rewrites the port, a keep-alive command at least every 30 minutes
//! while more commands are expected, a logout after the configured idle minutes otherwise (see
//! [`UdpClient::upkeep`] for the timing). It never sends anything while Work offline is on or
//! a cooldown is active, and every packet takes its slot from the shared limiter like any other.
//! The worker also logs out on its way out ([`SessionSlot::logout_on_exit`], bounded wait).

use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use animatic_anidb::limits::Limits;
use animatic_anidb::udp::{Config as UdpConfig, UdpClient, Upkeep, UpkeepPolicy};
use animatic_core::db::Db;
use animatic_core::settings::AnidbSettings;
use animatic_core::sync::QueueScope;
use crossbeam_channel::Sender;

use crate::worker::{Event, start_cooldown};

/// How long the worker waits before it looks again at a session it may not touch right now
/// (Work offline, an active cooldown, a database that cannot be read).
const BLOCKED_POLL: Duration = Duration::from_secs(30);

/// Never wake the worker more often than this for the session: a step that is due is done at
/// once, so a smaller wait would only come from a step that could not be done.
const MIN_WAKE: Duration = Duration::from_secs(1);

/// How long the logout on exit waits for AniDB's answer. The packet may first wait for its
/// send slot (up to 4 s), so the whole exit takes a little longer; see [`EXIT_LOGOUT_WAIT`].
const EXIT_LOGOUT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the application waits at exit for the worker's logout before it lets the process
/// end anyway. Best effort: AniDB drops an unused session after 35 minutes regardless.
pub(crate) const EXIT_LOGOUT_WAIT: Duration = Duration::from_secs(8);

/// The UDP client the worker keeps between runs, if any, and a flag the application reads to
/// know whether a logout is worth waiting for at exit. Only the worker thread uses it.
pub(crate) struct SessionSlot {
    client: RefCell<Option<UdpClient>>,
    /// True while a run is in progress or a session is kept.
    holds_session: Arc<AtomicBool>,
}

impl SessionSlot {
    pub(crate) fn new(holds_session: Arc<AtomicBool>) -> SessionSlot {
        SessionSlot {
            client: RefCell::new(None),
            holds_session,
        }
    }

    /// Whether a client is being kept (not counting one that a run has taken).
    #[cfg(test)]
    pub(crate) fn is_kept(&self) -> bool {
        self.client.borrow().is_some()
    }

    /// The client for a queue run: the kept one, told about the current settings (a changed
    /// account or server drops its session; see [`UdpClient::set_config`]), or
    /// a new one.
    pub(crate) fn take(&self, config: UdpConfig, limits: &Arc<Limits>) -> UdpClient {
        // A run is about to start and may log in: an exit during it is worth waiting for.
        self.holds_session.store(true, Ordering::SeqCst);
        match self.client.borrow_mut().take() {
            Some(mut client) => {
                client.set_config(config);
                client
            }
            None => UdpClient::new(config, limits.clone()),
        }
    }

    /// Give the client back after a run. With `keep` the session stays for the next run;
    /// otherwise, or when there is no session left to keep (never logged in, banned), the
    /// client logs out and is dropped.
    pub(crate) fn finish(&self, mut client: UdpClient, keep: bool) {
        if keep && client.is_logged_in() && client.banned_reason().is_none() {
            *self.client.borrow_mut() = Some(client);
        } else {
            let _ = client.logout();
            self.holds_session.store(false, Ordering::SeqCst);
        }
    }

    /// Do what the kept session needs now and say how long until it next needs anything
    /// (`None`: nothing is kept). Called by the worker whenever it is about to wait for a job,
    /// and again when the wait ran out.
    ///
    /// The settings are the current ones (not those of the run that made the session): with
    /// automatic runs switched off the session is logged out here. Nothing is ever sent while
    /// Work offline is on or a cooldown is active; the session then simply ages, and is
    /// forgotten without a packet once AniDB has expired it. Any error forgets the session (the
    /// next run logs in again, under the saved backoff) and starts the cooldown it calls for.
    pub(crate) fn maintain(
        &self,
        db_path: &Path,
        settings: &AnidbSettings,
        events: &Sender<Event>,
    ) -> Option<Duration> {
        let mut slot = self.client.borrow_mut();
        let client = slot.as_mut()?;
        if settings.offline {
            return Some(BLOCKED_POLL);
        }
        let db = match Db::open(db_path) {
            Ok(db) => db,
            Err(e) => {
                log::warn!("cannot look after the AniDB session: {e}");
                return Some(BLOCKED_POLL);
            }
        };
        // Fails closed: a cooldown that cannot be read is treated like an active one.
        if !matches!(db.sync_cooldown(), Ok(None)) {
            return Some(BLOCKED_POLL);
        }
        let policy = policy_for(&db, settings);
        match client.upkeep(&policy) {
            Ok(step) => {
                if let Some(note) = describe(step, settings.auto_run_queue) {
                    let _ = events.send(Event::Info(note.into()));
                }
            }
            Err(e) => {
                let _ = events.send(Event::Error(format!("AniDB session upkeep failed: {e}")));
                if let Some(wait) = e.retry_after()
                    && let Err(db_error) = start_cooldown(&db, wait, &e.to_string(), events)
                {
                    let _ = events.send(Event::Error(format!(
                        "Could not record the AniDB cooldown: {db_error}"
                    )));
                }
                *slot = None;
                self.holds_session.store(false, Ordering::SeqCst);
                return None;
            }
        }
        if let Some(reason) = client.banned_reason().map(str::to_string) {
            // A ban reply that arrived in answer to a logout: the same cooldown as anywhere.
            let _ = start_cooldown(&db, animatic_anidb::error::BAN_COOLDOWN, &reason, events);
        }
        match client.next_upkeep(&policy) {
            Some(due) if client.banned_reason().is_none() => Some(due.max(MIN_WAKE)),
            _ => {
                *slot = None;
                self.holds_session.store(false, Ordering::SeqCst);
                None
            }
        }
    }

    /// Log out of a kept session before the worker ends: one `LOGOUT`, answered or not within
    /// [`EXIT_LOGOUT_TIMEOUT`]. Sends nothing while Work offline is on or a cooldown is active
    /// (AniDB drops the session by itself).
    pub(crate) fn logout_on_exit(&self, db_path: &Path, settings: &AnidbSettings) {
        let Some(mut client) = self.client.borrow_mut().take() else {
            return;
        };
        self.holds_session.store(false, Ordering::SeqCst);
        if settings.offline {
            return;
        }
        let quiet = Db::open(db_path)
            .and_then(|db| db.sync_cooldown())
            .map_or(true, |cooldown| cooldown.is_some());
        if quiet {
            return;
        }
        let mut config = client.config().clone();
        config.timeout = EXIT_LOGOUT_TIMEOUT;
        client.set_config(config);
        let _ = client.logout();
    }
}

/// What the kept session needs, from the settings and the queue.
fn policy_for(db: &Db, settings: &AnidbSettings) -> UpkeepPolicy {
    if !settings.auto_run_queue {
        // Sessions are only kept for automatic runs: this one ends now.
        return UpkeepPolicy {
            work_expected: false,
            idle_logout: Duration::ZERO,
        };
    }
    let work_expected = match db.queue_pending(QueueScope::Automatic) {
        Ok(pending) => pending.count > 0,
        Err(e) => {
            log::warn!("cannot read the command queue: {e}");
            false
        }
    };
    UpkeepPolicy {
        work_expected,
        idle_logout: settings.idle_logout(),
    }
}

/// The activity-log line for a step worth showing. Pings and keep-alive commands are routine.
fn describe(step: Upkeep, automatic_runs: bool) -> Option<&'static str> {
    match step {
        Upkeep::LoggedOut if automatic_runs => Some("Logged out of AniDB: the session was idle"),
        Upkeep::LoggedOut => Some("Logged out of AniDB: automatic queue runs are off"),
        Upkeep::Expired => Some("The AniDB session ran out while idle; the next run logs in again"),
        Upkeep::Nothing | Upkeep::NatPing | Upkeep::KeepAlive => None,
    }
}
