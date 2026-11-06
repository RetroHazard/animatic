//! One-click install of a pinned FFmpeg build, keeping only `ffprobe` and its license.
//!
//! The user confirms first (the dialog shows [`Pin::size`], [`Pin::license`] and
//! [`Pin::page`]); nothing here runs unasked. The transport is separate from every AniDB
//! transport and from the update check: it makes plain anonymous `GET` requests (IP address and
//! an `Animatic/<version>` user agent are all the server learns), follows redirects manually and
//! only to the fixed GitHub hosts, bounds time and size, and never needs elevation: everything
//! goes into `<data folder>/tools/ffprobe/`.
//!
//! The expected SHA-256 and exact size are constants. The archive is streamed to a private
//! staging folder next to the destination, hashed on the way, and refused before it is opened
//! when the size or the digest differs. Only `<root>/bin/ffprobe[.exe]` and `<root>/LICENSE.txt`
//! are extracted (exact names, regular files, bounded sizes); every other entry is skipped
//! without being written. The staged files replace the installed ones last.
//!
//! # Updating the pin
//!
//! See "Media properties and the pinned FFmpeg build" in `docs/development.md`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use ureq::http::Uri;

use super::tool::{FILE_NAME, LICENSE_NAME};
use super::{MediaError, Result};

/// Archive container of a pinned build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    /// Windows builds.
    Zip,
    /// Linux builds (xz-compressed tar).
    TarXz,
}

impl ArchiveFormat {
    /// Name of the executable inside the archive.
    fn exe_entry(self) -> &'static str {
        match self {
            ArchiveFormat::Zip => "ffprobe.exe",
            ArchiveFormat::TarXz => "ffprobe",
        }
    }

    /// First bytes of a valid executable of the platform the container is built for.
    fn magic(self) -> &'static [u8] {
        match self {
            ArchiveFormat::Zip => b"MZ",
            ArchiveFormat::TarXz => b"\x7fELF",
        }
    }
}

/// One pinned download.
#[derive(Debug, Clone, Copy)]
pub struct Pin<'a> {
    /// FFmpeg release the build is made from, for example `9.0.2`.
    pub version: &'a str,
    /// Exact name of the build, shown to the user.
    pub build: &'a str,
    /// Exact URL of the archive.
    pub url: &'a str,
    /// Lowercase hex SHA-256 of the archive.
    pub sha256: &'a str,
    /// Exact size of the archive in bytes.
    pub size: u64,
    pub format: ArchiveFormat,
    /// Top-level folder inside the archive.
    pub root: &'a str,
    /// License of the build, shown before the download.
    pub license: &'a str,
    /// Human-readable page of the release the file belongs to, shown before the download.
    pub page: &'a str,
}

/// BtbN FFmpeg-Builds, Windows x86-64, LGPL, static, FFmpeg 9.0 branch. The release is the last
/// build of October 2026, which the project keeps for two years.
pub const WINDOWS_X64: Pin<'static> = Pin {
    version: "9.0.2",
    build: "n9.0.2-39-g7adfbe40dc (BtbN FFmpeg-Builds, win64, lgpl, static)",
    url: "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-31-12-59/ffmpeg-n9.0.2-39-g7adfbe40dc-win64-lgpl-9.0.zip",
    sha256: "9d37c564e0d01c18cde184a079fa6ef04d7d2783ada41da81d04165cc8f1a650",
    size: 168_715_328,
    format: ArchiveFormat::Zip,
    root: "ffmpeg-n9.0.2-39-g7adfbe40dc-win64-lgpl-9.0",
    license: "GNU Lesser General Public License, version 3 (FFmpeg LGPL build, no GPL components)",
    page: "https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-10-31-12-59",
};

/// BtbN FFmpeg-Builds, Linux x86-64, LGPL, static, FFmpeg 9.0 branch (same release as
/// [`WINDOWS_X64`]).
pub const LINUX_X64: Pin<'static> = Pin {
    version: "9.0.2",
    build: "n9.0.2-39-g7adfbe40dc (BtbN FFmpeg-Builds, linux64, lgpl, static)",
    url: "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-31-12-59/ffmpeg-n9.0.2-39-g7adfbe40dc-linux64-lgpl-9.0.tar.xz",
    sha256: "6046d6731b26f58887e953561551d254a6eec9fbca4100dfeab56e19af8328d5",
    size: 136_027_168,
    format: ArchiveFormat::TarXz,
    root: "ffmpeg-n9.0.2-39-g7adfbe40dc-linux64-lgpl-9.0",
    license: "GNU Lesser General Public License, version 3 (FFmpeg LGPL build, no GPL components)",
    page: "https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-10-31-12-59",
};

/// The pinned build for this platform; `None` where no build is pinned (only 64-bit Windows and
/// Linux have one).
pub fn current_pin() -> Option<&'static Pin<'static>> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some(&WINDOWS_X64)
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(&LINUX_X64)
    } else {
        None
    }
}

