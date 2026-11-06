//! Synchronous AniDB UDP API client (protocol version 3).
//!
//! Intended to be owned by a single worker thread. Packet spacing (≥2 s between packets, ≥4 s
//! once a burst exceeds five) comes from the process-wide [`Limits`] every client shares, which
//! saves each send time before the packet leaves. The client re-authenticates on expired
//! sessions, supports compressed and encrypted sessions, and matches responses to requests
//! with tags. It binds the configured local port and never silently uses another one.

use std::io::Read;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aes::Aes128;
use aes::cipher::array::Array;
use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use md5::{Digest, Md5};

use crate::error::{AnidbError, Result};
use crate::limits::Limits;

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
            self.session = None;
            self.cipher = None;
            self.banned = None;
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
            let sock = UdpSocket::bind(("0.0.0.0", port)).map_err(|e| {
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
                self.session = None;
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
        if let Some(s) = self.session.take() {
            let line = Self::encode("LOGOUT", &[("s", s)]);
            let _ = self.exchange(&line);
        }
        self.cipher = None;
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
            match resp.code {
                501 | 506 if attempt == 0 => {
                    self.session = None;
                    self.cipher = None;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::{Limits, Protocol};
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
        let held = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
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
}
