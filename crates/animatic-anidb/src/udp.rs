//! Synchronous AniDB UDP API client (protocol version 3).
//!
//! Intended to be owned by a single worker thread. Packet spacing (≥2 s between packets, ≥4 s
//! once a burst exceeds five) comes from the process-wide [`Limits`] every client shares, which
//! saves each send time before the packet leaves. The client re-authenticates on expired
//! sessions, supports compressed and encrypted sessions, and matches responses to requests
//! with tags. It binds the configured local port and never silently uses another one.

use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aes::Aes128;
use aes::cipher::array::Array;
use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use md5::{Digest, Md5};

use crate::error::{AnidbError, Result};
use crate::limits::{
    Limits, UDP_KEEP_ALIVE_INTERVAL, UDP_MAX_IDLE_LOGOUT, UDP_NAT_PING_INTERVAL,
    UDP_SESSION_TIMEOUT,
};

pub const DEFAULT_SERVER: &str = "api.anidb.net:9000";
pub const PROTOVER: u32 = 3;

#[derive(Debug, Clone)]
pub struct Config {
    pub server: String,
    pub local_port: u16,
    pub client: String,
    pub client_version: u32,
    pub username: String,
    pub password: String,
    /// UDP API key; when set the session is encrypted.
    pub api_key: Option<String>,
    pub timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server: DEFAULT_SERVER.into(),
            local_port: 0,
            client: String::new(),
            client_version: 1,
            username: String::new(),
            password: String::new(),
            api_key: None,
            timeout: Duration::from_secs(20),
        }
    }
}

/// A parsed API reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub code: u16,
    /// Text after the code on the first line (e.g. "LOGIN ACCEPTED", or "{session} LOGIN ACCEPTED").
    pub message: String,
    /// Remaining data lines.
    pub lines: Vec<String>,
}

impl Response {
    pub fn parse(text: &str) -> Result<(Option<String>, Response)> {
        let text = text.trim_end_matches(['\n', '\r']);
        let mut lines = text
            .split('\n')
            .map(|l| l.trim_end_matches('\r').to_string());
        let first = lines.next().unwrap_or_default();
        let mut words = first.splitn(2, ' ');
        let w1 = words.next().unwrap_or_default();
        let rest = words.next().unwrap_or_default();
        let (tag, code_and_msg) = if w1.len() == 3 && w1.chars().all(|c| c.is_ascii_digit()) {
            (None, first.as_str())
        } else {
            (Some(w1.to_string()), rest)
        };
        let code: u16 = code_and_msg
            .get(..3)
            .and_then(|c| c.parse().ok())
            .ok_or_else(|| AnidbError::Protocol(format!("malformed reply: {first}")))?;
        let message = code_and_msg.get(4..).unwrap_or_default().to_string();
        Ok((
            tag,
            Response {
                code,
                message,
                lines: lines.collect(),
            },
        ))
    }

    /// Fields of data line `i`, unescaped.
    pub fn fields(&self, i: usize) -> Vec<String> {
        self.lines
            .get(i)
            .map(|l| split_fields(l))
            .unwrap_or_default()
    }
}

/// Split a data line on '|' and undo AniDB's escaping (`<br />` → newline, '`' → apostrophe).
pub fn split_fields(line: &str) -> Vec<String> {
    line.split('|').map(unescape).collect()
}

pub fn unescape(s: &str) -> String {
    s.replace("<br />", "\n").replace('`', "'")
}

/// Escape a parameter value for a request.
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace("\r\n", "<br />")
        .replace('\n', "<br />")
}

/// The source port AniDB saw the login packet come from. With `nat=1` a successful `AUTH`
/// answers `{session} {ip}:{port} LOGIN ACCEPTED`; a reply without an address (or with a port
/// that is not a number) says nothing about NAT.
fn reported_port(message: &str) -> Option<u16> {
    let address = message.split_whitespace().nth(1)?;
    address.rsplit_once(':')?.1.parse().ok()
}

/// What a held session needs between commands: see [`UdpClient::upkeep`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpkeepPolicy {
    /// More commands are expected soon: keep the session, with a keep-alive command at least
    /// every [`UDP_KEEP_ALIVE_INTERVAL`], instead of logging out when it idles.
    pub work_expected: bool,
    /// Without expected work, log out after this long without a command. Longer than
    /// [`UDP_MAX_IDLE_LOGOUT`] counts as that.
    pub idle_logout: Duration,
}

/// What one [`UdpClient::upkeep`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upkeep {
    /// Nothing was due.
    Nothing,
    /// `PING nat=1` went out to keep the router mapping, and with it the session's port.
    NatPing,
    /// A keep-alive command went out.
    KeepAlive,
    /// `LOGOUT` went out after the session idled.
    LoggedOut,
    /// AniDB no longer has the session; it was forgotten and nothing was sent for it.
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Expire,
    KeepAlive,
    Logout,
    NatPing,
}