/// Hosts a request or a redirect may go to (exact match, HTTPS, default port, no user info).
const ALLOWED_HOSTS: [&str; 3] = [
    "github.com",
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
];
/// No pinned archive may be larger than this, whatever the constants say.
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_LICENSE_BYTES: u64 = 1024 * 1024;
/// Decompressed bytes read from an xz stream before giving up (the whole build is about 0.5 GiB).
const MAX_EXPANDED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
const MAX_REDIRECTS: usize = 5;
const CHUNK: usize = 64 * 1024;
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(30 * 60);

/// What an install put on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub exe: PathBuf,
    pub license: PathBuf,
}

/// Anonymous HTTP access for the pinned download.
pub struct Installer {
    agent: ureq::Agent,
    /// `host:port` of a loopback test server; `None` for the real hosts.
    loopback: Option<String>,
}

impl Installer {
    /// Honors proxy settings of the environment like the rest of the application.
    pub fn new() -> Self {
        Self::build(None)
    }

    fn build(loopback: Option<String>) -> Self {
        let mut builder = ureq::Agent::config_builder()
            .https_only(loopback.is_none())
            .max_redirects(0)
            .max_redirects_will_error(false)
            .http_status_as_error(false)
            .user_agent(format!("Animatic/{}", env!("CARGO_PKG_VERSION")))
            .accept_encoding("identity")
            .timeout_resolve(Some(Duration::from_secs(15)))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_send_request(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_per_read(Some(Duration::from_secs(30)));
        if loopback.is_some() {
            // Loopback fixtures never go through a proxy picked up from the environment.
            builder = builder.proxy(None);
        }
        Installer {
            agent: builder.build().into(),
            loopback,
        }
    }

    #[cfg(test)]
    fn loopback(origin: &str) -> Self {
        Self::build(Some(
            origin
                .strip_prefix("http://")
                .expect("loopback origin is http")
                .to_owned(),
        ))
    }

    /// Whether a request (or redirect target) may be sent to `url`.
    fn allows(&self, url: &str) -> bool {
        let Ok(uri) = url.parse::<Uri>() else {
            return false;
        };
        let (Some(scheme), Some(authority)) = (uri.scheme_str(), uri.authority()) else {
            return false;
        };
        if authority.as_str().contains('@') {
            return false;
        }
        if let Some(expected) = &self.loopback {
            return scheme == "http" && authority.as_str() == expected;
        }
        scheme == "https"
            && matches!(authority.port_u16(), None | Some(443))
            && ALLOWED_HOSTS
                .iter()
                .any(|host| authority.host().eq_ignore_ascii_case(host))
    }

    /// One GET with manual, allow-listed redirect handling. Only HTTP 200 returns `Ok`.
    fn get(&self, url: &str, cancel: &AtomicBool) -> Result<ureq::http::Response<ureq::Body>> {
        let mut current = url.to_owned();
        for _ in 0..=MAX_REDIRECTS {
            check_cancel(cancel)?;
            if !self.allows(&current) {
                return Err(MediaError::Redirect(
                    "the target is not an allowed GitHub HTTPS host".into(),
                ));
            }
            let response = self
                .agent
                .get(current.as_str())
                .header("Accept", "application/octet-stream")
                .call()
                .map_err(network_error)?;
            let status = response.status().as_u16();
            match status {
                200 => {
                    let encoding = response
                        .headers()
                        .get("content-encoding")
                        .and_then(|v| v.to_str().ok());
                    if encoding.is_some_and(|e| !e.eq_ignore_ascii_case("identity")) {
                        return Err(MediaError::Network(
                            "the server used an unexpected content encoding".into(),
                        ));
                    }
                    return Ok(response);
                }
                301 | 302 | 303 | 307 | 308 => {
                    current = response
                        .headers()
                        .get("location")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|loc| resolve_location(&current, loc))
                        .ok_or_else(|| {
                            MediaError::Redirect("a redirect has no usable target".into())
                        })?;
                }
                other => return Err(MediaError::Http(other)),
            }
        }
        Err(MediaError::Redirect("too many redirects".into()))
    }

