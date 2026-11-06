//! Fixtures for tests of this crate and of the crates built on it (cargo feature
//! `test-support`): a clock that only moves when something sleeps, an in-memory
//! [`LimitStore`], and a loopback HTTP server. Nothing here talks to AniDB.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::Mutex;

use crate::limits::{Clock, LimitStore, Limits, Protocol};

/// A clock that stands still until [`Clock::sleep`] or [`ManualClock::advance`] moves it, so
/// spacing tests run instantly and deterministically.
#[derive(Debug)]
pub struct ManualClock {
    now_ms: AtomicI64,
    sleeps: Mutex<Vec<Duration>>,
}

impl ManualClock {
    pub fn at(now_ms: i64) -> ManualClock {
        ManualClock {
            now_ms: AtomicI64::new(now_ms),
            sleeps: Mutex::new(Vec::new()),
        }
    }

    /// Let time pass without anyone sleeping (an idle period, a restart that took a while).
    pub fn advance(&self, by: Duration) {
        self.now_ms.fetch_add(
            i64::try_from(by.as_millis()).unwrap_or(i64::MAX),
            Ordering::SeqCst,
        );
    }

    /// Every sleep so far, in order.
    pub fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.lock().clone()
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        ManualClock::at(1_700_000_000_000)
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }

    fn sleep(&self, duration: Duration) {
        self.sleeps.lock().push(duration);
        self.advance(duration);
    }
}

#[derive(Default)]
struct MemoryState {
    udp: Option<i64>,
    http: Option<i64>,
    udp_sends: Vec<i64>,
    http_sends: Vec<i64>,
    auth: u32,
}

/// A [`LimitStore`] that keeps everything in memory. Sharing one between two [`Limits`] values
/// models a restart.
#[derive(Default)]
pub struct MemoryStore {
    state: Mutex<MemoryState>,
    fail_writes: AtomicBool,
}

impl MemoryStore {
    /// Pretend an earlier process sent a request at `at_ms`.
    pub fn set_last_send(&self, protocol: Protocol, at_ms: i64) {
        let mut state = self.state.lock();
        match protocol {
            Protocol::Udp => state.udp = Some(at_ms),
            Protocol::Http => state.http = Some(at_ms),
        }
    }

    /// Every send time saved through [`LimitStore::record_send`], in order.
    pub fn sends(&self, protocol: Protocol) -> Vec<i64> {
        let state = self.state.lock();
        match protocol {
            Protocol::Udp => state.udp_sends.clone(),
            Protocol::Http => state.http_sends.clone(),
        }
    }

    /// Make every write fail, as a full disk or a locked database would.
    pub fn fail_writes(&self, fail: bool) {
        self.fail_writes.store(fail, Ordering::SeqCst);
    }

    fn check_writable(&self) -> Result<(), String> {
        if self.fail_writes.load(Ordering::SeqCst) {
            Err("synthetic write failure".into())
        } else {
            Ok(())
        }
    }
}

impl LimitStore for MemoryStore {
    fn last_send(&self, protocol: Protocol) -> Result<Option<i64>, String> {
        let state = self.state.lock();
        Ok(match protocol {
            Protocol::Udp => state.udp,
            Protocol::Http => state.http,
        })
    }

    fn record_send(&self, protocol: Protocol, at_ms: i64) -> Result<(), String> {
        self.check_writable()?;
        let mut state = self.state.lock();
        match protocol {
            Protocol::Udp => {
                state.udp = Some(at_ms);
                state.udp_sends.push(at_ms);
            }
            Protocol::Http => {
                state.http = Some(at_ms);
                state.http_sends.push(at_ms);
            }
        }
        Ok(())
    }

    fn auth_unanswered(&self) -> Result<u32, String> {
        Ok(self.state.lock().auth)
    }

    fn set_auth_unanswered(&self, count: u32) -> Result<(), String> {
        self.check_writable()?;
        self.state.lock().auth = count;
        Ok(())
    }
}

/// [`Limits`] over a [`ManualClock`] and a [`MemoryStore`], with handles to both.
pub struct TestLimits {
    pub limits: Arc<Limits>,
    pub clock: Arc<ManualClock>,
    pub store: Arc<MemoryStore>,
}

pub fn test_limits() -> TestLimits {
    let clock = Arc::new(ManualClock::default());
    let store = Arc::new(MemoryStore::default());
    TestLimits {
        limits: Limits::new(clock.clone(), store.clone()),
        clock,
        store,
    }
}