/// A UDP client. Spacing between packets is not its own: every client of a process shares one
/// [`Limits`], so a new client per queue run cannot restart the burst allowance.
pub struct UdpClient {
    cfg: Config,
    socket: Option<UdpSocket>,
    addr: Option<SocketAddr>,
    session: Option<String>,
    cipher: Option<Aes128>,
    limits: Arc<Limits>,
    tag: u32,
    /// Image server announced at login (imgserver=1).
    pub image_server: Option<String>,
    /// Set when the server told us a newer client version exists (201).
    pub new_version_available: bool,
    /// Earliest time another packet may be sent after a server-side error (600/601/602/604).
    backoff_until: Option<Instant>,
    banned: Option<String>,
    /// The port AniDB saw the login packet come from (`nat=1`), when it said so. A session is
    /// bound to it: if it differs from our own port, a router rewrote it and only a mapping
    /// that stays alive keeps the session valid.
    reported_port: Option<u16>,
    /// When the last packet of any kind was sent, on the limiter's clock (ms since the epoch).
    last_packet_ms: Option<i64>,
    /// When the last packet that belongs to the session (login or authenticated command) was
    /// sent. `PING` is not authenticated and does not count: AniDB expires a session after 35
    /// minutes without a command.
    last_session_ms: Option<i64>,
}