    /// Stream the archive to `dest`, hashing on the way. Returns when size and SHA-256 match the
    /// pin; on any mismatch `dest` is removed and nothing was opened.
    fn download(
        &self,
        pin: &Pin<'_>,
        dest: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<()> {
        if pin.size > MAX_ARCHIVE_BYTES {
            return Err(MediaError::TooLarge {
                limit: MAX_ARCHIVE_BYTES,
            });
        }
        let response = self.get(pin.url, cancel)?;
        if let Some(declared) = response.body().content_length()
            && declared != pin.size
        {
            return Err(MediaError::Size {
                expected: pin.size,
                received: declared,
            });
        }
        let outcome = (|| {
            let mut reader = response.into_body().into_reader();
            let mut file = OpenOptions::new().write(true).create_new(true).open(dest)?;
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; CHUNK];
            let mut total = 0u64;
            let deadline = Instant::now() + DOWNLOAD_DEADLINE;
            loop {
                check_cancel(cancel)?;
                if Instant::now() > deadline {
                    return Err(MediaError::Network("the download took too long".into()));
                }
                let n = match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => {
                        return Err(MediaError::Network(format!(
                            "reading the download failed ({})",
                            e.kind()
                        )));
                    }
                };
                total += n as u64;
                if total > pin.size {
                    return Err(MediaError::Size {
                        expected: pin.size,
                        received: total,
                    });
                }
                hasher.update(&buf[..n]);
                file.write_all(&buf[..n])?;
                progress(total, pin.size);
            }
            file.sync_all()?;
            if total != pin.size {
                return Err(MediaError::Size {
                    expected: pin.size,
                    received: total,
                });
            }
            if !hex(&hasher.finalize()).eq_ignore_ascii_case(pin.sha256) {
                return Err(MediaError::Checksum);
            }
            Ok(())
        })();
        if outcome.is_err() {
            let _ = fs::remove_file(dest);
        }
        outcome
    }

    /// Download, verify and extract `pin`, then put `ffprobe` and its license into
    /// `install_dir` (replacing an earlier install). `progress` receives (bytes done, bytes
    /// total) while downloading.
    pub fn install(
        &self,
        pin: &Pin<'_>,
        install_dir: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Installed> {
        let tools = install_dir
            .parent()
            .ok_or_else(|| MediaError::Archive("the install folder has no parent".into()))?;
        fs::create_dir_all(tools)?;
        // Same volume as the destination, so the final moves are renames; removed on drop.
        let stage = tempfile::Builder::new()
            .prefix(".ffprobe-install-")
            .tempdir_in(tools)?;
        let archive = stage.path().join("archive");
        self.download(pin, &archive, cancel, progress)?;
        let exe = stage.path().join(FILE_NAME);
        let license = stage.path().join(LICENSE_NAME);
        extract(pin, &archive, &exe, &license, cancel)?;
        fs::remove_file(&archive)?;
        check_cancel(cancel)?;

        fs::create_dir_all(install_dir)?;
        let final_license = install_dir.join(LICENSE_NAME);
        let final_exe = install_dir.join(FILE_NAME);
        fs::rename(&license, &final_license)?;
        fs::rename(&exe, &final_exe)?;
        Ok(Installed {
            exe: final_exe,
            license: final_license,
        })
    }
}

impl Default for Installer {
    fn default() -> Self {
        Self::new()
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(MediaError::Cancelled)
    } else {
        Ok(())
    }
}

fn network_error(e: ureq::Error) -> MediaError {
    // Fixed texts only: the underlying error may mention proxy settings of the environment.
    let what = match &e {
        ureq::Error::Timeout(_) => "the request timed out".to_owned(),
        ureq::Error::HostNotFound => "the host name could not be resolved".to_owned(),
        ureq::Error::ConnectionFailed => "the connection failed".to_owned(),
        ureq::Error::Io(io) => format!("connection error ({})", io.kind()),
        ureq::Error::RequireHttpsOnly(_) => {
            return MediaError::Redirect("a non-HTTPS URL was refused".into());
        }
        _ => "the request failed".to_owned(),
    };
    MediaError::Network(what)
}