/// One canned HTTP response.
#[derive(Clone)]
pub struct HttpReply {
    /// Status line text, for example `"200 OK"`.
    pub status: &'static str,
    pub body: Vec<u8>,
    /// `Content-Length` to announce; shorter bodies model a connection that ends early.
    pub declared_len: Option<usize>,
}

impl HttpReply {
    pub fn ok(body: Vec<u8>) -> HttpReply {
        HttpReply {
            status: "200 OK",
            body,
            declared_len: None,
        }
    }

    pub fn status(status: &'static str) -> HttpReply {
        HttpReply {
            status,
            body: Vec::new(),
            declared_len: None,
        }
    }

    /// Announce the whole body but send only the first half.
    pub fn truncated(body: Vec<u8>) -> HttpReply {
        HttpReply {
            status: "200 OK",
            declared_len: Some(body.len()),
            body: body[..body.len() / 2].to_vec(),
        }
    }
}

/// A loopback HTTP server that answers every request, one at a time, with whatever the handler
/// returns for it. It counts requests, so a test can prove that none was made.
pub struct HttpFixture {
    base: String,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    addr: std::net::SocketAddr,
    handle: Option<JoinHandle<()>>,
}

impl HttpFixture {
    /// `reply(index, request_head)` answers the `index`-th request (from 0).
    pub fn start(reply: impl Fn(usize, &str) -> HttpReply + Send + 'static) -> HttpFixture {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local address");
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (counter, stopped) = (requests.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let mut head = Vec::new();
                let mut chunk = [0u8; 512];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&chunk[..n]),
                    }
                }
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let answer = reply(index, &String::from_utf8_lossy(&head));
                let announced = answer.declared_len.unwrap_or(answer.body.len());
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {}\r\nContent-Length: {announced}\r\nConnection: close\r\n\r\n",
                        answer.status
                    )
                    .as_bytes(),
                );
                let _ = stream.write_all(&answer.body);
            }
        });
        HttpFixture {
            base: format!("http://{addr}"),
            requests,
            stop,
            addr,
            handle: Some(handle),
        }
    }

    /// `http://127.0.0.1:PORT` without a trailing slash.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// URL of `path` (which starts with `/`) on the fixture.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Requests received so far.
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            // Wake the blocking accept; the loop sees `stop` and ends.
            let _ = std::net::TcpStream::connect(self.addr);
            let _ = handle.join();
        }
    }
}

/// A loopback stand-in for the AniDB UDP server. `reply(index, request)` is the reply (without
/// tag) to the `index`-th packet, counted from 0; the fixture echoes the request tag like the
/// real server. An empty reply sends nothing, which models a server that does not answer.
///
/// The receive loop blocks without a read timeout: Windows documents a socket whose blocking
/// receive timed out as indeterminate, and a short polling timeout intermittently lost the one
/// request a test sends. `Drop` wakes the loop with an empty datagram instead.
pub struct UdpFixture {
    pub addr: std::net::SocketAddr,
    packets: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl UdpFixture {
    pub fn start(reply: impl Fn(usize, &str) -> String + Send + 'static) -> UdpFixture {
        UdpFixture::start_from(move |index, request, _| reply(index, request))
    }

    /// Like [`UdpFixture::start`], with the address the packet came from as the third
    /// argument: a reply can then report the client's own port (no NAT) or another one (a
    /// router that rewrites it), as `AUTH ... nat=1` does.
    pub fn start_from(
        reply: impl Fn(usize, &str, std::net::SocketAddr) -> String + Send + 'static,
    ) -> UdpFixture {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind loopback");
        let addr = socket.local_addr().expect("local address");
        let packets = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (counter, stopped) = (packets.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                let received = socket.recv_from(&mut buf);
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                // Errors such as a Windows connection reset from an earlier reply are reported
                // once per receive; the next receive is unaffected.
                let Ok((n, from)) = received else {
                    continue;
                };
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                let tag = request
                    .rsplit_once("&tag=")
                    .map(|(_, t)| t.trim().to_string())
                    .unwrap_or_default();
                let answer = reply(index, &request, from);
                if !answer.is_empty() {
                    let _ = socket.send_to(format!("{tag} {answer}").as_bytes(), from);
                }
            }
        });
        UdpFixture {
            addr,
            packets,
            stop,
            handle: Some(handle),
        }
    }

    /// Packets received so far.
    pub fn packets(&self) -> usize {
        self.packets.load(Ordering::SeqCst)
    }
}

impl Drop for UdpFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            if let Ok(waker) = std::net::UdpSocket::bind("127.0.0.1:0") {
                let _ = waker.send_to(&[], self.addr);
            }
            let _ = handle.join();
        }
    }
}
