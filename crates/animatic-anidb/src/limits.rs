//! Request spacing and the state that must survive a restart, shared by every AniDB client.
//!
//! AniDB bans clients that send faster than it allows, and a ban lasts about half an hour. One
//! [`Limits`] value is created per process (by the application's worker) and handed to every
//! [`UdpClient`](crate::udp::UdpClient) and [`HttpClient`](crate::http::HttpClient). A new client
//! for every queue run therefore cannot restart the burst allowance, and the last send time of
//! each protocol is saved through a [`LimitStore`] *before* the request leaves, so a restart or
//! a crash right after sending cannot reset the spacing either.
//!
//! The rules and their sources are recorded in
//! [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits).
//! Request guards that need the database (title dump once a day, minimum age per anime and per
//! file) live in `animatic-core`, which owns the storage; this crate has none.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

use crate::error::{AnidbError, Result};

/// Packets at the start of a UDP burst that are only held to [`UDP_BURST_SPACING`]. Rule: UDP
/// API, "Rate" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits)
/// (a packet every 2 s, "enforced after the first 5").
pub const UDP_BURST_PACKETS: u32 = 5;

/// Least time between two UDP packets while the burst allowance lasts. Rule: UDP API, "Rate" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits).
pub const UDP_BURST_SPACING: Duration = Duration::from_secs(2);

/// Least time between two UDP packets once the burst allowance is used up ("one per 4 s over an
/// extended period"). Rule: UDP API, "Rate" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits).
pub const UDP_SUSTAINED_SPACING: Duration = Duration::from_secs(4);

/// Idle time after which the UDP burst allowance starts over. A design choice: AniDB says only
/// "over an extended period" (see "Rate" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits)).
pub const UDP_BURST_RESET: Duration = Duration::from_secs(10 * 60);

/// Least time between two HTTP requests of any kind (anime records, the title dump, images).
/// Rule: HTTP API, "at most one request per 2 s" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits).
/// Images have no published rate; they use this one by design choice.
pub const HTTP_SPACING: Duration = Duration::from_secs(2);

/// How long to wait before the next login after the 1st, 2nd, ... consecutive `AUTH` without a
/// reply; the last value repeats. Rule: UDP API, "`AUTH` without reply" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits)
/// (AniDB allows "up to 2 to 4 h"; the shorter end is the design choice).
pub const AUTH_BACKOFF: [Duration; 6] = [
    Duration::from_secs(30),
    Duration::from_secs(2 * 60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(10 * 60),
    Duration::from_secs(30 * 60),
    Duration::from_secs(2 * 3600),
];

/// AniDB drops a login session after this long without a command. Rule: UDP API, "Session" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits)
/// (35 minutes idle).
pub const UDP_SESSION_TIMEOUT: Duration = Duration::from_secs(35 * 60);

/// Longest a session that is being kept goes without a command: AniDB asks for one "at least
/// every 30 min". Rule: UDP API, "Session" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits).
pub const UDP_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Longest an idle session may be held before the client logs out. It keeps every setting
/// below [`UDP_KEEP_ALIVE_INTERVAL`], so a session never reaches [`UDP_SESSION_TIMEOUT`]
/// unnoticed. Rule: UDP API, "Session" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits)
/// ("`LOGOUT` when idling longer").
pub const UDP_MAX_IDLE_LOGOUT: Duration = Duration::from_secs(30 * 60);

/// Longest silence on a NAT-mapped session before a `PING nat=1` keeps the mapping, and with
/// it the port AniDB bound the session to. Rule: UDP API, "NAT" in
/// [AniDB server limits](https://github.com/RetroHazard/animatic/blob/main/docs/storage-and-network.md#anidb-server-limits)
/// (router timeouts of 5 to 15 minutes, `PING` "no more often than needed"). The 4 minutes are
/// a design choice: just under the shortest timeout.
pub const UDP_NAT_PING_INTERVAL: Duration = Duration::from_secs(4 * 60);

/// Wall-clock time and sleeping, replaceable so tests need not wait.
pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;
    /// Block for at least `duration`.
    fn sleep(&self, duration: Duration);
}

/// The real clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// The two AniDB transports that have their own spacing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// The UDP API (`api.anidb.net:9000`).
    Udp,
    /// The HTTP API, the title dump and image downloads.
    Http,
}

/// Where the request state is kept between runs and restarts (a table of the application
/// database in `animatic-core`). Errors are text because this crate has no database.
///
/// Every method must be durable when it returns: a write that is lost in a crash would let the
/// next start send too early.
pub trait LimitStore: Send + Sync {
    /// Epoch milliseconds of the last request sent over `protocol`, if one is known.
    fn last_send(&self, protocol: Protocol) -> std::result::Result<Option<i64>, String>;
    /// Save the time at which a request over `protocol` is about to be sent.
    fn record_send(&self, protocol: Protocol, at_ms: i64) -> std::result::Result<(), String>;
    /// Consecutive `AUTH` attempts so far that got no reply (0 after a successful login).
    fn auth_unanswered(&self) -> std::result::Result<u32, String>;
    /// Save the number of consecutive `AUTH` attempts without a reply.
    fn set_auth_unanswered(&self, count: u32) -> std::result::Result<(), String>;
}

