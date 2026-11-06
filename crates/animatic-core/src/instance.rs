//! Single-instance handoff: a launch with media files gives them to the Animatic that already
//! has the same database open instead of opening a second window.
//!
//! The running instance (the *primary*) binds `127.0.0.1` on a random port and records the port
//! and a random token in a lock file next to the database (`<database file>.instance`, see
//! [`lock_path`]). A later launch reads the lock file, connects, sends the token and the files
//! as one line of JSON and waits for the acknowledgement; the primary queues the files for its
//! window ([`Incoming`]) and answers `ok`. Nothing listens on a non-loopback address and a
//! request without the token is refused.
//!
//! Nothing here can prevent a launch: any failure (unwritable directory, unreachable or
//! unresponsive primary, a path that is not Unicode) yields [`Launch::Standalone`] or a fresh
//! primary, so the application starts normally. A lock file whose port refuses connections or
//! that cannot be parsed is stale (left by a crash) and is replaced. Two launches share a lock
//! only when their canonical database paths are equal; another `--database` never forwards.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Longest request line the primary reads.
const MAX_REQUEST: u64 = 4 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
/// How long a client waits for the acknowledgement, and the primary for the request.
const IO_TIMEOUT: Duration = Duration::from_secs(3);
/// A lock file is created before its content is written; readers wait this long for it.
const LOCK_CONTENT_WAIT: Duration = Duration::from_millis(1000);

/// What [`start`] decided.
pub enum Launch {
    /// The files were given to the running instance; this process should exit.
    Forwarded,
    /// This process owns the lock; keep the [`Primary`] alive until the window is gone.
    Primary(Primary),
    /// Start normally without handoff: another instance holds the lock but there was nothing to
    /// forward (or forwarding failed), or the lock file could not be used.
    Standalone,
}

#[derive(Serialize, Deserialize)]
struct Lock {
    version: u32,
    port: u16,
    token: String,
    pid: u32,
    /// Canonical database path; a copied lock file for another database is never trusted.
    database: String,
}

#[derive(Serialize, Deserialize)]
struct Request {
    token: String,
    files: Vec<String>,
}

type Waker = Box<dyn Fn() + Send + Sync>;

/// Files forwarded by other launches, for the window to consume.
#[derive(Clone)]
pub struct Incoming {
    receiver: Receiver<Vec<PathBuf>>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Incoming {
    /// The next batch of forwarded files, if any arrived.
    pub fn try_recv(&self) -> Option<Vec<PathBuf>> {
        self.receiver.try_recv().ok()
    }

    /// `wake` runs on the listener thread whenever a batch arrives (the window uses it to
    /// request a repaint).
    pub fn set_waker(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self.waker.lock() = Some(Box::new(wake));
    }
}

/// The instance that owns the lock. Dropping it stops the listener and removes the lock file.
pub struct Primary {
    lock_path: PathBuf,
    token: String,
    addr: SocketAddr,
    incoming: Incoming,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Primary {
    pub fn incoming(&self) -> Incoming {
        self.incoming.clone()
    }

    /// Where the lock file is.
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}

impl Drop for Primary {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept.
        let _ = TcpStream::connect_timeout(&self.addr, CONNECT_TIMEOUT);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // Only a lock that is still ours: a replaced one belongs to someone else.
        if read_lock(&self.lock_path).is_some_and(|lock| lock.token == self.token) {
            let _ = fs::remove_file(&self.lock_path);
        }
    }
}

/// The database path with every link and spelling difference resolved. The directory is
/// created, as opening the database would do anyway.
fn canonical_database(db_path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(db_path)?;
    let name = absolute.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "database path has no file name",
        )
    })?;
    let parent = absolute.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let canonical = fs::canonicalize(parent)?.join(name);
    // An existing database may itself be a link or differ in case; resolve it completely.
    Ok(fs::canonicalize(&canonical).unwrap_or(canonical))
}

/// The lock file of `db_path`: the canonical database path with `.instance` appended.
pub fn lock_path(db_path: &Path) -> io::Result<PathBuf> {
    let mut lock = canonical_database(db_path)?.into_os_string();
    lock.push(".instance");
    Ok(PathBuf::from(lock))
}