fn resolve_location(current: &str, location: &str) -> Option<String> {
    if location.starts_with("https://") || location.starts_with("http://") {
        return Some(location.to_owned());
    }
    if location.starts_with('/') && !location.starts_with("//") {
        let uri: Uri = current.parse().ok()?;
        return Some(format!(
            "{}://{}{}",
            uri.scheme_str()?,
            uri.authority()?.as_str(),
            location
        ));
    }
    None
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

// ---------------------------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------------------------

/// The two entries kept, by exact name in the archive.
struct Wanted {
    exe: String,
    license: String,
}

impl Wanted {
    fn new(pin: &Pin<'_>) -> Self {
        Wanted {
            exe: format!("{}/bin/{}", pin.root, pin.format.exe_entry()),
            license: format!("{}/LICENSE.txt", pin.root),
        }
    }
}

fn archive_error(text: impl Into<String>) -> MediaError {
    MediaError::Archive(text.into())
}

/// Write one entry to `dest` (never over an existing file), at most `limit` bytes.
fn write_entry(reader: &mut dyn Read, dest: &Path, limit: u64, executable: bool) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if executable { 0o755 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = executable;
    let mut file = options.open(dest)?;
    let copied = io::copy(&mut reader.take(limit + 1), &mut file)?;
    if copied > limit {
        return Err(MediaError::TooLarge { limit });
    }
    file.sync_all()?;
    Ok(())
}

/// The staged executable must start like an executable of the platform the build is for.
fn check_executable(pin: &Pin<'_>, path: &Path) -> Result<()> {
    let magic = pin.format.magic();
    let mut head = vec![0u8; magic.len()];
    File::open(path)?
        .read_exact(&mut head)
        .map_err(|_| archive_error("ffprobe is too short to be an executable"))?;
    if head != magic {
        return Err(archive_error(
            "ffprobe is not an executable of this platform",
        ));
    }
    Ok(())
}

fn extract(
    pin: &Pin<'_>,
    archive: &Path,
    exe: &Path,
    license: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    match pin.format {
        ArchiveFormat::Zip => extract_zip(pin, archive, exe, license, cancel)?,
        ArchiveFormat::TarXz => extract_tar_xz(pin, archive, exe, license, cancel)?,
    }
    check_executable(pin, exe)
}

fn extract_zip(
    pin: &Pin<'_>,
    archive: &Path,
    exe: &Path,
    license: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    let zip_error =
        |e: zip::result::ZipError| archive_error(format!("the zip archive is invalid ({e})"));
    let mut zip = zip::ZipArchive::new(File::open(archive)?).map_err(zip_error)?;
    if zip.len() > MAX_ENTRIES {
        return Err(archive_error("the archive has too many entries"));
    }
    if zip.has_duplicate_names() {
        return Err(archive_error("the archive has duplicate entries"));
    }
    let wanted = Wanted::new(pin);
    for (name, dest, limit, executable) in [
        (&wanted.license, license, MAX_LICENSE_BYTES, false),
        (&wanted.exe, exe, MAX_EXECUTABLE_BYTES, true),
    ] {
        check_cancel(cancel)?;
        let index = zip
            .index_for_name(name)
            .ok_or_else(|| archive_error(format!("{name} is missing")))?;
        let mut entry = zip.by_index(index).map_err(zip_error)?;
        let kind = entry.unix_mode().map_or(0, |m| m & 0o170000);
        if entry.is_dir() || entry.encrypted() || !matches!(kind, 0 | 0o100000) {
            return Err(archive_error(format!("{name} is not a regular file")));
        }
        if entry.size() > limit {
            return Err(MediaError::TooLarge { limit });
        }
        write_entry(&mut entry, dest, limit, executable)?;
    }
    Ok(())
}

/// Messages of the decoder thread behind [`XzStream`].
enum Chunk {
    Data(Vec<u8>),
    Failed(String),
}

/// `Write` end of the decoder thread: batches decoded bytes and hands them over bounded.
struct ChannelWriter {
    tx: crossbeam_channel::Sender<Chunk>,
    pending: Vec<u8>,
    total: u64,
}

impl ChannelWriter {
    fn send_pending(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let data = std::mem::replace(&mut self.pending, Vec::with_capacity(CHUNK));
        self.tx
            .send(Chunk::Data(data))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.total += buf.len() as u64;
        if self.total > MAX_EXPANDED_BYTES {
            return Err(io::Error::other("the archive expands to more than allowed"));
        }
        self.pending.extend_from_slice(buf);
        if self.pending.len() >= CHUNK {
            self.send_pending()?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send_pending()
    }
}

/// The decompressed bytes of an `.xz` file as a `Read`, decoded by a helper thread (the decoder
/// pushes into a writer; the tar reader pulls). Dropping it stops the thread.
struct XzStream<'a> {
    rx: Option<crossbeam_channel::Receiver<Chunk>>,
    current: Vec<u8>,
    position: usize,
    cancel: &'a AtomicBool,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl<'a> XzStream<'a> {
    fn start(file: File, cancel: &'a AtomicBool) -> Self {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let thread = std::thread::spawn(move || {
            let mut writer = ChannelWriter {
                tx: tx.clone(),
                pending: Vec::with_capacity(CHUNK),
                total: 0,
            };
            let result =
                lzma_rs::xz_decompress(&mut BufReader::with_capacity(CHUNK, file), &mut writer)
                    .map_err(|e| e.to_string())
                    .and_then(|()| writer.flush().map_err(|e| e.to_string()));
            if let Err(message) = result {
                let _ = tx.send(Chunk::Failed(message));
            }
        });
        XzStream {
            rx: Some(rx),
            current: Vec::new(),
            position: 0,
            cancel,
            thread: Some(thread),
        }
    }
}

impl Read for XzStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.position >= self.current.len() {
            if self.cancel.load(Ordering::Acquire) {
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            let Some(rx) = &self.rx else {
                return Ok(0);
            };
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Chunk::Data(data)) => {
                    self.current = data;
                    self.position = 0;
                }
                Ok(Chunk::Failed(message)) => {
                    self.rx = None;
                    return Err(io::Error::other(format!(
                        "the xz data is invalid ({message})"
                    )));
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
        let n = buf.len().min(self.current.len() - self.position);
        buf[..n].copy_from_slice(&self.current[self.position..self.position + n]);
        self.position += n;
        Ok(n)
    }
}