#[derive(Clone, Copy)]
struct State {
    last_ms: Option<i64>,
    /// UDP only: packets of the current burst.
    burst: u32,
}

/// Process-wide spacing of AniDB requests plus the persisted `AUTH` backoff. Create one and
/// share it through an [`Arc`].
pub struct Limits {
    clock: Arc<dyn Clock>,
    store: Arc<dyn LimitStore>,
    /// `None` until the saved state was read (on first use, so creating `Limits` never fails).
    udp: Mutex<Option<State>>,
    http: Mutex<Option<State>>,
}

fn storage(e: String) -> AnidbError {
    AnidbError::Storage(e)
}

/// Least gap before the next request, given how many packets of the burst went out.
fn gap(protocol: Protocol, burst: u32) -> Duration {
    match protocol {
        Protocol::Udp if burst < UDP_BURST_PACKETS => UDP_BURST_SPACING,
        Protocol::Udp => UDP_SUSTAINED_SPACING,
        Protocol::Http => HTTP_SPACING,
    }
}

impl Limits {
    pub fn new(clock: Arc<dyn Clock>, store: Arc<dyn LimitStore>) -> Arc<Limits> {
        Arc::new(Limits {
            clock,
            store,
            udp: Mutex::new(None),
            http: Mutex::new(None),
        })
    }

    /// Milliseconds since the Unix epoch on the clock that spaces the packets. Everything that
    /// asks "how long ago was the last packet" uses this clock, not its own, so tests that
    /// drive the spacing with a manual clock drive session timing too.
    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    /// Wait until the next UDP packet may be sent, and save its send time. Call it right before
    /// sending; the packet must be sent when this returns `Ok`.
    pub fn wait_udp(&self) -> Result<()> {
        self.wait(Protocol::Udp, &self.udp)
    }

    /// Wait until the next HTTP request may be sent, and save its send time. Call it right
    /// before every request to AniDB.
    pub fn wait_http(&self) -> Result<()> {
        self.wait(Protocol::Http, &self.http)
    }

    fn wait(&self, protocol: Protocol, slot: &Mutex<Option<State>>) -> Result<()> {
        // Held while sleeping: concurrent callers queue up behind the one that waits.
        let mut slot = slot.lock();
        let mut state = match *slot {
            Some(state) => state,
            None => {
                let last_ms = self.store.last_send(protocol).map_err(storage)?;
                State {
                    last_ms,
                    // The burst size is not saved. After a restart assume it was used up, so the
                    // next packet keeps the long-term spacing from the saved send time.
                    burst: if last_ms.is_some() && protocol == Protocol::Udp {
                        UDP_BURST_PACKETS
                    } else {
                        0
                    },
                }
            }
        };
        if let Some(last) = state.last_ms {
            // A clock that went backwards counts as no idle time: the wait stays bounded by
            // one gap.
            let idle = Duration::from_millis(
                u64::try_from(self.clock.now_ms().saturating_sub(last)).unwrap_or(0),
            );
            if protocol == Protocol::Udp && idle > UDP_BURST_RESET {
                state.burst = 0;
            }
            let gap = gap(protocol, state.burst);
            if idle < gap {
                self.clock.sleep(gap - idle);
            }
        }
        let at = self.clock.now_ms().max(state.last_ms.unwrap_or(i64::MIN));
        // Before the request leaves: a crash right after sending must still count it.
        self.store.record_send(protocol, at).map_err(storage)?;
        state.last_ms = Some(at);
        state.burst = state.burst.saturating_add(1);
        *slot = Some(state);
        Ok(())
    }

    /// An `AUTH` got no reply: count it and return how long login must now wait
    /// ([`AUTH_BACKOFF`]). The count is saved, so a restart continues the escalation.
    pub fn auth_unanswered(&self) -> Result<Duration> {
        let count = self
            .store
            .auth_unanswered()
            .map_err(storage)?
            .saturating_add(1)
            .min(AUTH_BACKOFF.len() as u32);
        self.store.set_auth_unanswered(count).map_err(storage)?;
        Ok(AUTH_BACKOFF[count as usize - 1])
    }