/// Decides how this launch proceeds. With `files` and a live instance on the same database the
/// files are forwarded (made absolute first, since the other process has another working
/// directory) and [`Launch::Forwarded`] is returned. Otherwise this process claims the lock.
pub fn start(db_path: &Path, files: &[PathBuf]) -> Launch {
    let Ok(canonical) = canonical_database(db_path) else {
        return Launch::Standalone;
    };
    let database = canonical.to_string_lossy().into_owned();
    let mut lock_file = canonical.into_os_string();
    lock_file.push(".instance");
    let lock_path = PathBuf::from(lock_file);
    let absolute: Option<Vec<String>> = files
        .iter()
        .map(|file| {
            std::path::absolute(file)
                .ok()
                .and_then(|p| p.to_str().map(str::to_string))
        })
        .collect();

    for _ in 0..3 {
        match read_lock_waiting(&lock_path) {
            LockState::Missing => {}
            LockState::Unusable => remove_stale(&lock_path),
            LockState::Held(lock) if lock.database != database => remove_stale(&lock_path),
            LockState::Held(lock) => {
                let Some(absolute) = &absolute else {
                    return Launch::Standalone;
                };
                match forward(&lock, absolute) {
                    Ok(()) if absolute.is_empty() => return Launch::Standalone,
                    Ok(()) => return Launch::Forwarded,
                    Err(ForwardError::Unreachable) => remove_stale_if(&lock_path, &lock.token),
                    Err(ForwardError::Failed) => return Launch::Standalone,
                }
            }
        }
        match claim(&lock_path, &database) {
            Ok(primary) => return Launch::Primary(primary),
            // Another launch claimed it first; look at its lock.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                log::warn!("single-instance lock unavailable: {e}");
                return Launch::Standalone;
            }
        }
    }
    Launch::Standalone
}

enum LockState {
    Missing,
    /// Present but unreadable or malformed after waiting for a writer.
    Unusable,
    Held(Lock),
}

fn read_lock(path: &Path) -> Option<Lock> {
    let text = fs::read_to_string(path).ok()?;
    let lock: Lock = serde_json::from_str(&text).ok()?;
    (lock.version == 1).then_some(lock)
}

fn read_lock_waiting(path: &Path) -> LockState {
    let started = std::time::Instant::now();
    loop {
        match fs::metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return LockState::Missing,
            Err(_) => return LockState::Unusable,
            Ok(_) => {}
        }
        if let Some(lock) = read_lock(path) {
            return LockState::Held(lock);
        }
        // The claiming process creates the file, then writes it.
        if started.elapsed() >= LOCK_CONTENT_WAIT {
            return LockState::Unusable;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn remove_stale(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Removes the lock only while it still carries `token`, so a lock claimed meanwhile survives.
fn remove_stale_if(path: &Path, token: &str) {
    if read_lock(path).is_some_and(|lock| lock.token == token) {
        remove_stale(path);
    }
}

fn random_token() -> String {
    format!("{:032x}", rand::random::<u128>())
}

fn claim(lock_path: &Path, database: &str) -> io::Result<Primary> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
    let addr = listener.local_addr()?;
    let token = random_token();
    let lock = Lock {
        version: 1,
        port: addr.port(),
        token: token.clone(),
        pid: std::process::id(),
        database: database.to_string(),
    };
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(lock_path)?;
    let written = serde_json::to_vec(&lock)
        .map_err(io::Error::other)
        .and_then(|bytes| file.write_all(&bytes))
        .and_then(|()| file.flush());
    drop(file);
    if let Err(e) = written {
        let _ = fs::remove_file(lock_path);
        return Err(e);
    }

    let (sender, receiver) = crossbeam_channel::unbounded();
    let incoming = Incoming {
        receiver,
        waker: Arc::new(Mutex::new(None)),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let server = Server {
            token: token.clone(),
            sender,
            waker: incoming.waker.clone(),
        };
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("animatic-instance".into())
            .spawn(move || serve(listener, server, stop))
    };
    let thread = match thread {
        Ok(thread) => thread,
        Err(e) => {
            let _ = fs::remove_file(lock_path);
            return Err(e);
        }
    };
    Ok(Primary {
        lock_path: lock_path.to_path_buf(),
        token,
        addr,
        incoming,
        stop,
        thread: Some(thread),
    })
}

#[derive(Clone)]
struct Server {
    token: String,
    sender: Sender<Vec<PathBuf>>,
    waker: Arc<Mutex<Option<Waker>>>,
}

fn serve(listener: TcpListener, server: Server, stop: Arc<AtomicBool>) {
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else { continue };
        // One short-lived thread per connection: a stalled client cannot hold up the others.
        let server = server.clone();
        let _ = std::thread::Builder::new()
            .name("animatic-instance-conn".into())
            .spawn(move || {
                let _ = handle(stream, &server);
            });
    }
}

fn handle(mut stream: TcpStream, server: &Server) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut line = String::new();
    BufReader::new((&stream).take(MAX_REQUEST)).read_line(&mut line)?;
    let request: Option<Request> = serde_json::from_str(&line).ok();
    let accepted = request.filter(|r| same_token(&r.token, &server.token));
    let Some(request) = accepted else {
        return stream.write_all(b"denied\n");
    };
    let files: Vec<PathBuf> = request.files.into_iter().map(PathBuf::from).collect();
    if !files.is_empty()
        && server.sender.send(files).is_ok()
        && let Some(wake) = &*server.waker.lock()
    {
        wake();
    }
    stream.write_all(b"ok\n")
}

fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |diff, (x, y)| diff | (x ^ y))
            == 0
}

enum ForwardError {
    /// Nobody answers on the recorded port: the lock is stale.
    Unreachable,
    /// Something answered but the files were not accepted.
    Failed,
}

fn forward(lock: &Lock, files: &[String]) -> Result<(), ForwardError> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, lock.port));
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|_| ForwardError::Unreachable)?;
    let fail = |e: io::Error| {
        log::warn!("forwarding to the running instance failed: {e}");
        ForwardError::Failed
    };
    stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(fail)?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(fail)?;
    let mut request = serde_json::to_vec(&Request {
        token: lock.token.clone(),
        files: files.to_vec(),
    })
    .map_err(|e| fail(io::Error::other(e)))?;
    request.push(b'\n');
    stream.write_all(&request).map_err(fail)?;
    let mut reply = String::new();
    BufReader::new(&stream)
        .read_line(&mut reply)
        .map_err(fail)?;
    if reply.trim() == "ok" {
        Ok(())
    } else {
        Err(ForwardError::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn db(dir: &Path, name: &str) -> PathBuf {
        dir.join(name)
    }

    fn primary(launch: Launch) -> Primary {
        match launch {
            Launch::Primary(primary) => primary,
            Launch::Forwarded => panic!("expected a primary, files were forwarded"),
            Launch::Standalone => panic!("expected a primary, got standalone"),
        }
    }

    fn receive(incoming: &Incoming) -> Vec<PathBuf> {
        let started = Instant::now();
        loop {
            if let Some(files) = incoming.try_recv() {
                return files;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "nothing arrived"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn second_launch_forwards_its_files_to_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let database = db(dir.path(), "library.sqlite3");
        let first = primary(start(&database, &[]));
        let incoming = first.incoming();
        let woken = Arc::new(AtomicBool::new(false));
        let flag = woken.clone();
        incoming.set_waker(move || flag.store(true, Ordering::SeqCst));

        let media = dir.path().join("synthetic episode 01.mkv");
        assert!(matches!(
            start(&database, std::slice::from_ref(&media)),
            Launch::Forwarded
        ));
        assert_eq!(receive(&incoming), vec![media]);
        assert!(woken.load(Ordering::SeqCst));
    }

    #[test]
    fn relative_paths_are_forwarded_as_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let database = db(dir.path(), "library.sqlite3");
        let first = primary(start(&database, &[]));
        let incoming = first.incoming();

        assert!(matches!(
            start(&database, &[PathBuf::from("episode.mkv")]),
            Launch::Forwarded
        ));
        let received = receive(&incoming);
        assert!(received[0].is_absolute(), "{received:?}");
        assert!(received[0].ends_with("episode.mkv"));
    }

    #[test]
    fn another_database_starts_normally() {
        let dir = tempfile::tempdir().unwrap();
        let first = primary(start(&db(dir.path(), "one.sqlite3"), &[]));
        let media = [dir.path().join("a.mkv")];
        let second = primary(start(&db(dir.path(), "two.sqlite3"), &media));

        assert_ne!(first.lock_path(), second.lock_path());
        assert!(first.incoming().try_recv().is_none());
    }

    #[test]
    fn a_launch_without_files_never_takes_over_a_live_instance() {
        let dir = tempfile::tempdir().unwrap();
        let database = db(dir.path(), "library.sqlite3");
        let first = primary(start(&database, &[]));
        assert!(matches!(start(&database, &[]), Launch::Standalone));
        // The first instance still owns the lock.
        assert!(first.lock_path().is_file());
        assert!(matches!(
            start(&database, &[dir.path().join("a.mkv")]),
            Launch::Forwarded
        ));
    }

    #[test]
    fn the_same_database_under_another_spelling_is_the_same_instance() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        let first = primary(start(&db(dir.path(), "library.sqlite3"), &[]));
        let spelled = dir.path().join("sub").join("..").join("library.sqlite3");

        assert!(matches!(
            start(&spelled, &[dir.path().join("a.mkv")]),
            Launch::Forwarded
        ));
        assert_eq!(receive(&first.incoming()).len(), 1);
    }

    #[test]
    fn a_stale_lock_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let database = db(dir.path(), "library.sqlite3");
        let lock = lock_path(&database).unwrap();
        // A port nobody listens on any more.
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let stale = Lock {
            version: 1,
            port,
            token: "stale".into(),
            pid: 1,
            database: canonical_database(&database)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        };
        fs::write(&lock, serde_json::to_vec(&stale).unwrap()).unwrap();

        let media = [dir.path().join("a.mkv")];
        let fresh = primary(start(&database, &media));
        assert_eq!(read_lock(&lock).unwrap().token, fresh.token);
        drop(fresh);
        assert!(!lock.exists());
    }

    #[test]
    fn a_malformed_lock_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let database = db(dir.path(), "library.sqlite3");
        let lock = lock_path(&database).unwrap();
        fs::write(&lock, b"not json").unwrap();

        let started = Instant::now();
        let fresh = primary(start(&database, &[]));
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(read_lock(&lock).is_some());
        drop(fresh);
    }

    #[test]
    fn a_request_without_the_token_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let first = primary(start(&db(dir.path(), "library.sqlite3"), &[]));
        let lock = read_lock(first.lock_path()).unwrap();

        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, lock.port)).unwrap();
        stream
            .write_all(b"{\"token\":\"wrong\",\"files\":[\"/synthetic/a.mkv\"]}\n")
            .unwrap();
        let mut reply = String::new();
        BufReader::new(&stream).read_line(&mut reply).unwrap();
        assert_eq!(reply.trim(), "denied");
        assert!(first.incoming().try_recv().is_none());
    }

    #[test]
    fn the_listener_is_bound_to_loopback_only() {
        let dir = tempfile::tempdir().unwrap();
        let first = primary(start(&db(dir.path(), "library.sqlite3"), &[]));
        assert!(first.addr.ip().is_loopback());
    }

    #[test]
    fn dropping_the_primary_frees_the_lock_for_the_next_launch() {
        let dir = tempfile::tempdir().unwrap();
        let database = db(dir.path(), "library.sqlite3");
        let first = primary(start(&database, &[]));
        let lock = first.lock_path().to_path_buf();
        drop(first);
        assert!(!lock.exists());

        let media = [dir.path().join("a.mkv")];
        let second = primary(start(&database, &media));
        assert!(second.lock_path().is_file());
    }

    #[test]
    fn many_simultaneous_launches_end_with_one_primary() {
        let dir = tempfile::tempdir().unwrap();
        let database = db(dir.path(), "library.sqlite3");
        let handles: Vec<_> = (0..6)
            .map(|n| {
                let database = database.clone();
                let media = dir.path().join(format!("{n}.mkv"));
                std::thread::spawn(move || start(&database, &[media]))
            })
            .collect();
        let launches: Vec<Launch> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let primaries: Vec<&Primary> = launches
            .iter()
            .filter_map(|l| match l {
                Launch::Primary(p) => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(primaries.len(), 1);
        let forwarded = launches
            .iter()
            .filter(|l| matches!(l, Launch::Forwarded))
            .count();
        assert_eq!(forwarded, 5);
        let incoming = primaries[0].incoming();
        let mut received = 0;
        while received < 5 {
            received += receive(&incoming).len();
        }
        assert_eq!(received, 5);
    }
}