impl UdpClient {
    pub fn new(cfg: Config, limits: Arc<Limits>) -> Self {
        UdpClient {
            cfg,
            socket: None,
            addr: None,
            session: None,
            cipher: None,
            limits,
            tag: 0,
            image_server: None,
            new_version_available: false,
            backoff_until: None,
            banned: None,
            reported_port: None,
            last_packet_ms: None,
            last_session_ms: None,
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn set_config(&mut self, cfg: Config) {
        let reconnect = cfg.server != self.cfg.server || cfg.local_port != self.cfg.local_port;
        let relogin = reconnect
            || cfg.username != self.cfg.username
            || cfg.password != self.cfg.password
            || cfg.api_key != self.cfg.api_key
            || cfg.client != self.cfg.client
            || cfg.client_version != self.cfg.client_version;
        self.cfg = cfg;
        if reconnect {
            self.socket = None;
            self.addr = None;
        }
        if relogin {
            self.forget_session();
            self.banned = None;
        }
    }

    /// Drop the session, its cipher and what was learned at login, without sending anything.
    fn forget_session(&mut self) {
        self.session = None;
        self.cipher = None;
        self.reported_port = None;
        self.last_session_ms = None;
    }

    /// The local UDP port in use, once the socket is open.
    pub fn local_port(&self) -> Option<u16> {
        self.socket
            .as_ref()
            .and_then(|s| s.local_addr().ok())
            .map(|a| a.port())
    }

    /// True when AniDB reported a source port at login that is not our own: a router
    /// rewrites the port, so the mapping must be kept alive (see [`UdpClient::upkeep`]). False
    /// when the ports agree or AniDB did not say.
    pub fn nat_mapped(&self) -> bool {
        match (self.reported_port, self.local_port()) {
            (Some(reported), Some(local)) => reported != local,
            _ => false,
        }
    }

    pub fn is_logged_in(&self) -> bool {
        self.session.is_some()
    }

    pub fn banned_reason(&self) -> Option<&str> {
        self.banned.as_deref()
    }

    fn socket(&mut self) -> Result<(&UdpSocket, SocketAddr)> {
        if self.addr.is_none() {
            let addr = self
                .cfg
                .server
                .to_socket_addrs()
                .map_err(|e| {
                    AnidbError::Network(format!("cannot resolve {}: {e}", self.cfg.server))
                })?
                .find(SocketAddr::is_ipv4)
                .ok_or_else(|| {
                    AnidbError::Network(format!("no IPv4 address for {}", self.cfg.server))
                })?;
            self.addr = Some(addr);
        }
        if self.socket.is_none() {
            // Never another port than the configured one: many different source ports from one
            // address within an hour get it banned. Port 0 is chosen by the caller (tests).
            let port = self.cfg.local_port;
            // A loopback server (the test fixtures) is reached from the loopback interface, so
            // no socket listens on the network and no firewall prompt appears.
            let ip = if self.addr.is_some_and(|a| a.ip().is_loopback()) {
                Ipv4Addr::LOCALHOST
            } else {
                Ipv4Addr::UNSPECIFIED
            };
            let sock = UdpSocket::bind((ip, port)).map_err(|e| {
                AnidbError::Network(if port == 0 {
                    format!("cannot open a UDP socket: {e}")
                } else {
                    format!(
                        "cannot bind UDP port {port}: {e}; the port is set by anidb.local_port, so close the program that uses it or choose another free port above 1024"
                    )
                })
            })?;
            self.socket = Some(sock);
        }
        Ok((self.socket.as_ref().unwrap(), self.addr.unwrap()))
    }

    fn next_tag(&mut self) -> String {
        self.tag = self.tag % 9999 + 1;
        format!("t{}", self.tag)
    }

    fn encrypt(&self, data: &[u8]) -> Vec<u8> {
        let Some(cipher) = &self.cipher else {
            return data.to_vec();
        };
        let pad = 16 - data.len() % 16;
        let mut buf = data.to_vec();
        buf.extend(std::iter::repeat_n(pad as u8, pad));
        let (blocks, _) = Array::slice_as_chunks_mut(&mut buf);
        cipher.encrypt_blocks(blocks);
        buf
    }

    fn decrypt(&self, data: &mut Vec<u8>) {
        let Some(cipher) = &self.cipher else { return };
        if !data.len().is_multiple_of(16) {
            return;
        }
        let (blocks, _) = Array::slice_as_chunks_mut(data.as_mut_slice());
        cipher.decrypt_blocks(blocks);
        if let Some(&pad) = data.last() {
            let pad = usize::from(pad);
            if (1..=16).contains(&pad) && pad <= data.len() {
                data.truncate(data.len() - pad);
            }
        }
    }

    /// Send one raw command line and wait for the reply with the matching tag.
    fn exchange(&mut self, line: &str) -> Result<Response> {
        if let Some(reason) = &self.banned {
            return Err(AnidbError::Banned(reason.clone()));
        }
        if let Some(until) = self.backoff_until {
            let now = Instant::now();
            if until > now {
                return Err(AnidbError::ServerUnavailable(until - now));
            }
            self.backoff_until = None;
        }
        let tag = self.next_tag();
        let line = format!("{line}&tag={tag}");
        let payload = self.encrypt(line.as_bytes());
        let timeout = self.cfg.timeout;
        // Resolve and bind first: a packet that cannot be sent must not use up a send slot.
        let addr = self.socket()?.1;
        // The send time is saved before the packet leaves (see `Limits::wait_udp`).
        self.limits.wait_udp()?;
        self.last_packet_ms = Some(self.limits.now_ms());
        let sock = self.socket.as_ref().expect("opened by socket()");
        sock.set_read_timeout(Some(timeout)).ok();
        sock.send_to(&payload, addr)
            .map_err(|e| AnidbError::Network(e.to_string()))?;
        let deadline = Instant::now() + timeout;
        let mut buf = vec![0u8; 16384];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(AnidbError::Timeout);
            }
            let sock = self.socket.as_ref().unwrap();
            sock.set_read_timeout(Some(left)).ok();
            let n = match sock.recv_from(&mut buf) {
                Ok((n, from)) if from.ip() == addr.ip() => n,
                Ok(_) => continue,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(AnidbError::Timeout);
                }
                Err(e) => return Err(AnidbError::Network(e.to_string())),
            };
            let mut data = buf[..n].to_vec();
            self.decrypt(&mut data);
            if data.starts_with(&[0, 0]) {
                let mut out = Vec::new();
                flate2::read::ZlibDecoder::new(&data[2..])
                    .read_to_end(&mut out)
                    .or_else(|_| {
                        flate2::read::DeflateDecoder::new(&data[2..]).read_to_end(&mut out)
                    })
                    .map_err(|e| AnidbError::Protocol(format!("bad compressed reply: {e}")))?;
                data = out;
            }
            let text = String::from_utf8_lossy(&data).into_owned();
            let (rtag, resp) = Response::parse(&text)?;
            match rtag {
                Some(t) if t == tag => return self.check_global(resp),
                Some(_) => continue, // stale reply of an earlier, timed out request
                None => return self.check_global(resp),
            }
        }
    }

    /// Handle replies that are errors regardless of the command.
    fn check_global(&mut self, resp: Response) -> Result<Response> {
        match resp.code {
            // 510 is treated like an explicit ban: the client stops and never retries on its own.
            510 | 555 => {
                let reason = if resp.message.is_empty() {
                    "banned".to_string()
                } else {
                    resp.message.clone()
                };
                self.banned = Some(reason.clone());
                self.forget_session();
                Err(AnidbError::Banned(reason))
            }
            503 => Err(AnidbError::ClientOutdated),
            // The client name and version are banned for good: nothing helps until they change,
            // so this is a correction-needed error like 503, not a timed ban.
            504 => Err(AnidbError::ClientBanned),
            598 => Err(AnidbError::UnknownCommand),
            600 | 601 | 602 | 604 => {
                let wait = if resp.code == 604 {
                    Duration::from_secs(60)
                } else {
                    Duration::from_secs(30 * 60)
                };
                self.backoff_until = Some(Instant::now() + wait);
                Err(AnidbError::ServerError(resp.code, resp.message))
            }
            _ => Ok(resp),
        }
    }

    fn encode(cmd: &str, params: &[(&str, String)]) -> String {
        let mut line = String::from(cmd);
        for (i, (k, v)) in params.iter().enumerate() {
            line.push(if i == 0 { ' ' } else { '&' });
            line.push_str(k);
            line.push('=');
            line.push_str(&escape(v));
        }
        line
    }

    fn start_encryption(&mut self) -> Result<()> {
        let Some(key) = self.cfg.api_key.clone().filter(|k| !k.is_empty()) else {
            return Ok(());
        };
        self.cipher = None;
        let line = Self::encode(
            "ENCRYPT",
            &[("user", self.cfg.username.clone()), ("type", "1".into())],
        );
        let resp = self.exchange(&line)?;
        match resp.code {
            209 => {
                let salt = resp.message.split(' ').next().unwrap_or_default();
                let digest = Md5::digest(format!("{key}{salt}").as_bytes());
                let key: [u8; 16] = digest.into();
                self.cipher = Some(Aes128::new(&key.into()));
                Ok(())
            }
            309 => Err(AnidbError::Auth(
                "API key not defined in your AniDB profile".into(),
            )),
            394 => Err(AnidbError::Auth("no such user".into())),
            _ => Err(AnidbError::Unexpected(resp.code, resp.message)),
        }
    }

    /// Log in (also called implicitly by [`UdpClient::request`]). An `AUTH` (or the `ENCRYPT`
    /// before it) without a reply is answered with [`AnidbError::AuthTimeout`] carrying the
    /// next step of the escalating, saved backoff; a successful login resets it.
    pub fn auth(&mut self) -> Result<()> {
        if self.cfg.client.is_empty() {
            return Err(AnidbError::Auth("no AniDB client name configured".into()));
        }
        if self.cfg.username.is_empty() || self.cfg.password.is_empty() {
            return Err(AnidbError::Auth("username and password required".into()));
        }
        match self.login() {
            Err(AnidbError::Timeout) => {
                Err(AnidbError::AuthTimeout(self.limits.auth_unanswered()?))
            }
            Ok(()) => {
                // Failing to reset only keeps a longer backoff for the next unanswered login.
                if let Err(e) = self.limits.auth_answered() {
                    log::warn!("could not reset the AUTH backoff: {e}");
                }
                Ok(())
            }
            other => other,
        }
    }

    /// Make sure a session exists, logging in if needed. Call it before recording that a
    /// request is about to be sent, so a login that fails first does not count as a request.
    pub fn ensure_login(&mut self) -> Result<()> {
        if self.session.is_none() {
            self.auth()?;
        }
        Ok(())
    }

    fn login(&mut self) -> Result<()> {
        self.start_encryption()?;
        let line = Self::encode(
            "AUTH",
            &[
                ("user", self.cfg.username.clone()),
                ("pass", self.cfg.password.clone()),
                ("protover", PROTOVER.to_string()),
                ("client", self.cfg.client.clone()),
                ("clientver", self.cfg.client_version.to_string()),
                ("nat", "1".into()),
                ("comp", "1".into()),
                ("enc", "UTF8".into()),
                ("imgserver", "1".into()),
            ],
        );
        let resp = self.exchange(&line)?;
        match resp.code {
            200 | 201 => {
                let session = resp
                    .message
                    .split(' ')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                if session.is_empty() {
                    return Err(AnidbError::Protocol(
                        "login reply without session key".into(),
                    ));
                }
                self.session = Some(session);
                self.reported_port = reported_port(&resp.message);
                self.last_session_ms = self.last_packet_ms;
                self.new_version_available = resp.code == 201;
                self.image_server = resp
                    .lines
                    .first()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                Ok(())
            }
            500 => Err(AnidbError::Auth(
                "login failed: wrong username or password".into(),
            )),
            505 => Err(AnidbError::Auth(
                "illegal input or access denied (is the client registered?)".into(),
            )),
            _ => Err(AnidbError::Unexpected(resp.code, resp.message)),
        }
    }

    pub fn logout(&mut self) -> Result<()> {
        if let Some(s) = self.session.clone() {
            let line = Self::encode("LOGOUT", &[("s", s)]);
            let _ = self.exchange(&line);
        }
        self.forget_session();
        Ok(())
    }

    /// Run an authenticated command, logging in first and retrying once on an expired session.
    pub fn request(&mut self, cmd: &str, params: &[(&str, String)]) -> Result<Response> {
        for attempt in 0..2 {
            if self.session.is_none() {
                self.auth()?;
            }
            let mut p: Vec<(&str, String)> = params.to_vec();
            p.push(("s", self.session.clone().unwrap_or_default()));
            let line = Self::encode(cmd, &p);
            let resp = match self.exchange(&line) {
                Err(AnidbError::Timeout) if attempt == 0 => continue,
                other => other?,
            };
            if !matches!(resp.code, 501 | 506) {
                self.last_session_ms = self.last_packet_ms;
            }
            match resp.code {
                501 | 506 if attempt == 0 => {
                    self.forget_session();
                    continue;
                }
                502 => return Err(AnidbError::Auth("access denied".into())),
                505 => return Err(AnidbError::IllegalInput(resp.message)),
                _ => return Ok(resp),
            }
        }
        Err(AnidbError::Timeout)
    }

    /// Unauthenticated PING (keeps NAT mappings alive).
    pub fn ping(&mut self) -> Result<()> {
        let resp = self.exchange("PING nat=1")?;
        if resp.code == 300 {
            Ok(())
        } else {
            Err(AnidbError::Unexpected(resp.code, resp.message))
        }
    }

    /// How long until [`UdpClient::upkeep`] has something to do for the held session, on the
    /// same policy; `None` when there is no session. Zero means it is due now.
    pub fn next_upkeep(&self, policy: &UpkeepPolicy) -> Option<Duration> {
        self.next_step(policy).map(|(_, due)| due)
    }

    /// Do what the held session needs now, if anything, and say what it was: at most one
    /// packet. Call it when [`UdpClient::next_upkeep`] is due; it does nothing earlier. Every
    /// packet takes its send slot from the shared limiter.
    ///
    /// - A session idle for [`UDP_SESSION_TIMEOUT`] is already gone at AniDB: it is forgotten
    ///   without a packet.
    /// - While work is expected, a keep-alive command goes out every
    ///   [`UDP_KEEP_ALIVE_INTERVAL`] of silence.
    /// - Otherwise the client logs out after the policy's idle time.
    /// - On a NAT-mapped session ([`UdpClient::nat_mapped`]) a `PING nat=1` goes out after
    ///   [`UDP_NAT_PING_INTERVAL`] without any packet. Any other packet restarts that interval,
    ///   so a busy session never pings.
    pub fn upkeep(&mut self, policy: &UpkeepPolicy) -> Result<Upkeep> {
        match self.next_step(policy) {
            Some((step, due)) if due.is_zero() => match step {
                Step::Expire => {
                    self.forget_session();
                    Ok(Upkeep::Expired)
                }
                Step::KeepAlive => self.keep_alive(),
                Step::Logout => {
                    self.logout()?;
                    Ok(Upkeep::LoggedOut)
                }
                Step::NatPing => {
                    self.ping()?;
                    Ok(Upkeep::NatPing)
                }
            },
            _ => Ok(Upkeep::Nothing),
        }
    }

    /// The step the held session needs next and how long until it is due. Ties go to the
    /// step listed first: forgetting an expired session beats sending anything.
    fn next_step(&self, policy: &UpkeepPolicy) -> Option<(Step, Duration)> {
        self.session.as_ref()?;
        let now = self.limits.now_ms();
        let since = |at: Option<i64>| {
            at.map_or(Duration::ZERO, |at| {
                Duration::from_millis(u64::try_from(now.saturating_sub(at)).unwrap_or(0))
            })
        };
        let session_idle = since(self.last_session_ms);
        let mut best: Option<(Step, Duration)> = None;
        let mut consider = |step: Step, due: Duration| {
            if best.is_none_or(|(_, earliest)| due < earliest) {
                best = Some((step, due));
            }
        };
        consider(
            Step::Expire,
            UDP_SESSION_TIMEOUT.saturating_sub(session_idle),
        );
        if policy.work_expected {
            consider(
                Step::KeepAlive,
                UDP_KEEP_ALIVE_INTERVAL.saturating_sub(session_idle),
            );
        } else {
            let idle_logout = policy.idle_logout.min(UDP_MAX_IDLE_LOGOUT);
            consider(Step::Logout, idle_logout.saturating_sub(session_idle));
        }
        if self.nat_mapped() {
            consider(
                Step::NatPing,
                UDP_NAT_PING_INTERVAL.saturating_sub(since(self.last_packet_ms)),
            );
        }
        best
    }

    /// `UPTIME`, the cheapest authenticated command: it shows AniDB the session is in use.
    fn keep_alive(&mut self) -> Result<Upkeep> {
        let Some(session) = self.session.clone() else {
            return Ok(Upkeep::Nothing);
        };
        let line = Self::encode("UPTIME", &[("s", session)]);
        let resp = self.exchange(&line)?;
        match resp.code {
            208 => {
                self.last_session_ms = self.last_packet_ms;
                Ok(Upkeep::KeepAlive)
            }
            // AniDB no longer knows the session (it expired, or the source port changed). The
            // next command logs in again.
            501 | 506 => {
                self.forget_session();
                Ok(Upkeep::Expired)
            }
            _ => Err(AnidbError::Unexpected(resp.code, resp.message)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::{Clock as _, Limits, Protocol};
    use crate::testing::{TestLimits, UdpFixture, test_limits};
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn client() -> UdpClient {
        UdpClient::new(Config::default(), test_limits().limits)
    }

    fn config_for(server: std::net::SocketAddr) -> Config {
        Config {
            server: server.to_string(),
            client: "syntheticclient".into(),
            username: "synthetic-user".into(),
            password: "synthetic-password".into(),
            timeout: Duration::from_millis(150),
            ..Config::default()
        }
    }

    #[test]
    fn parses_tagged_and_untagged_replies() {
        let (tag, r) =
            Response::parse("t12 200 abcde 1.2.3.4:9000 LOGIN ACCEPTED\ncdn.anidb.net\n").unwrap();
        assert_eq!(tag.as_deref(), Some("t12"));
        assert_eq!(r.code, 200);
        assert_eq!(r.message, "abcde 1.2.3.4:9000 LOGIN ACCEPTED");
        assert_eq!(r.lines, vec!["cdn.anidb.net"]);
        let (tag, r) = Response::parse("555 BANNED\nflooding").unwrap();
        assert_eq!(tag, None);
        assert_eq!(r.code, 555);
        assert_eq!(r.lines, vec!["flooding"]);
    }

    #[test]
    fn field_escaping_round_trip() {
        assert_eq!(escape("a&b\nc"), "a&amp;b<br />c");
        assert_eq!(
            split_fields("1|It`s<br />two|x"),
            vec!["1", "It's\ntwo", "x"]
        );
    }

    #[test]
    fn encrypt_decrypt_round_trip_with_pkcs_padding() {
        let mut c = client();
        c.cipher = Some(Aes128::new(&[7u8; 16].into()));
        for len in [0usize, 5, 16, 33] {
            let msg: Vec<u8> = (0..len as u8).collect();
            let mut enc = c.encrypt(&msg);
            assert_eq!(enc.len() % 16, 0);
            c.decrypt(&mut enc);
            assert_eq!(enc, msg);
        }
    }

    fn reply(code: u16) -> Response {
        Response {
            code,
            message: String::new(),
            lines: Vec::new(),
        }
    }

    #[test]
    fn ban_replies_stop_the_client_and_never_retry() {
        for code in [510, 555] {
            let mut c = client();
            assert!(matches!(
                c.check_global(reply(code)),
                Err(AnidbError::Banned(_))
            ));
            assert!(c.banned_reason().is_some());
            // Nothing is sent: the socket is never even opened once banned.
            assert!(matches!(
                c.exchange("PING nat=1"),
                Err(AnidbError::Banned(_))
            ));
            assert!(c.socket.is_none());
        }
    }

    #[test]
    fn a_banned_client_version_needs_a_correction_instead_of_a_timed_ban() {
        let mut c = client();
        let err = c.check_global(reply(504)).unwrap_err();
        assert!(matches!(err, AnidbError::ClientBanned), "{err:?}");
        assert!(err.is_fatal() && !err.is_transient() && err.retry_after().is_none());
        assert!(err.to_string().contains("banned"), "{err}");
    }

    #[test]
    fn global_replies_are_classified() {
        let mut c = client();
        assert!(matches!(
            c.check_global(reply(503)),
            Err(AnidbError::ClientOutdated)
        ));
        assert!(matches!(
            c.check_global(reply(598)),
            Err(AnidbError::UnknownCommand)
        ));
        assert!(c.check_global(reply(200)).is_ok());
        assert!(matches!(
            c.check_global(reply(604)),
            Err(AnidbError::ServerError(604, _))
        ));
        let Err(AnidbError::ServerUnavailable(left)) = c.exchange("PING nat=1") else {
            panic!("server backoff must stop further packets");
        };
        assert!(left <= Duration::from_secs(60));
        assert!(c.socket.is_none());
    }

    #[test]
    fn a_busy_local_port_is_an_error_and_never_replaced_by_another_port() {
        let held = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let server = UdpFixture::start(|_, _| "300 PONG".into());
        let t = test_limits();
        let mut c = UdpClient::new(
            Config {
                local_port: port,
                ..config_for(server.addr)
            },
            t.limits.clone(),
        );
        let err = c.ping().unwrap_err();
        let AnidbError::Network(message) = &err else {
            panic!("{err:?}");
        };
        assert!(message.contains("anidb.local_port"), "{message}");
        assert!(message.contains(&port.to_string()), "{message}");
        assert_eq!(server.packets(), 0, "nothing was sent from another port");
        assert!(
            t.store.sends(Protocol::Udp).is_empty(),
            "no send slot was used up"
        );

        // Once the port is free it is bound again.
        drop(held);
        let mut c = UdpClient::new(
            Config {
                local_port: port,
                ..config_for(server.addr)
            },
            t.limits.clone(),
        );
        c.ping().unwrap();
        assert_eq!(
            c.socket.as_ref().unwrap().local_addr().unwrap().port(),
            port
        );
    }

    #[test]
    fn clients_of_one_process_share_the_spacing_and_a_restart_keeps_it() {
        let t = test_limits();
        let arrivals = Arc::new(Mutex::new(Vec::new()));
        let log = arrivals.clone();
        let clock = t.clock.clone();
        let server = UdpFixture::start(move |_, _| {
            use crate::limits::Clock;
            log.lock().push(clock.now_ms());
            "300 PONG".into()
        });
        // Three "runs", each with brand new clients, one of them after a restart.
        for run in 0..3 {
            let limits = if run == 2 {
                Limits::new(t.clock.clone(), t.store.clone())
            } else {
                t.limits.clone()
            };
            let mut c = UdpClient::new(config_for(server.addr), limits);
            c.ping().unwrap();
            c.ping().unwrap();
        }
        let arrivals = arrivals.lock().clone();
        let gaps: Vec<i64> = arrivals.windows(2).map(|w| w[1] - w[0]).collect();
        // Four packets of two runs keep the 2 s burst spacing. The restart cannot know the
        // burst size, so its first and every later packet keep the 4 s long-term spacing.
        assert_eq!(gaps, [2000, 2000, 2000, 4000, 4000]);
        assert_eq!(t.store.sends(Protocol::Udp), arrivals);
    }

    #[test]
    fn unanswered_logins_back_off_in_steps_and_a_successful_login_starts_over() {
        let t: TestLimits = test_limits();
        let answer = Arc::new(AtomicBool::new(false));
        let reply_when = answer.clone();
        let server = UdpFixture::start(move |_, _| {
            if reply_when.load(Ordering::SeqCst) {
                "200 session 127.0.0.1:1 LOGIN ACCEPTED".into()
            } else {
                String::new()
            }
        });
        let wait_of = |t: &TestLimits| {
            let mut c = UdpClient::new(config_for(server.addr), t.limits.clone());
            match c.auth() {
                Err(AnidbError::AuthTimeout(wait)) => wait.as_secs(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(
            [wait_of(&t), wait_of(&t), wait_of(&t)],
            [30, 120, 300],
            "each unanswered AUTH escalates, also from a new client"
        );
        answer.store(true, Ordering::SeqCst);
        let mut c = UdpClient::new(config_for(server.addr), t.limits.clone());
        c.auth().unwrap();
        assert!(c.is_logged_in());
        answer.store(false, Ordering::SeqCst);
        assert_eq!(wait_of(&t), 30, "a login resets the escalation");
    }

    #[test]
    fn malformed_replies_are_protocol_errors() {
        for bad in ["", "x", "99 short", "t1", "t1 abc"] {
            assert!(
                matches!(Response::parse(bad), Err(AnidbError::Protocol(_))),
                "{bad:?}"
            );
        }
    }

    fn mins(n: u64) -> Duration {
        Duration::from_secs(n * 60)
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// Log out after `minutes` idle minutes; no work is expected.
    fn idle_for(minutes: u64) -> UpkeepPolicy {
        UpkeepPolicy {
            work_expected: false,
            idle_logout: mins(minutes),
        }
    }

    fn expecting_work() -> UpkeepPolicy {
        UpkeepPolicy {
            work_expected: true,
            idle_logout: mins(10),
        }
    }

    /// A stand-in AniDB that logs in, keeps sessions and answers the upkeep commands, and the
    /// log of every request it got. With `nat` it reports a source port that is not the
    /// client's own, as behind a router that rewrites it.
    fn session_server(nat: bool, uptime: &'static str) -> (UdpFixture, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        let server = UdpFixture::start_from(move |_, request, peer| {
            seen.lock().push(request.to_string());
            let port = if nat { peer.port() ^ 1 } else { peer.port() };
            let name = request.split([' ', '&']).next().unwrap_or_default();
            match name {
                "AUTH" => format!("200 session {}:{port} LOGIN ACCEPTED", peer.ip()),
                "UPTIME" => uptime.into(),
                "PING" => format!("300 PONG\n{port}"),
                "LOGOUT" => "203 LOGGED OUT".into(),
                _ => "598 UNKNOWN COMMAND".into(),
            }
        });
        (server, log)
    }

    /// The command names the server has received, in order.
    fn sent(log: &Mutex<Vec<String>>) -> Vec<String> {
        log.lock()
            .iter()
            .map(|r| r.split([' ', '&']).next().unwrap_or_default().to_string())
            .collect()
    }

    /// A logged-in client of `server` whose clock is `t.clock`.
    fn logged_in(server: &UdpFixture, t: &TestLimits) -> UdpClient {
        let mut c = UdpClient::new(config_for(server.addr), t.limits.clone());
        c.auth().unwrap();
        c
    }

    /// Let `total` pass, doing each upkeep step when it falls due; returns what was done.
    fn drive(
        c: &mut UdpClient,
        t: &TestLimits,
        policy: &UpkeepPolicy,
        total: Duration,
    ) -> Vec<Upkeep> {
        let end = t.clock.now_ms() + i64::try_from(total.as_millis()).unwrap();
        let mut done = Vec::new();
        for _ in 0..1000 {
            let Some(due) = c.next_upkeep(policy) else {
                break;
            };
            if t.clock.now_ms() + i64::try_from(due.as_millis()).unwrap() > end {
                break;
            }
            t.clock.advance(due);
            done.push(c.upkeep(policy).unwrap());
        }
        done
    }

    #[test]
    fn the_login_reply_tells_whether_a_router_rewrote_the_port() {
        assert_eq!(
            reported_port("abcde 1.2.3.4:9000 LOGIN ACCEPTED"),
            Some(9000)
        );
        assert_eq!(
            reported_port("abcde 1.2.3.4:9000 LOGIN ACCEPTED - NEW VERSION AVAILABLE"),
            Some(9000)
        );
        assert_eq!(reported_port("abcde LOGIN ACCEPTED"), None);
        assert_eq!(reported_port("abcde 1.2.3.4:port LOGIN ACCEPTED"), None);
        assert_eq!(reported_port(""), None);

        let t = test_limits();
        let (same, _) = session_server(false, "208 UPTIME");
        assert!(!logged_in(&same, &t).nat_mapped());
        let (rewritten, _) = session_server(true, "208 UPTIME");
        assert!(logged_in(&rewritten, &t).nat_mapped());
    }

    #[test]
    fn an_idle_session_logs_out_after_the_idle_time_and_not_before() {
        let t = test_limits();
        let (server, log) = session_server(false, "208 UPTIME");
        let mut c = logged_in(&server, &t);
        let policy = idle_for(10);
        assert_eq!(c.next_upkeep(&policy), Some(mins(10)));

        t.clock.advance(mins(10) - secs(1));
        assert_eq!(c.upkeep(&policy).unwrap(), Upkeep::Nothing);
        assert_eq!(sent(&log), ["AUTH"]);
        t.clock.advance(secs(1));
        assert_eq!(c.upkeep(&policy).unwrap(), Upkeep::LoggedOut);
        assert_eq!(sent(&log), ["AUTH", "LOGOUT"]);
        assert!(!c.is_logged_in());
        assert_eq!(c.next_upkeep(&policy), None, "nothing is held any more");
        assert_eq!(c.upkeep(&policy).unwrap(), Upkeep::Nothing);
        assert_eq!(server.packets(), 2);
    }

    #[test]
    fn the_idle_time_counts_from_the_last_command_and_never_exceeds_the_server_timeout() {
        let t = test_limits();
        let (server, log) = session_server(false, "208 UPTIME");
        let mut c = logged_in(&server, &t);
        let policy = idle_for(10);

        t.clock.advance(mins(9));
        c.request("UPTIME", &[]).unwrap();
        t.clock.advance(mins(9));
        assert_eq!(
            c.upkeep(&policy).unwrap(),
            Upkeep::Nothing,
            "the command restarted the idle time"
        );
        t.clock.advance(mins(1));
        assert_eq!(c.upkeep(&policy).unwrap(), Upkeep::LoggedOut);
        assert_eq!(sent(&log), ["AUTH", "UPTIME", "LOGOUT"]);

        // A setting longer than the keep-alive interval is held to it, so the session is
        // always logged out before AniDB would expire it.
        let c = logged_in(&server, &t);
        let hours = idle_for(180);
        assert_eq!(c.next_upkeep(&hours), Some(UDP_MAX_IDLE_LOGOUT));
        assert!(UDP_MAX_IDLE_LOGOUT < UDP_SESSION_TIMEOUT);
        // No idle time at all logs out at once.
        assert_eq!(c.next_upkeep(&idle_for(0)), Some(Duration::ZERO));
    }

    #[test]
    fn a_session_with_expected_work_is_kept_by_a_command_at_least_every_30_minutes() {
        let t = test_limits();
        let (server, log) = session_server(false, "208 UPTIME");
        let mut c = logged_in(&server, &t);
        let policy = expecting_work();
        assert_eq!(c.next_upkeep(&policy), Some(UDP_KEEP_ALIVE_INTERVAL));

        // Two hours: a keep-alive every 30 minutes, no login, no logout.
        let done = drive(&mut c, &t, &policy, mins(120));
        assert_eq!(done, [Upkeep::KeepAlive; 4]);
        assert_eq!(sent(&log), ["AUTH", "UPTIME", "UPTIME", "UPTIME", "UPTIME"]);
        assert!(
            log.lock()[1].starts_with("UPTIME s=session&tag="),
            "an authenticated command keeps the session: {:?}",
            log.lock()[1]
        );
        assert!(c.is_logged_in());
    }

    #[test]
    fn when_the_work_is_gone_the_session_logs_out_instead_of_being_kept() {
        let t = test_limits();
        let (server, log) = session_server(false, "208 UPTIME");
        let mut c = logged_in(&server, &t);

        t.clock.advance(mins(20));
        let no_work = idle_for(10);
        assert_eq!(c.upkeep(&no_work).unwrap(), Upkeep::LoggedOut);
        assert_eq!(sent(&log), ["AUTH", "LOGOUT"], "no keep-alive was due");
    }

    #[test]
    fn a_session_that_idled_past_the_server_timeout_is_forgotten_without_a_packet() {
        let t = test_limits();
        let (server, log) = session_server(false, "208 UPTIME");
        let mut c = logged_in(&server, &t);

        t.clock.advance(UDP_SESSION_TIMEOUT);
        assert_eq!(c.upkeep(&expecting_work()).unwrap(), Upkeep::Expired);
        assert!(!c.is_logged_in());
        assert_eq!(sent(&log), ["AUTH"], "AniDB has no session to log out");

        // A keep-alive that finds the session gone does the same, and the next command logs
        // in again.
        let (server, log) = session_server(false, "506 INVALID SESSION");
        let mut c = logged_in(&server, &t);
        t.clock.advance(UDP_KEEP_ALIVE_INTERVAL);
        assert_eq!(c.upkeep(&expecting_work()).unwrap(), Upkeep::Expired);
        assert!(!c.is_logged_in());
        assert_eq!(sent(&log), ["AUTH", "UPTIME"]);
    }

    #[test]
    fn pings_go_out_only_when_the_ports_differ_and_never_more_often_than_every_4_minutes() {
        let t = test_limits();
        // The same port on both ends: no router, so a quiet session never pings.
        let (server, log) = session_server(false, "208 UPTIME");
        let mut c = logged_in(&server, &t);
        assert!(!c.nat_mapped());
        let done = drive(&mut c, &t, &expecting_work(), mins(29));
        assert_eq!(done, []);
        assert_eq!(sent(&log), ["AUTH"]);

        // Behind a router: a ping whenever 4 minutes pass without any packet, until the
        // keep-alive command at 30 minutes (which restarts the 4 minutes as well).
        let (server, log) = session_server(true, "208 UPTIME");
        let mut c = logged_in(&server, &t);
        assert!(c.nat_mapped());
        assert_eq!(
            c.next_upkeep(&expecting_work()),
            Some(UDP_NAT_PING_INTERVAL)
        );
        let done = drive(&mut c, &t, &expecting_work(), mins(31));
        let mut expected = vec![Upkeep::NatPing; 7];
        expected.push(Upkeep::KeepAlive);
        assert_eq!(done, expected);
        let arrivals = sent(&log);
        assert_eq!(arrivals[0], "AUTH");
        assert_eq!(arrivals[1..8], ["PING"; 7]);
        assert_eq!(arrivals[8], "UPTIME");
        assert!(
            log.lock()[1].starts_with("PING nat=1&tag="),
            "{:?}",
            log.lock()[1]
        );
    }

    #[test]
    fn a_command_restarts_the_ping_interval() {
        let t = test_limits();
        let (server, log) = session_server(true, "208 UPTIME");
        let mut c = logged_in(&server, &t);
        let policy = expecting_work();

        t.clock.advance(UDP_NAT_PING_INTERVAL - secs(1));
        assert_eq!(c.upkeep(&policy).unwrap(), Upkeep::Nothing);
        t.clock.advance(secs(1));
        assert_eq!(c.upkeep(&policy).unwrap(), Upkeep::NatPing);
        t.clock.advance(mins(2));
        c.request("UPTIME", &[]).unwrap();
        t.clock.advance(UDP_NAT_PING_INTERVAL - secs(1));
        assert_eq!(
            c.upkeep(&policy).unwrap(),
            Upkeep::Nothing,
            "4 minutes since the command, not since the ping"
        );
        t.clock.advance(secs(1));
        assert_eq!(c.upkeep(&policy).unwrap(), Upkeep::NatPing);
        assert_eq!(sent(&log), ["AUTH", "PING", "UPTIME", "PING"]);
    }

    #[test]
    fn an_idle_session_behind_a_router_pings_until_it_logs_out() {
        let t = test_limits();
        let (server, log) = session_server(true, "208 UPTIME");
        let mut c = logged_in(&server, &t);
        // A ping is not a command: the idle time still runs from the login.
        let done = drive(&mut c, &t, &idle_for(10), mins(12));
        assert_eq!(done, [Upkeep::NatPing, Upkeep::NatPing, Upkeep::LoggedOut]);
        assert_eq!(sent(&log), ["AUTH", "PING", "PING", "LOGOUT"]);
    }

    #[test]
    fn changed_credentials_drop_the_held_session_and_with_it_every_timer() {
        let t = test_limits();
        let (server, log) = session_server(true, "208 UPTIME");
        let mut c = logged_in(&server, &t);
        assert!(c.nat_mapped());

        // The same settings change nothing.
        c.set_config(config_for(server.addr));
        assert!(c.is_logged_in());
        c.set_config(Config {
            password: "another-synthetic-password".into(),
            ..config_for(server.addr)
        });
        assert!(!c.is_logged_in());
        assert!(!c.nat_mapped());
        assert_eq!(c.next_upkeep(&expecting_work()), None);
        t.clock.advance(mins(60));
        assert_eq!(c.upkeep(&expecting_work()).unwrap(), Upkeep::Nothing);
        assert_eq!(sent(&log), ["AUTH"]);
    }
}