    /// A login succeeded: the escalation starts over.
    pub fn auth_answered(&self) -> Result<()> {
        if self.store.auth_unanswered().map_err(storage)? > 0 {
            self.store.set_auth_unanswered(0).map_err(storage)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ManualClock, test_limits};

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// Send times (ms) of `count` consecutive UDP packets.
    fn udp_sends(limits: &Limits, clock: &ManualClock, count: usize) -> Vec<i64> {
        (0..count)
            .map(|_| {
                limits.wait_udp().unwrap();
                clock.now_ms()
            })
            .collect()
    }

    #[test]
    fn udp_packets_keep_the_burst_then_the_long_term_spacing() {
        let t = test_limits();
        let sends = udp_sends(&t.limits, &t.clock, 8);
        let gaps: Vec<i64> = sends.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(gaps, [2000, 2000, 2000, 2000, 4000, 4000, 4000]);
        // The first packet of an idle client is not delayed.
        assert_eq!(t.clock.sleeps().len(), 7);
        // Every send time was saved before the packet could leave.
        assert_eq!(t.store.sends(Protocol::Udp), sends);
    }

    #[test]
    fn idle_time_restarts_the_burst_allowance() {
        let t = test_limits();
        udp_sends(&t.limits, &t.clock, 6);
        t.clock.advance(UDP_BURST_RESET + secs(1));
        let sends = udp_sends(&t.limits, &t.clock, 3);
        assert_eq!(
            t.clock.sleeps().len(),
            5 + 2,
            "no wait after a long idle time"
        );
        assert_eq!(sends[1] - sends[0], 2000, "the burst allowance is back");
    }

    #[test]
    fn a_restart_keeps_the_long_term_spacing_from_the_saved_send_time() {
        // The previous process sent a packet 1 s ago; its burst count is unknown.
        let t = test_limits();
        let saved = t.clock.now_ms() - 1000;
        t.store.set_last_send(Protocol::Udp, saved);
        t.limits.wait_udp().unwrap();
        assert_eq!(
            t.clock.sleeps(),
            [secs(3)],
            "4 s after the saved time, not 2 s"
        );

        // Long ago: nothing to wait for, and the burst allowance is back.
        let t = test_limits();
        t.store.set_last_send(
            Protocol::Udp,
            t.clock.now_ms() - UDP_BURST_RESET.as_millis() as i64 - 1,
        );
        udp_sends(&t.limits, &t.clock, 2);
        assert_eq!(t.clock.sleeps(), [secs(2)]);
    }

    #[test]
    fn a_new_limits_value_over_the_same_store_is_a_restart() {
        let first = test_limits();
        udp_sends(&first.limits, &first.clock, 3);
        let restarted = Limits::new(first.clock.clone(), first.store.clone());
        let before = first.clock.now_ms();
        restarted.wait_udp().unwrap();
        assert_eq!(first.clock.now_ms() - before, 4000);
    }

    #[test]
    fn http_requests_are_two_seconds_apart_and_survive_a_restart() {
        let t = test_limits();
        for _ in 0..4 {
            t.limits.wait_http().unwrap();
        }
        assert_eq!(t.clock.sleeps(), [secs(2); 3]);

        let restarted = Limits::new(t.clock.clone(), t.store.clone());
        t.clock.advance(Duration::from_millis(500));
        let before = t.clock.now_ms();
        restarted.wait_http().unwrap();
        assert_eq!(t.clock.now_ms() - before, 1500);
    }

    #[test]
    fn http_and_udp_are_spaced_independently() {
        let t = test_limits();
        t.limits.wait_udp().unwrap();
        t.limits.wait_http().unwrap();
        assert!(t.clock.sleeps().is_empty());
    }

    #[test]
    fn a_send_time_that_cannot_be_saved_stops_the_request() {
        let t = test_limits();
        t.store.fail_writes(true);
        assert!(matches!(t.limits.wait_udp(), Err(AnidbError::Storage(_))));
        assert!(matches!(t.limits.wait_http(), Err(AnidbError::Storage(_))));
        t.store.fail_writes(false);
        t.limits.wait_udp().unwrap();
        assert_eq!(
            t.store.sends(Protocol::Udp).len(),
            1,
            "only the saved send counts"
        );
    }

    #[test]
    fn a_clock_that_went_backwards_waits_at_most_one_gap() {
        let t = test_limits();
        let ahead = t.clock.now_ms() + 3_600_000;
        t.store.set_last_send(Protocol::Http, ahead);
        t.limits.wait_http().unwrap();
        assert_eq!(t.clock.sleeps(), [HTTP_SPACING]);
        assert!(
            t.store.sends(Protocol::Http)[0] >= ahead,
            "saved times never go back"
        );
    }

    #[test]
    fn unanswered_logins_back_off_in_steps_that_persist_and_reset() {
        let t = test_limits();
        let expected = [30, 120, 300, 600, 1800, 7200, 7200, 7200];
        for (i, want) in expected.into_iter().enumerate() {
            // A restart between attempts continues the escalation.
            let limits = Limits::new(t.clock.clone(), t.store.clone());
            assert_eq!(
                limits.auth_unanswered().unwrap(),
                secs(want),
                "attempt {}",
                i + 1
            );
        }
        t.limits.auth_answered().unwrap();
        assert_eq!(t.store.auth_unanswered().unwrap(), 0);
        assert_eq!(
            t.limits.auth_unanswered().unwrap(),
            secs(30),
            "starts over after a login"
        );
    }

    #[test]
    fn the_saved_backoff_survives_a_failing_store_without_changing() {
        let t = test_limits();
        t.limits.auth_unanswered().unwrap();
        t.store.fail_writes(true);
        assert!(matches!(
            t.limits.auth_unanswered(),
            Err(AnidbError::Storage(_))
        ));
        t.store.fail_writes(false);
        assert_eq!(t.limits.auth_unanswered().unwrap(), secs(120));
    }
}