impl Drop for XzStream<'_> {
    fn drop(&mut self) {
        // The decoder stops at its next write once nobody receives.
        self.rx = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn extract_tar_xz(
    pin: &Pin<'_>,
    archive: &Path,
    exe: &Path,
    license: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    let wanted = Wanted::new(pin);
    let stream = XzStream::start(File::open(archive)?, cancel);
    let mut tar = tar::Archive::new(stream);
    let io_error = |e: io::Error| {
        if cancel.load(Ordering::Acquire) {
            MediaError::Cancelled
        } else {
            archive_error(format!("the archive cannot be read ({e})"))
        }
    };
    let (mut got_exe, mut got_license) = (false, false);
    let mut seen = 0usize;
    for entry in tar.entries().map_err(io_error)? {
        let mut entry = entry.map_err(io_error)?;
        seen += 1;
        if seen > MAX_ENTRIES {
            return Err(archive_error("the archive has too many entries"));
        }
        let name = entry.path_bytes().into_owned();
        let (dest, limit, executable, done) = if name == wanted.exe.as_bytes() {
            (exe, MAX_EXECUTABLE_BYTES, true, &mut got_exe)
        } else if name == wanted.license.as_bytes() {
            (license, MAX_LICENSE_BYTES, false, &mut got_license)
        } else {
            continue;
        };
        if *done {
            return Err(archive_error("the archive has duplicate entries"));
        }
        if !entry.header().entry_type().is_file() {
            return Err(archive_error(format!(
                "{} is not a regular file",
                String::from_utf8_lossy(&name)
            )));
        }
        if entry.size() > limit {
            return Err(MediaError::TooLarge { limit });
        }
        write_entry(&mut entry, dest, limit, executable).map_err(|e| match e {
            MediaError::Io(io) => io_error(io),
            other => other,
        })?;
        *done = true;
        if got_exe && got_license {
            // Everything needed was found; the digest of the whole file was verified already.
            break;
        }
    }
    if !got_exe {
        return Err(archive_error(format!("{} is missing", wanted.exe)));
    }
    if !got_license {
        return Err(archive_error(format!("{} is missing", wanted.license)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};

    const LICENSE_TEXT: &[u8] = b"synthetic license text\n";

    fn sha256_hex(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    // ---- loopback server --------------------------------------------------------------------

    struct Reply {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Reply {
        fn ok(body: Vec<u8>) -> Reply {
            Reply {
                status: 200,
                headers: Vec::new(),
                body,
            }
        }

        fn status(status: u16) -> Reply {
            Reply {
                status,
                headers: Vec::new(),
                body: Vec::new(),
            }
        }

        fn header(mut self, name: &str, value: &str) -> Reply {
            self.headers.push((name.into(), value.into()));
            self
        }
    }

    type Handler = Box<dyn Fn(&str) -> Reply + Send + Sync>;

    struct Server {
        origin: String,
        stop: Arc<AtomicBool>,
        requests: Arc<Mutex<Vec<String>>>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    fn serve_one(mut stream: TcpStream, handler: &Handler, log: &Mutex<Vec<String>>) {
        stream.set_nonblocking(false).ok();
        stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            match stream.read(&mut byte) {
                Ok(1) => head.push(byte[0]),
                _ => return,
            }
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        let path = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/")
            .to_owned();
        log.lock().unwrap().push(head);
        let reply = handler(&path);
        let mut text = format!(
            "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
            reply.status,
            reply.body.len()
        );
        for (name, value) in &reply.headers {
            text.push_str(&format!("{name}: {value}\r\n"));
        }
        text.push_str("\r\n");
        let _ = stream.write_all(text.as_bytes());
        let _ = stream.write_all(&reply.body);
        let _ = stream.flush();
    }

    impl Server {
        fn start(handler: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let stop = Arc::new(AtomicBool::new(false));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let handler: Handler = Box::new(handler);
            let (flag, log) = (stop.clone(), requests.clone());
            let thread = std::thread::spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => serve_one(stream, &handler, &log),
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            });
            Server {
                origin,
                stop,
                requests,
                thread: Some(thread),
            }
        }

        fn installer(&self) -> Installer {
            Installer::loopback(&self.origin)
        }

        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.origin)
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    // ---- synthetic archives -----------------------------------------------------------------

    const ROOT: &str = "ffmpeg-test-build";

    fn fake_exe(format: ArchiveFormat) -> Vec<u8> {
        let mut bytes = format.magic().to_vec();
        bytes.extend((0..4096u32).map(|i| (i % 251) as u8));
        bytes
    }

    /// The entries of a build: the two wanted files plus the neighbours of a real build.
    fn build_entries(format: ArchiveFormat) -> Vec<(String, Vec<u8>)> {
        vec![
            (format!("{ROOT}/bin/ffmpeg"), b"other tool".to_vec()),
            (format!("{ROOT}/bin/ffplay"), b"another tool".to_vec()),
            (
                format!("{ROOT}/bin/{}", format.exe_entry()),
                fake_exe(format),
            ),
            (format!("{ROOT}/LICENSE.txt"), LICENSE_TEXT.to_vec()),
            (format!("{ROOT}/doc/manual.html"), b"<html></html>".to_vec()),
            (format!("{ROOT}/presets/x.ffpreset"), b"preset".to_vec()),
        ]
    }

    fn zip_archive(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
        let mut out = io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut out);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(0o755);
            for (name, data) in entries {
                zip.start_file(name.as_str(), options).unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        out.into_inner()
    }

    fn tar_xz_archive(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_entry_type(tar::EntryType::Regular);
            builder
                .append_data(&mut header, name, data.as_slice())
                .unwrap();
        }
        let tar = builder.into_inner().unwrap();
        let mut xz = Vec::new();
        lzma_rs::xz_compress(&mut io::Cursor::new(tar), &mut xz).unwrap();
        xz
    }

    fn archive(format: ArchiveFormat, entries: &[(String, Vec<u8>)]) -> Vec<u8> {
        match format {
            ArchiveFormat::Zip => zip_archive(entries),
            ArchiveFormat::TarXz => tar_xz_archive(entries),
        }
    }

    const BOTH: [ArchiveFormat; 2] = [ArchiveFormat::Zip, ArchiveFormat::TarXz];

    /// Serves `body` at `/build` and returns the server with a pin for it.
    struct Fixture {
        server: Server,
        url: String,
        sha: String,
        size: u64,
        format: ArchiveFormat,
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new(format: ArchiveFormat, body: Vec<u8>) -> Fixture {
            let (sha, size) = (sha256_hex(&body), body.len() as u64);
            let server = Server::start(move |path| {
                if path == "/build" {
                    Reply::ok(body.clone())
                } else {
                    Reply::status(404)
                }
            });
            let url = server.url("/build");
            Fixture {
                server,
                url,
                sha,
                size,
                format,
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn pin(&self) -> Pin<'_> {
            Pin {
                version: "1.0",
                build: "test",
                url: &self.url,
                sha256: &self.sha,
                size: self.size,
                format: self.format,
                root: ROOT,
                license: "test license",
                page: "http://example.invalid/",
            }
        }

        fn install_dir(&self) -> PathBuf {
            tool_install_dir(self.dir.path())
        }

        fn install(&self, pin: &Pin<'_>) -> Result<Installed> {
            self.server.installer().install(
                pin,
                &self.install_dir(),
                &AtomicBool::new(false),
                &mut |_, _| {},
            )
        }

        fn tools_listing(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.dir.path().join("tools"))
                .map(|rd| {
                    rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        }
    }

    fn tool_install_dir(data: &Path) -> PathBuf {
        super::super::tool::install_dir(data)
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    // ---- tests ------------------------------------------------------------------------------

    #[test]
    fn install_keeps_only_ffprobe_and_the_license_and_cleans_up() {
        for format in BOTH {
            let fixture = Fixture::new(format, archive(format, &build_entries(format)));
            let mut seen = Vec::new();
            let installed = fixture
                .server
                .installer()
                .install(
                    &fixture.pin(),
                    &fixture.install_dir(),
                    &AtomicBool::new(false),
                    &mut |done, total| seen.push((done, total)),
                )
                .unwrap();
            assert_eq!(
                installed.exe,
                fixture.install_dir().join(FILE_NAME),
                "{format:?}"
            );
            assert_eq!(fs::read(&installed.exe).unwrap(), fake_exe(format));
            assert_eq!(fs::read(&installed.license).unwrap(), LICENSE_TEXT);
            let mut expected = vec![FILE_NAME.to_owned(), LICENSE_NAME.to_owned()];
            expected.sort();
            assert_eq!(listing(&fixture.install_dir()), expected, "{format:?}");
            // No staging folder is left behind next to the install folder.
            assert_eq!(fixture.tools_listing(), vec!["ffprobe".to_owned()]);
            assert_eq!(seen.last(), Some(&(fixture.size, fixture.size)));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(&installed.exe).unwrap().permissions().mode();
                assert_eq!(mode & 0o111, 0o111);
            }
        }
    }

    #[test]
    fn a_reinstall_replaces_the_previous_files() {
        let format = ArchiveFormat::Zip;
        let fixture = Fixture::new(format, archive(format, &build_entries(format)));
        fs::create_dir_all(fixture.install_dir()).unwrap();
        fs::write(fixture.install_dir().join(FILE_NAME), b"old").unwrap();
        fixture.install(&fixture.pin()).unwrap();
        assert_eq!(
            fs::read(fixture.install_dir().join(FILE_NAME)).unwrap(),
            fake_exe(format)
        );
    }

    #[test]
    fn a_sha256_mismatch_refuses_the_install_and_leaves_nothing_behind() {
        for format in BOTH {
            let fixture = Fixture::new(format, archive(format, &build_entries(format)));
            let wrong = sha256_hex(b"something else");
            let mut pin = fixture.pin();
            pin.sha256 = &wrong;
            let error = fixture.install(&pin).unwrap_err();
            assert!(matches!(error, MediaError::Checksum), "{format:?}: {error}");
            assert!(!fixture.install_dir().exists(), "{format:?}");
            assert_eq!(fixture.tools_listing(), Vec::<String>::new(), "{format:?}");
        }
    }

    #[test]
    fn a_mismatch_keeps_an_existing_install_untouched() {
        let format = ArchiveFormat::Zip;
        let fixture = Fixture::new(format, archive(format, &build_entries(format)));
        fs::create_dir_all(fixture.install_dir()).unwrap();
        fs::write(fixture.install_dir().join(FILE_NAME), b"working copy").unwrap();
        let wrong = "0".repeat(64);
        let mut pin = fixture.pin();
        pin.sha256 = &wrong;
        assert!(fixture.install(&pin).is_err());
        assert_eq!(
            fs::read(fixture.install_dir().join(FILE_NAME)).unwrap(),
            b"working copy"
        );
    }

    #[test]
    fn a_download_of_the_wrong_size_is_refused() {
        let format = ArchiveFormat::Zip;
        let body = archive(format, &build_entries(format));
        let fixture = Fixture::new(format, body);
        for size in [fixture.size - 1, fixture.size + 1] {
            let mut pin = fixture.pin();
            pin.size = size;
            let error = fixture.install(&pin).unwrap_err();
            assert!(matches!(error, MediaError::Size { .. }), "{size}: {error}");
            assert_eq!(fixture.tools_listing(), Vec::<String>::new());
        }
    }

    #[test]
    fn a_pin_above_the_hard_limit_is_refused_without_a_request() {
        let format = ArchiveFormat::Zip;
        let fixture = Fixture::new(format, archive(format, &build_entries(format)));
        let mut pin = fixture.pin();
        pin.size = MAX_ARCHIVE_BYTES + 1;
        assert!(matches!(
            fixture.install(&pin).unwrap_err(),
            MediaError::TooLarge { .. }
        ));
        assert!(fixture.server.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn archives_without_the_needed_files_are_refused() {
        for format in BOTH {
            for missing in [
                format!("{ROOT}/bin/{}", format.exe_entry()),
                format!("{ROOT}/LICENSE.txt"),
            ] {
                let entries: Vec<_> = build_entries(format)
                    .into_iter()
                    .filter(|(name, _)| *name != missing)
                    .collect();
                let fixture = Fixture::new(format, archive(format, &entries));
                let error = fixture.install(&fixture.pin()).unwrap_err();
                assert!(
                    matches!(error, MediaError::Archive(_)),
                    "{format:?} {missing}: {error}"
                );
                assert!(!fixture.install_dir().exists());
                assert_eq!(fixture.tools_listing(), Vec::<String>::new());
            }
        }
    }

    #[test]
    fn entries_with_other_names_are_not_taken_for_ffprobe() {
        for format in BOTH {
            let entries = vec![
                (
                    format!("other-root/bin/{}", format.exe_entry()),
                    fake_exe(format),
                ),
                (
                    format!("{ROOT}/bin/{}.bak", format.exe_entry()),
                    fake_exe(format),
                ),
                (format!("{ROOT}/LICENSE.txt"), LICENSE_TEXT.to_vec()),
            ];
            let fixture = Fixture::new(format, archive(format, &entries));
            assert!(matches!(
                fixture.install(&fixture.pin()).unwrap_err(),
                MediaError::Archive(_)
            ));
        }
    }

    #[test]
    fn an_executable_of_the_wrong_kind_is_refused() {
        for format in BOTH {
            let mut entries = build_entries(format);
            entries[2].1 = b"#!/bin/sh\necho not an executable of this platform\n".to_vec();
            let fixture = Fixture::new(format, archive(format, &entries));
            let error = fixture.install(&fixture.pin()).unwrap_err();
            assert!(
                matches!(error, MediaError::Archive(_)),
                "{format:?}: {error}"
            );
            assert!(!fixture.install_dir().exists());
        }
    }

    #[test]
    fn an_oversized_executable_entry_is_refused() {
        // A bounded writer and a declared size above the limit: tested on the helper the real
        // extraction uses, with a small limit.
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out");
        let data = vec![1u8; 100];
        let error = write_entry(&mut data.as_slice(), &dest, 99, false).unwrap_err();
        assert!(
            matches!(error, MediaError::TooLarge { limit: 99 }),
            "{error}"
        );
        let dest = dir.path().join("exact");
        write_entry(&mut data.as_slice(), &dest, 100, false).unwrap();
        assert_eq!(fs::read(&dest).unwrap().len(), 100);
        // Never over an existing file.
        assert!(write_entry(&mut data.as_slice(), &dest, 100, false).is_err());
    }

    #[test]
    fn data_that_is_not_xz_is_an_archive_error_even_when_its_digest_matches() {
        let good = tar_xz_archive(&build_entries(ArchiveFormat::TarXz));
        let mut bad_magic = good.clone();
        bad_magic[0] ^= 0xFF;
        for body in [
            bad_magic,
            good[..10].to_vec(),
            b"plain text, not xz".to_vec(),
        ] {
            let fixture = Fixture::new(ArchiveFormat::TarXz, body);
            let error = fixture.install(&fixture.pin()).unwrap_err();
            assert!(matches!(error, MediaError::Archive(_)), "{error}");
            assert_eq!(fixture.tools_listing(), Vec::<String>::new());
        }
    }

    #[test]
    fn requests_are_anonymous_and_identify_only_the_application() {
        let format = ArchiveFormat::Zip;
        let fixture = Fixture::new(format, archive(format, &build_entries(format)));
        fixture.install(&fixture.pin()).unwrap();
        let requests = fixture.server.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        let head = requests[0].to_ascii_lowercase();
        assert!(head.starts_with("get /build "), "{head}");
        assert!(head.contains(&format!(
            "user-agent: animatic/{}",
            env!("CARGO_PKG_VERSION")
        )));
        for forbidden in ["authorization", "cookie", "referer", "x-"] {
            assert!(
                !head.contains(&format!("\n{forbidden}")),
                "{forbidden}: {head}"
            );
        }
    }

    #[test]
    fn redirects_are_followed_only_to_allowed_hosts() {
        let format = ArchiveFormat::Zip;
        let body = archive(format, &build_entries(format));
        let (sha, size) = (sha256_hex(&body), body.len() as u64);
        let served = body.clone();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let server = Server::start(move |path| match path {
            "/start" => Reply::status(302).header("Location", "/final"),
            "/final" => {
                counter.fetch_add(1, Ordering::SeqCst);
                Reply::ok(served.clone())
            }
            "/foreign" => Reply::status(302).header("Location", "https://evil.example/x"),
            "/plain" => Reply::status(302).header("Location", "http://127.0.0.1:1/x"),
            _ => Reply::status(404),
        });
        let dir = tempfile::tempdir().unwrap();
        let install_dir = tool_install_dir(dir.path());
        let url = |path: &str| server.url(path);
        let run = |start: String| {
            let pin = Pin {
                version: "1",
                build: "t",
                url: &start,
                sha256: &sha,
                size,
                format,
                root: ROOT,
                license: "l",
                page: "p",
            };
            server
                .installer()
                .install(&pin, &install_dir, &AtomicBool::new(false), &mut |_, _| {})
        };
        run(url("/start")).unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        for bad in ["/foreign", "/plain"] {
            let error = run(url(bad)).unwrap_err();
            assert!(matches!(error, MediaError::Redirect(_)), "{bad}: {error}");
        }
    }

    #[test]
    fn the_real_allow_list_is_exact() {
        let installer = Installer::new();
        for ok in [
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/x/y.zip",
            "https://release-assets.githubusercontent.com/github-production-release-asset/1",
            "https://objects.githubusercontent.com/x",
            "https://GITHUB.com/x",
        ] {
            assert!(installer.allows(ok), "{ok}");
        }
        for bad in [
            "http://github.com/x",
            "https://github.com.evil.example/x",
            "https://evil.example/github.com",
            "https://user@github.com/x",
            "https://github.com:8443/x",
            "https://gist.github.com/x",
            "ftp://github.com/x",
            "//github.com/x",
            "/relative",
        ] {
            assert!(!installer.allows(bad), "{bad}");
        }
        // Both pinned URLs are inside the list.
        assert!(installer.allows(WINDOWS_X64.url));
        assert!(installer.allows(LINUX_X64.url));
    }

    #[test]
    fn http_errors_and_cancellation_leave_nothing_behind() {
        let format = ArchiveFormat::Zip;
        let fixture = Fixture::new(format, archive(format, &build_entries(format)));
        let mut pin = fixture.pin();
        let missing = fixture.server.url("/missing");
        pin.url = &missing;
        assert!(matches!(
            fixture.install(&pin).unwrap_err(),
            MediaError::Http(404)
        ));
        assert_eq!(fixture.tools_listing(), Vec::<String>::new());

        let cancel = AtomicBool::new(true);
        let error = fixture
            .server
            .installer()
            .install(
                &fixture.pin(),
                &fixture.install_dir(),
                &cancel,
                &mut |_, _| {},
            )
            .unwrap_err();
        assert!(matches!(error, MediaError::Cancelled), "{error}");
        assert_eq!(fixture.tools_listing(), Vec::<String>::new());
    }

    #[test]
    fn the_pinned_constants_are_well_formed() {
        for pin in [&WINDOWS_X64, &LINUX_X64] {
            assert_eq!(pin.sha256.len(), 64);
            assert!(
                pin.sha256
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            );
            assert!(pin.size > 0 && pin.size <= MAX_ARCHIVE_BYTES);
            assert!(
                pin.url.starts_with(
                    "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-"
                )
            );
            assert!(pin.url.ends_with(match pin.format {
                ArchiveFormat::Zip => ".zip",
                ArchiveFormat::TarXz => ".tar.xz",
            }));
            assert!(
                pin.url.contains(pin.root),
                "the asset is named after its root folder"
            );
            assert!(
                pin.url.contains("-lgpl-"),
                "the pinned build is an LGPL build"
            );
            assert!(!pin.url.contains("gpl-shared") && !pin.url.contains("-gpl"));
            assert!(
                pin.page
                    .starts_with("https://github.com/BtbN/FFmpeg-Builds/releases/tag/")
            );
        }
        assert_eq!(
            current_pin().is_some(),
            cfg!(all(
                target_arch = "x86_64",
                any(windows, target_os = "linux")
            ))
        );
    }
}
