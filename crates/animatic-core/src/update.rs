//! GitHub release checker, downloader and installer.
//!
//! The module answers one question for the application: is there a newer *stable, published*
//! release of `RetroHazard/animatic` for this platform, and if so can it be downloaded,
//! verified and installed in place? It never acts on its own: the application drives the three
//! steps ([`UpdateClient::check`], [`UpdateClient::download`], [`VerifiedUpdate::install`]) after
//! the user approved each one.
//!
//! # Trust model
//!
//! * Only `https` requests to `api.github.com` and `github.com` are made, redirects are
//!   followed manually and only to the fixed CDN hosts of GitHub. No credentials or tokens
//!   are ever sent; GitHub learns the user's IP address and the `Animatic/<version>` user
//!   agent. No library data is sent.
//! * A 404 from GitHub is reported as [`UpdateError::NotFound`] (the releases are not
//!   available or not accessible), never as "up to date"; an accessible but empty release list
//!   means there is no update.
//! * The archive is checked against the `SHA256SUMS` asset of the same release (and GitHub's
//!   own `digest` field when present). This proves that the download matches what the release
//!   lists; it does **not** authenticate the publisher: the archives are unsigned.
//! * Every download and every extraction is size-bounded (see the `MAX_*` constants), the
//!   archive is verified before it is opened, and only the exact executable, `LICENSE` and
//!   `THIRD-PARTY-NOTICES.html` are extracted. Any other entry must be a known bundled
//!   document and is only validated.
//!
//! # Installation
//!
//! [`VerifiedUpdate::install`] first writes the new `LICENSE-v<version>` and
//! `THIRD-PARTY-NOTICES-v<version>.html` next to the executable (additive, never overwriting a
//! different file), then replaces the executable with the maintained `self-replace` crate.
//! Concurrent installers are serialized with an advisory lock on the reserved, empty
//! `.animatic-update.lock` file in the installation folder; the file's content is never read
//! or written and it is harmless to leave behind. The running executable's identity (path and
//! OS file identity) is captured at startup by [`InstallationTarget::capture`] and revalidated
//! under the lock, so a second instance that started from an executable which has since been
//! replaced refuses to install.
//!
//! `self-replace` is not transactional on Windows (it moves the old executable aside before
//! the new one is in place). On Windows a recovery copy of the current executable is therefore
//! created next to it first; a failed replacement restores the original name (never
//! overwriting something another writer created) and always keeps that recovery copy. Once the
//! replacement has been entered, [`UpdateError::install_attempted`] reports `true`: the caller
//! must not retry or restart on its own. Power-loss atomicity is not promised.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use ureq::http::Uri;

const REPOSITORY: &str = "RetroHazard/animatic";
const API_ORIGIN: &str = "https://api.github.com";
const WEB_ORIGIN: &str = "https://github.com";
/// Hosts a redirect may lead to (exact match, HTTPS, default port, no user info).
const REDIRECT_HOSTS: [&str; 4] = [
    "github.com",
    "api.github.com",
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
];
const CHECKSUMS_NAME: &str = "SHA256SUMS";
const LOCK_NAME: &str = ".animatic-update.lock";

const MAX_METADATA_BYTES: u64 = 1024 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 64 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_LICENSE_BYTES: u64 = 8 * 1024 * 1024;
/// Sum of the declared sizes of all entries of one archive (executable plus bundled documents).
const MAX_EXPANDED_BYTES: u64 = 640 * 1024 * 1024;
/// Slack for tar headers and padding on top of [`MAX_EXPANDED_BYTES`] in the decompressed stream.
const STREAM_OVERHEAD: u64 = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 128;
const RELEASES_PER_PAGE: usize = 30;
const MAX_RELEASE_PAGES: usize = 100;
const MAX_REDIRECTS: usize = 5;
const CHUNK: usize = 64 * 1024;
const METADATA_DEADLINE: Duration = Duration::from_secs(60);
const CHECKSUMS_DEADLINE: Duration = Duration::from_secs(60);
const ARCHIVE_DEADLINE: Duration = Duration::from_secs(30 * 60);

/// Result of every operation of this module.
pub type Result<T, E = UpdateError> = std::result::Result<T, E>;

/// Why an update operation failed. Independent of the AniDB error types: nothing here affects
/// the AniDB cooldown or queue.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("the update was cancelled")]
    Cancelled,
    #[error("updates are not available for this platform")]
    UnsupportedPlatform,
    #[error("invalid version: {0}")]
    InvalidVersion(String),
    #[error("network error: {0}")]
    Network(String),
    #[error(
        "GitHub refused the request (HTTP {0}); this is usually the unauthenticated rate limit, try again later"
    )]
    RateLimited(u16),
    #[error("GitHub answered with HTTP status {0}")]
    Http(u16),
    /// The release feed is unavailable or not accessible; never treated as "up to date".
    #[error(
        "GitHub reports that the release data is not available (HTTP 404): the release feed was not found or is not accessible"
    )]
    NotFound,
    #[error("refused a redirect: {0}")]
    Redirect(String),
    #[error("invalid release metadata: {0}")]
    Metadata(String),
    /// The highest stable release exists but cannot be installed; older releases are never
    /// offered instead.
    #[error("the newest release {tag} cannot be installed: {reason}")]
    IncompleteRelease { tag: String, reason: String },
    #[error("{0}")]
    Limit(String),
    #[error("{what} is larger than the allowed {limit} bytes")]
    TooLarge { what: &'static str, limit: u64 },
    #[error(
        "the download of {what} was truncated or has an unexpected size (expected {expected} bytes, received {received})"
    )]
    Truncated {
        what: &'static str,
        expected: u64,
        received: u64,
    },
    #[error("checksum verification failed: {0}")]
    Checksum(String),
    #[error("invalid update archive: {0}")]
    Archive(String),
    #[error("installation refused: {0}")]
    Refused(String),
    #[error("another Animatic installer is already running for this installation")]
    Busy,
    #[error(
        "the installation location is not writable ({0}); update manually from the release page"
    )]
    NotWritable(String),
    /// The OS replacement of the executable was entered and failed. The process must not retry
    /// or restart by itself; `recovery` explains the state of the executable.
    #[error("replacing the executable failed: {cause}. {recovery}")]
    Replace {
        cause: String,
        recovery: String,
        backup: Option<PathBuf>,
    },
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

impl UpdateError {
    /// `true` once the replacement of the executable was entered. After such an error the
    /// caller must not retry the installation and must not restart the application on its own.
    pub fn install_attempted(&self) -> bool {
        matches!(self, UpdateError::Replace { .. })
    }
}

/// Platforms an update can be installed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
}

impl Platform {
    /// The platform this build runs on; `None` when no release archive exists for it.
    pub fn current() -> Option<Self> {
        if cfg!(all(windows, target_arch = "x86_64")) {
            Some(Platform::Windows)
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Some(Platform::Linux)
        } else {
            None
        }
    }

    fn label(self) -> &'static str {
        match self {
            Platform::Windows => "windows",
            Platform::Linux => "linux",
        }
    }

    fn stem(self, version: &str) -> String {
        format!("animatic-v{version}-{}-x86_64", self.label())
    }

    fn archive_name(self, version: &str) -> String {
        let extension = match self {
            Platform::Windows => "zip",
            Platform::Linux => "tar.gz",
        };
        format!("{}.{extension}", self.stem(version))
    }

    fn exe_name(self) -> &'static str {
        match self {
            Platform::Windows => "animatic.exe",
            Platform::Linux => "animatic",
        }
    }
}

#[derive(Clone, Debug)]
struct AssetRef {
    name: String,
    /// Canonical browser download URL.
    url: String,
    size: u64,
    /// GitHub's `sha256:` digest of the asset, when the API listed one.
    sha256: Option<String>,
}

/// A newer stable release whose assets passed metadata validation.
#[derive(Clone, Debug)]
pub struct ReleaseInfo {
    pub version: String,
    pub tag: String,
    pub page_url: String,
    pub archive_size: u64,
    platform: Platform,
    archive: AssetRef,
    checksums: AssetRef,
}

// ---------------------------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------------------------

/// Parses `YYYY.M.PATCH[-prerelease]`: year 2000..=9999, month 1..=12, SemVer/Cargo grammar
/// (no leading zeros), no build metadata.
fn parse_calver(text: &str) -> std::result::Result<semver::Version, String> {
    let version = semver::Version::parse(text)
        .map_err(|e| format!("{text:?} is not a valid version ({e})"))?;
    if !version.build.is_empty() {
        return Err(format!("{text:?} must not carry build metadata"));
    }
    if !(2000..=9999).contains(&version.major) || !(1..=12).contains(&version.minor) {
        return Err(format!("{text:?} is not a YYYY.M.PATCH calendar version"));
    }
    Ok(version)
}

fn parse_tag(tag: &str) -> std::result::Result<semver::Version, String> {
    let rest = tag
        .strip_prefix('v')
        .ok_or_else(|| format!("tag {tag:?} does not start with 'v'"))?;
    parse_calver(rest)
}

// ---------------------------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
struct Endpoints {
    api: String,
    web: String,
    /// `host:port` of a loopback test server; `None` for the real GitHub endpoints.
    loopback: Option<String>,
}

impl Endpoints {
    fn github() -> Self {
        Endpoints {
            api: API_ORIGIN.to_owned(),
            web: WEB_ORIGIN.to_owned(),
            loopback: None,
        }
    }

    #[cfg(test)]
    fn loopback(origin: &str) -> Self {
        Endpoints {
            api: origin.to_owned(),
            web: origin.to_owned(),
            loopback: Some(
                origin
                    .strip_prefix("http://")
                    .expect("loopback origin is http")
                    .to_owned(),
            ),
        }
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
            && REDIRECT_HOSTS
                .iter()
                .any(|host| authority.host().eq_ignore_ascii_case(host))
    }
}

fn network_error(e: ureq::Error) -> UpdateError {
    // Fixed texts only: the underlying error may mention proxy settings of the environment.
    let what = match &e {
        ureq::Error::Timeout(_) => "the request timed out".to_owned(),
        ureq::Error::HostNotFound => "the host name could not be resolved".to_owned(),
        ureq::Error::ConnectionFailed => "the connection failed".to_owned(),
        ureq::Error::Io(io) => format!("connection error ({})", io.kind()),
        ureq::Error::RequireHttpsOnly(_) => {
            return UpdateError::Redirect("a non-HTTPS URL was refused".to_owned());
        }
        _ => "the request failed".to_owned(),
    };
    UpdateError::Network(what)
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(UpdateError::Cancelled)
    } else {
        Ok(())
    }
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

/// Streams a response body into `sink`, enforcing the byte limit, an optional exact length,
/// cancellation (checked per chunk) and a wall-clock deadline. Returns the byte count.
fn stream_body(
    response: ureq::http::Response<ureq::Body>,
    limit: u64,
    exact: Option<u64>,
    what: &'static str,
    cancel: &AtomicBool,
    deadline: Instant,
    sink: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<u64> {
    let declared = response.body().content_length();
    if declared.is_some_and(|n| n > limit) {
        return Err(UpdateError::TooLarge { what, limit });
    }
    let mut reader = response.into_body().into_reader();
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        check_cancel(cancel)?;
        if Instant::now() > deadline {
            return Err(UpdateError::Network(format!(
                "the download of {what} took too long"
            )));
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(UpdateError::Truncated {
                    what,
                    expected: exact.or(declared).unwrap_or(0),
                    received: total,
                });
            }
            Err(e) => {
                return Err(UpdateError::Network(format!(
                    "reading {what} failed ({})",
                    e.kind()
                )));
            }
        };
        total += n as u64;
        if total > limit {
            return Err(UpdateError::TooLarge { what, limit });
        }
        sink(&buf[..n])?;
    }
    if let Some(expected) = exact.or(declared)
        && total != expected
    {
        return Err(UpdateError::Truncated {
            what,
            expected,
            received: total,
        });
    }
    Ok(total)
}

// ---------------------------------------------------------------------------------------------
// Release listing
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Debug, Deserialize)]
struct ApiAsset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn parse_digest(digest: Option<&str>) -> std::result::Result<Option<String>, String> {
    let Some(digest) = digest else {
        return Ok(None);
    };
    match digest.strip_prefix("sha256:") {
        Some(hex) if is_sha256_hex(hex) => Ok(Some(hex.to_ascii_lowercase())),
        Some(_) => Err("an asset lists a malformed sha256 digest".to_owned()),
        // Other algorithms are not used.
        None => Ok(None),
    }
}

fn find_asset(
    endpoints: &Endpoints,
    tag: &str,
    assets: &[ApiAsset],
    name: &str,
    limit: u64,
) -> std::result::Result<AssetRef, String> {
    let mut matching = assets.iter().filter(|a| a.name == name);
    let asset = matching
        .next()
        .ok_or_else(|| format!("the asset {name} is missing"))?;
    if matching.next().is_some() {
        return Err(format!("the asset {name} is listed more than once"));
    }
    if asset.state != "uploaded" {
        return Err(format!("the asset {name} is not fully uploaded"));
    }
    if asset.size == 0 || asset.size > limit {
        return Err(format!(
            "the asset {name} has an unacceptable size ({} bytes)",
            asset.size
        ));
    }
    let url = format!(
        "{}/{REPOSITORY}/releases/download/{tag}/{name}",
        endpoints.web
    );
    if asset.browser_download_url != url {
        return Err(format!(
            "the asset {name} does not point at the project repository"
        ));
    }
    Ok(AssetRef {
        name: name.to_owned(),
        url,
        size: asset.size,
        sha256: parse_digest(asset.digest.as_deref())?,
    })
}

fn release_info(
    endpoints: &Endpoints,
    platform: Platform,
    version: &semver::Version,
    release: &ApiRelease,
) -> std::result::Result<ReleaseInfo, String> {
    let tag = release.tag_name.as_str();
    let version = version.to_string();
    let page_url = format!("{}/{REPOSITORY}/releases/tag/{tag}", endpoints.web);
    if release.html_url.as_deref() != Some(page_url.as_str()) {
        return Err("the release page does not point at the project repository".to_owned());
    }
    let archive = find_asset(
        endpoints,
        tag,
        &release.assets,
        &platform.archive_name(&version),
        MAX_ARCHIVE_BYTES,
    )?;
    let checksums = find_asset(
        endpoints,
        tag,
        &release.assets,
        CHECKSUMS_NAME,
        MAX_CHECKSUM_BYTES,
    )?;
    Ok(ReleaseInfo {
        version,
        tag: tag.to_owned(),
        page_url,
        archive_size: archive.size,
        platform,
        archive,
        checksums,
    })
}

/// Checksum file: lines of `<64 lowercase hex>  <file name>`. Returns the hash of `name`.
fn parse_checksums(bytes: &[u8], name: &str) -> Result<String> {
    let bad = |why: &str| UpdateError::Checksum(format!("SHA256SUMS is invalid: {why}"));
    let text = std::str::from_utf8(bytes).map_err(|_| bad("not UTF-8 text"))?;
    if text.contains('\r') {
        return Err(bad("unexpected carriage return"));
    }
    let mut names = HashSet::new();
    let mut found = None;
    for line in text.lines() {
        let (hash, file) = line
            .split_once("  ")
            .ok_or_else(|| bad("a line is not '<sha256>  <file>'"))?;
        let hash_ok = hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hash_ok {
            return Err(bad("a hash is not 64 lowercase hexadecimal digits"));
        }
        if file.is_empty() || file.starts_with(' ') || file.contains(['/', '\\']) {
            return Err(bad("a file name is not valid"));
        }
        if !names.insert(file) {
            return Err(bad("a file is listed twice"));
        }
        if file == name {
            found = Some(hash.to_owned());
        }
    }
    found.ok_or_else(|| bad("the archive is not listed"))
}

// ---------------------------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------------------------

/// HTTP access to the GitHub releases of the project. Cheap to create; holds a connection pool.
pub struct UpdateClient {
    agent: ureq::Agent,
    endpoints: Endpoints,
}

impl UpdateClient {
    /// Client for the public repository `https://github.com/RetroHazard/animatic`; all
    /// requests are anonymous. Honors proxy settings of the environment like the rest of the
    /// application.
    pub fn new() -> Result<Self> {
        Self::with_endpoints(Endpoints::github())
    }

    fn with_endpoints(endpoints: Endpoints) -> Result<Self> {
        let loopback = endpoints.loopback.is_some();
        let mut builder = ureq::Agent::config_builder()
            .https_only(!loopback)
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
        if loopback {
            // Loopback fixtures never go through a proxy picked up from the environment.
            builder = builder.proxy(None);
        }
        let agent: ureq::Agent = builder.build().into();
        Ok(UpdateClient { agent, endpoints })
    }

    /// One GET with manual, allow-listed redirect handling. Only HTTP 200 returns `Ok`.
    fn get(
        &self,
        url: &str,
        accept: &str,
        cancel: &AtomicBool,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let mut current = url.to_owned();
        for _ in 0..=MAX_REDIRECTS {
            check_cancel(cancel)?;
            if !self.endpoints.allows(&current) {
                return Err(UpdateError::Redirect(
                    "the target is not an allowed GitHub HTTPS host".to_owned(),
                ));
            }
            let response = self
                .agent
                .get(current.as_str())
                .header("Accept", accept)
                .header("X-GitHub-Api-Version", "2022-11-28")
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
                        return Err(UpdateError::Network(
                            "the server used an unexpected content encoding".to_owned(),
                        ));
                    }
                    return Ok(response);
                }
                301 | 302 | 303 | 307 | 308 => {
                    let location = response
                        .headers()
                        .get("location")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|loc| resolve_location(&current, loc))
                        .ok_or_else(|| {
                            UpdateError::Redirect("a redirect has no usable target".to_owned())
                        })?;
                    current = location;
                }
                403 | 429 => return Err(UpdateError::RateLimited(status)),
                404 => return Err(UpdateError::NotFound),
                other => return Err(UpdateError::Http(other)),
            }
        }
        Err(UpdateError::Redirect("too many redirects".to_owned()))
    }

    fn fetch_release_page(
        &self,
        url: &str,
        cancel: &AtomicBool,
    ) -> Result<(Vec<ApiRelease>, bool)> {
        let response = self.get(url, "application/vnd.github+json", cancel)?;
        let has_next = response
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|link| link.split(',').any(|part| part.contains("rel=\"next\"")));
        let mut body = Vec::new();
        stream_body(
            response,
            MAX_METADATA_BYTES,
            None,
            "the release metadata",
            cancel,
            Instant::now() + METADATA_DEADLINE,
            &mut |chunk| {
                body.extend_from_slice(chunk);
                Ok(())
            },
        )?;
        let releases = serde_json::from_slice(&body).map_err(|e| {
            UpdateError::Metadata(format!("GitHub returned malformed release data ({e})"))
        })?;
        Ok((releases, has_next))
    }

    /// Looks for a stable, published release newer than `current_version` for `platform`.
    ///
    /// All release pages are read (bounded) and the highest stable CalVer wins regardless of
    /// the order GitHub lists them in. `Ok(None)` means no newer compatible release exists. If
    /// the highest release is newer but incomplete or invalid, that is an error: an older
    /// release is never offered instead.
    pub fn check(
        &self,
        current_version: &str,
        platform: Platform,
        cancel: &AtomicBool,
    ) -> Result<Option<ReleaseInfo>> {
        let current = parse_calver(current_version).map_err(UpdateError::InvalidVersion)?;
        let mut best: Option<(semver::Version, ApiRelease)> = None;
        let mut complete = false;
        for page in 1..=MAX_RELEASE_PAGES {
            let url = format!(
                "{}/repos/{REPOSITORY}/releases?per_page={RELEASES_PER_PAGE}&page={page}",
                self.endpoints.api
            );
            let (releases, has_next) = self.fetch_release_page(&url, cancel)?;
            let count = releases.len();
            for release in releases {
                if release.draft || release.prerelease {
                    continue;
                }
                let Ok(version) = parse_tag(&release.tag_name) else {
                    continue;
                };
                if !version.pre.is_empty() {
                    continue;
                }
                if best.as_ref().is_some_and(|(b, _)| *b >= version) {
                    continue;
                }
                best = Some((version, release));
            }
            if count == 0 || (!has_next && count < RELEASES_PER_PAGE) {
                complete = true;
                break;
            }
        }
        if !complete {
            return Err(UpdateError::Limit(format!(
                "the release list has more than {MAX_RELEASE_PAGES} pages; giving up"
            )));
        }
        let Some((version, release)) = best else {
            return Ok(None);
        };
        if version <= current {
            return Ok(None);
        }
        release_info(&self.endpoints, platform, &version, &release)
            .map(Some)
            .map_err(|reason| UpdateError::IncompleteRelease {
                tag: release.tag_name.clone(),
                reason,
            })
    }

    fn fetch_checksums(&self, asset: &AssetRef, cancel: &AtomicBool) -> Result<Vec<u8>> {
        let response = self.get(&asset.url, "application/octet-stream", cancel)?;
        let mut body = Vec::new();
        stream_body(
            response,
            asset.size,
            Some(asset.size),
            "the checksum list",
            cancel,
            Instant::now() + CHECKSUMS_DEADLINE,
            &mut |chunk| {
                body.extend_from_slice(chunk);
                Ok(())
            },
        )?;
        Ok(body)
    }

    /// Streams the archive to `dest`, returning its SHA-256.
    fn fetch_archive(
        &self,
        asset: &AssetRef,
        dest: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<String> {
        let response = self.get(&asset.url, "application/octet-stream", cancel)?;
        let mut file = OpenOptions::new().write(true).create_new(true).open(dest)?;
        let mut hasher = Sha256::new();
        let mut done = 0u64;
        progress(0, asset.size);
        stream_body(
            response,
            asset.size,
            Some(asset.size),
            "the update archive",
            cancel,
            Instant::now() + ARCHIVE_DEADLINE,
            &mut |chunk| {
                file.write_all(chunk)?;
                hasher.update(chunk);
                done += chunk.len() as u64;
                progress(done, asset.size);
                Ok(())
            },
        )?;
        file.flush()?;
        Ok(hex_digest(hasher))
    }

    /// Downloads the release archive, verifies it against `SHA256SUMS` (and GitHub's digest
    /// when listed) and only then extracts the executable, `LICENSE` and notices into a private
    /// temporary directory owned by the returned [`VerifiedUpdate`]. `progress` receives
    /// `(bytes received, total bytes)` per chunk. Nothing is executed or installed.
    pub fn download(
        &self,
        release: &ReleaseInfo,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<VerifiedUpdate> {
        check_cancel(cancel)?;
        let sums = self.fetch_checksums(&release.checksums, cancel)?;
        if let Some(digest) = &release.checksums.sha256
            && *digest != sha256_hex(&sums)
        {
            return Err(UpdateError::Checksum(
                "SHA256SUMS does not match the digest GitHub lists for it".to_owned(),
            ));
        }
        let expected = parse_checksums(&sums, &release.archive.name)?;
        if let Some(digest) = &release.archive.sha256
            && *digest != expected
        {
            return Err(UpdateError::Checksum(
                "the digest GitHub lists for the archive disagrees with SHA256SUMS".to_owned(),
            ));
        }

        let dir = tempfile::Builder::new()
            .prefix("animatic-update-")
            .tempdir()?;
        let archive_path = dir.path().join("package");
        let actual = self.fetch_archive(&release.archive, &archive_path, cancel, progress)?;
        if actual != expected {
            return Err(UpdateError::Checksum(
                "the downloaded archive does not match SHA256SUMS".to_owned(),
            ));
        }
        check_cancel(cancel)?;

        let stage = dir.path().join("stage");
        fs::create_dir(&stage)?;
        let files = extract(release.platform, &release.version, &archive_path, &stage)?;
        Ok(VerifiedUpdate {
            version: release.version.clone(),
            sha256: actual,
            platform: release.platform,
            files,
            _dir: dir,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Hashing helpers
// ---------------------------------------------------------------------------------------------

fn hex_digest(hasher: Sha256) -> String {
    let out = hasher.finalize();
    let mut text = String::with_capacity(64);
    for byte in out.iter() {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_digest(hasher)
}

fn file_sha256(path: &Path) -> io::Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hasher.update(&buf[..n]);
    }
    Ok((total, hex_digest(hasher)))
}

// ---------------------------------------------------------------------------------------------
// Archive extraction
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
struct Staged {
    path: PathBuf,
    size: u64,
    sha256: String,
}

#[derive(Debug)]
struct Extracted {
    exe: Staged,
    license: Staged,
    notices: Staged,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Dir,
    Exe,
    License,
    Notices,
    Doc,
}

fn shown(name: &str) -> String {
    let short: String = name.chars().take(64).collect();
    format!("{short:?}")
}

fn valid_doc_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 100
        && name.ends_with(".md")
        && !name.contains("..")
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Entry-by-entry policy of an archive: exact names, no duplicates, counted and capped sizes.
struct Manifest {
    stem: String,
    exe_name: &'static str,
    seen: HashSet<String>,
    entries: usize,
    total: u64,
}

impl Manifest {
    fn new(stem: String, exe_name: &'static str) -> Self {
        Manifest {
            stem,
            exe_name,
            seen: HashSet::new(),
            entries: 0,
            total: 0,
        }
    }

    /// Validates one entry by its declared name and size and returns what it is.
    fn admit(&mut self, raw: &[u8], is_dir: bool, size: u64) -> Result<Role> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err(UpdateError::Archive(format!(
                "the archive has more than {MAX_ENTRIES} entries"
            )));
        }
        let name = std::str::from_utf8(raw)
            .map_err(|_| UpdateError::Archive("an entry name is not valid UTF-8".to_owned()))?;
        let unsafe_name = || {
            UpdateError::Archive(format!(
                "an entry has an unsafe or unexpected path: {}",
                shown(name)
            ))
        };
        if name
            .chars()
            .any(|c| c.is_control() || c == '\\' || c == ':')
        {
            return Err(unsafe_name());
        }
        let trimmed = if is_dir {
            name.strip_suffix('/').unwrap_or(name)
        } else {
            name
        };
        let parts: Vec<&str> = trimmed.split('/').collect();
        if parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == "..")
            || parts[0] != self.stem
        {
            return Err(unsafe_name());
        }
        let role = match (&parts[1..], is_dir) {
            ([], true) | (["docs"], true) => Role::Dir,
            ([file], false) if *file == self.exe_name => Role::Exe,
            (["LICENSE"], false) => Role::License,
            (["THIRD-PARTY-NOTICES.html"], false) => Role::Notices,
            (["README.md"], false) | (["CHANGELOG.md"], false) => Role::Doc,
            (["docs", file], false) if valid_doc_name(file) => Role::Doc,
            _ => return Err(unsafe_name()),
        };
        let limit = match role {
            Role::Dir => 0,
            Role::Exe => MAX_EXECUTABLE_BYTES,
            _ => MAX_LICENSE_BYTES,
        };
        if size > limit {
            return Err(UpdateError::Archive(format!(
                "the entry {} is larger than allowed",
                shown(name)
            )));
        }
        self.total = self.total.saturating_add(size);
        if self.total > MAX_EXPANDED_BYTES {
            return Err(UpdateError::Archive(
                "the archive expands beyond the allowed total size".to_owned(),
            ));
        }
        if !self.seen.insert(trimmed.to_ascii_lowercase()) {
            return Err(UpdateError::Archive(format!(
                "the entry {} appears more than once",
                shown(name)
            )));
        }
        Ok(role)
    }
}

/// Limits the bytes read from a decompressed stream, failing when the limit is exceeded.
struct ExpandLimit<R> {
    inner: R,
    remaining: u64,
}

impl<R: Read> Read for ExpandLimit<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::other(
                    "the archive expands beyond the allowed size",
                )),
            };
        }
        let max = buf
            .len()
            .min(self.remaining.min(usize::MAX as u64) as usize);
        let n = self.inner.read(&mut buf[..max])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}

/// Copies exactly `declared` bytes of an entry into a new file, hashing on the way. Any
/// difference between the declared and the actual length is an error.
fn copy_entry(reader: &mut dyn Read, dest: &Path, declared: u64) -> Result<Staged> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(dest)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let want = (declared - total).saturating_add(1).min(buf.len() as u64) as usize;
        let n = reader
            .read(&mut buf[..want])
            .map_err(|e| UpdateError::Archive(format!("an archive entry cannot be read ({e})")))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > declared {
            return Err(UpdateError::Archive(
                "an entry is larger than its declared size".to_owned(),
            ));
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])?;
    }
    if total != declared {
        return Err(UpdateError::Archive(
            "an entry is shorter than its declared size".to_owned(),
        ));
    }
    file.flush()?;
    Ok(Staged {
        path: dest.to_owned(),
        size: total,
        sha256: hex_digest(hasher),
    })
}

#[derive(Default)]
struct Collected {
    exe: Option<Staged>,
    license: Option<Staged>,
    notices: Option<Staged>,
}

impl Collected {
    fn store(
        &mut self,
        role: Role,
        exe_name: &str,
        reader: &mut dyn Read,
        size: u64,
        stage: &Path,
    ) -> Result<()> {
        let (slot, file_name) = match role {
            Role::Exe => (&mut self.exe, exe_name),
            Role::License => (&mut self.license, "LICENSE"),
            Role::Notices => (&mut self.notices, "THIRD-PARTY-NOTICES.html"),
            Role::Doc | Role::Dir => return Ok(()),
        };
        if slot.is_some() {
            return Err(UpdateError::Archive(format!(
                "{file_name} appears more than once"
            )));
        }
        *slot = Some(copy_entry(reader, &stage.join(file_name), size)?);
        Ok(())
    }
}

fn zip_error(e: zip::result::ZipError) -> UpdateError {
    UpdateError::Archive(format!("the zip archive is invalid ({e})"))
}

fn extract_zip(
    archive: &Path,
    stage: &Path,
    manifest: &mut Manifest,
    platform: Platform,
) -> Result<Collected> {
    let mut zip = zip::ZipArchive::new(File::open(archive)?).map_err(zip_error)?;
    if zip.has_duplicate_names() {
        return Err(UpdateError::Archive(
            "the archive has duplicate entries".to_owned(),
        ));
    }
    if zip.len() > MAX_ENTRIES {
        return Err(UpdateError::Archive(format!(
            "the archive has more than {MAX_ENTRIES} entries"
        )));
    }
    let mut plan = Vec::new();
    for index in 0..zip.len() {
        let (raw, is_dir, size, mode, encrypted, method_ok) = {
            let entry = zip.by_index_data(index).map_err(zip_error)?;
            (
                entry.name_raw().to_vec(),
                entry.is_dir(),
                entry.size(),
                entry.unix_mode(),
                entry.encrypted(),
                matches!(
                    entry.compression(),
                    zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
                ),
            )
        };
        if encrypted || !method_ok {
            return Err(UpdateError::Archive(
                "an entry is encrypted or uses an unsupported compression".to_owned(),
            ));
        }
        let kind = mode.map_or(0, |m| m & 0o170000);
        let kind_ok = if is_dir {
            matches!(kind, 0 | 0o040000)
        } else {
            matches!(kind, 0 | 0o100000)
        };
        if !kind_ok {
            return Err(UpdateError::Archive(
                "the archive contains a link or special file".to_owned(),
            ));
        }
        let role = manifest.admit(&raw, is_dir, size)?;
        if role == Role::Exe && platform == Platform::Linux && !mode.is_some_and(|m| m & 0o111 != 0)
        {
            return Err(UpdateError::Archive(
                "the executable is not marked executable in the archive".to_owned(),
            ));
        }
        plan.push((index, role, size));
    }
    let mut collected = Collected::default();
    for (index, role, size) in plan {
        if matches!(role, Role::Exe | Role::License | Role::Notices) {
            let mut entry = zip.by_index(index).map_err(zip_error)?;
            collected.store(role, manifest.exe_name, &mut entry, size, stage)?;
        }
    }
    Ok(collected)
}

fn extract_tar_gz(
    archive: &Path,
    stage: &Path,
    manifest: &mut Manifest,
    platform: Platform,
) -> Result<Collected> {
    let read_error =
        |e: io::Error| UpdateError::Archive(format!("the archive cannot be read ({e})"));
    let reader = ExpandLimit {
        inner: flate2::read::GzDecoder::new(File::open(archive)?),
        remaining: MAX_EXPANDED_BYTES + STREAM_OVERHEAD,
    };
    let mut tar = tar::Archive::new(reader);
    let mut collected = Collected::default();
    for entry in tar.entries().map_err(read_error)? {
        let mut entry = entry.map_err(read_error)?;
        let kind = entry.header().entry_type();
        let is_dir = kind == tar::EntryType::Directory;
        if !is_dir && kind != tar::EntryType::Regular {
            return Err(UpdateError::Archive(
                "the archive contains a link or special file".to_owned(),
            ));
        }
        let size = entry.size();
        let mode = entry.header().mode().unwrap_or(0);
        let raw = entry.path_bytes().into_owned();
        let role = manifest.admit(&raw, is_dir, size)?;
        if role == Role::Exe && platform == Platform::Linux && mode & 0o111 == 0 {
            return Err(UpdateError::Archive(
                "the executable is not marked executable in the archive".to_owned(),
            ));
        }
        collected.store(role, manifest.exe_name, &mut entry, size, stage)?;
    }
    Ok(collected)
}

/// Checks the container headers of the staged executable: PE x86-64 or ELF x86-64.
fn validate_executable(platform: Platform, path: &Path) -> Result<()> {
    let bad = |why: &str| {
        UpdateError::Archive(format!(
            "the staged executable is not a valid {} x86-64 executable ({why})",
            platform.label()
        ))
    };
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    match platform {
        Platform::Windows => {
            let mut dos = [0u8; 64];
            file.read_exact(&mut dos).map_err(|_| bad("too short"))?;
            if &dos[..2] != b"MZ" {
                return Err(bad("missing MZ header"));
            }
            let offset = u32::from_le_bytes([dos[60], dos[61], dos[62], dos[63]]) as u64;
            if offset + 6 > len {
                return Err(bad("PE header offset out of range"));
            }
            file.seek(SeekFrom::Start(offset))?;
            let mut pe = [0u8; 6];
            file.read_exact(&mut pe).map_err(|_| bad("too short"))?;
            if &pe[..4] != b"PE\0\0" {
                return Err(bad("missing PE signature"));
            }
            if u16::from_le_bytes([pe[4], pe[5]]) != 0x8664 {
                return Err(bad("wrong machine type"));
            }
        }
        Platform::Linux => {
            let mut elf = [0u8; 20];
            file.read_exact(&mut elf).map_err(|_| bad("too short"))?;
            if elf[..4] != [0x7f, b'E', b'L', b'F'] {
                return Err(bad("missing ELF magic"));
            }
            if elf[4] != 2 || elf[5] != 1 {
                return Err(bad("not a 64-bit little-endian ELF"));
            }
            if u16::from_le_bytes([elf[18], elf[19]]) != 0x3e {
                return Err(bad("wrong machine type"));
            }
        }
    }
    Ok(())
}

fn extract(platform: Platform, version: &str, archive: &Path, stage: &Path) -> Result<Extracted> {
    let mut manifest = Manifest::new(platform.stem(version), platform.exe_name());
    let collected = match platform {
        Platform::Windows => extract_zip(archive, stage, &mut manifest, platform)?,
        Platform::Linux => extract_tar_gz(archive, stage, &mut manifest, platform)?,
    };
    let missing = |what: &str| UpdateError::Archive(format!("the archive does not contain {what}"));
    let exe = collected.exe.ok_or_else(|| missing(platform.exe_name()))?;
    let license = collected.license.ok_or_else(|| missing("LICENSE"))?;
    let notices = collected
        .notices
        .ok_or_else(|| missing("THIRD-PARTY-NOTICES.html"))?;
    validate_executable(platform, &exe.path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&exe.path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(Extracted {
        exe,
        license,
        notices,
    })
}

// ---------------------------------------------------------------------------------------------
// Installation target identity
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileId {
    volume: u64,
    index: u64,
}

#[cfg(unix)]
fn file_id(file: &File) -> io::Result<FileId> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata()?;
    Ok(FileId {
        volume: meta.dev(),
        index: meta.ino(),
    })
}

#[cfg(windows)]
fn file_id(file: &File) -> io::Result<FileId> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    // SAFETY: the handle is valid for the duration of the call and `info` is a plain
    // out-structure of integers which is fully initialized by a successful call.
    unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(FileId {
            volume: u64::from(info.dwVolumeSerialNumber),
            index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        })
    }
}

#[cfg(not(any(unix, windows)))]
fn file_id(_file: &File) -> io::Result<FileId> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// Opens an existing file for reading without blocking renames or deletion of it.
fn open_shared(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
        options.share_mode(0x1 | 0x2 | 0x4);
    }
    options.open(path)
}

/// The executable this process was started from, bound by an open handle and the OS file
/// identity captured at startup. An installation only proceeds while the file at that path is
/// still the very file this process started from.
#[derive(Debug)]
pub struct InstallationTarget {
    path: PathBuf,
    id: FileId,
    _handle: File,
}

impl InstallationTarget {
    /// Captures the identity of the running executable. Call once, early in `main`.
    pub fn capture() -> Result<Self> {
        let exe = std::env::current_exe().map_err(|e| {
            UpdateError::Refused(format!("the running executable cannot be determined ({e})"))
        })?;
        Self::capture_path(&exe)
    }

    fn capture_path(path: &Path) -> Result<Self> {
        let path = fs::canonicalize(path).map_err(|e| {
            UpdateError::Refused(format!("the executable path cannot be resolved ({e})"))
        })?;
        let handle = open_shared(&path)?;
        if !handle.metadata()?.is_file() {
            return Err(UpdateError::Refused(
                "the executable is not a regular file".to_owned(),
            ));
        }
        let id = file_id(&handle)?;
        Ok(InstallationTarget {
            path,
            id,
            _handle: handle,
        })
    }

    /// Canonical path of the executable captured at startup (use it to restart or display).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Revalidates that `current` (the running executable now) and the file on disk still are
    /// the captured executable.
    fn revalidate(&self, current: &Path) -> Result<()> {
        let current = fs::canonicalize(current).map_err(|e| {
            UpdateError::Refused(format!("the running executable cannot be resolved ({e})"))
        })?;
        if current != self.path {
            return Err(UpdateError::Refused(
                "the running executable is not the installation this process started from"
                    .to_owned(),
            ));
        }
        let file = open_shared(&self.path).map_err(|e| {
            UpdateError::Refused(format!("the installed executable cannot be opened ({e})"))
        })?;
        if file_id(&file)? != self.id {
            return Err(UpdateError::Refused(
                "the executable on disk is no longer the file this process was started from \
                 (another installer may have updated it); restart Animatic and check again"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------------------------

fn install_io(e: io::Error) -> UpdateError {
    if matches!(
        e.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
    ) {
        UpdateError::NotWritable(e.to_string())
    } else {
        UpdateError::Io(e)
    }
}

/// Advisory lock on the reserved, empty lock file next to the executable. The file is never
/// read, written or removed; it is released when this guard is dropped.
struct InstallLock {
    file: File,
}

impl InstallLock {
    fn acquire(dir: &Path) -> Result<Self> {
        let path = dir.join(LOCK_NAME);
        match fs::symlink_metadata(&path) {
            Ok(meta) if !meta.file_type().is_file() => {
                return Err(UpdateError::Refused(format!(
                    "{LOCK_NAME} exists but is not a regular file"
                )));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(install_io(e)),
        }
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(false);
        // Never follow a symlink / reparse point at the lock name.
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(0o400000); // O_NOFOLLOW
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
        }
        let file = options.open(&path).map_err(install_io)?;
        let not_ours = || {
            UpdateError::Refused(format!(
                "{LOCK_NAME} is not a regular file inside the installation folder"
            ))
        };
        match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Err(UpdateError::Busy),
            Err(fs::TryLockError::Error(e)) => return Err(install_io(e)),
        }
        // Under the lock: the handle we hold must be the regular file the name designates, and
        // that file must live directly inside the installation folder. The folder itself is
        // trusted (it is where the executable lives).
        let named_is_file = fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_file())
            .unwrap_or(false);
        let resolved_inside = fs::canonicalize(&path)
            .map(|p| p.parent() == Some(dir))
            .unwrap_or(false);
        let same_file = match (file_id(&file), open_shared(&path).and_then(|f| file_id(&f))) {
            (Ok(held), Ok(named)) => held == named,
            _ => false,
        };
        if !named_is_file || !resolved_inside || !same_file || !file.metadata()?.is_file() {
            return Err(not_ours());
        }
        Ok(InstallLock { file })
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Whether the open files `a` and `b` have identical content.
fn same_content(a: &Path, b: &Path) -> io::Result<bool> {
    let (mut fa, mut fb) = (File::open(a)?, File::open(b)?);
    if fa.metadata()?.len() != fb.metadata()?.len() {
        return Ok(false);
    }
    let (mut ba, mut bb) = (vec![0u8; CHUNK], vec![0u8; CHUNK]);
    loop {
        let n = fa.read(&mut ba)?;
        if n == 0 {
            return Ok(true);
        }
        fb.read_exact(&mut bb[..n])?;
        if ba[..n] != bb[..n] {
            return Ok(false);
        }
    }
}

fn accept_existing(source: &Path, dest: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(dest).map_err(install_io)?;
    if !meta.file_type().is_file() {
        return Err(UpdateError::Refused(format!(
            "{} exists and is not a regular file",
            dest.display()
        )));
    }
    if !same_content(source, dest).map_err(install_io)? {
        return Err(UpdateError::Refused(format!(
            "{} already exists with different content; it is not overwritten",
            dest.display()
        )));
    }
    Ok(())
}

/// Places `source` at `dest` without ever replacing a different existing file: the content is
/// written to a temporary file next to `dest` and moved into place with a no-clobber persist.
/// An existing identical regular file is accepted.
fn install_additive(source: &Path, dest: &Path) -> Result<()> {
    let dir = dest
        .parent()
        .ok_or_else(|| UpdateError::Refused("the installation path has no folder".to_owned()))?;
    let mut temp = tempfile::Builder::new()
        .prefix(".animatic-install-")
        .tempfile_in(dir)
        .map_err(install_io)?;
    io::copy(&mut File::open(source)?, temp.as_file_mut()).map_err(install_io)?;
    temp.as_file().sync_all().map_err(install_io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o644)).map_err(install_io)?;
    }
    match temp.persist_noclobber(dest) {
        Ok(_) => Ok(()),
        Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => accept_existing(source, dest),
        Err(e) => Err(install_io(e.error)),
    }
}

fn verify_staged(staged: &Staged) -> Result<()> {
    let (size, sha256) = file_sha256(&staged.path)?;
    if size != staged.size || sha256 != staged.sha256 {
        return Err(UpdateError::Checksum(
            "a staged update file changed after verification".to_owned(),
        ));
    }
    Ok(())
}

/// Copy of the current executable that is kept after a failed replacement. Created with a
/// random unique name, never over an existing file.
fn create_recovery_copy(exe: &Path) -> io::Result<PathBuf> {
    let dir = exe
        .parent()
        .ok_or_else(|| io::Error::other("the executable has no folder"))?;
    let suffix = exe
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut builder = tempfile::Builder::new();
    builder.prefix(".animatic-recovery-").suffix(&suffix);
    let mut temp = builder.tempfile_in(dir)?;
    io::copy(&mut File::open(exe)?, temp.as_file_mut())?;
    temp.as_file().sync_all()?;
    fs::set_permissions(temp.path(), fs::metadata(exe)?.permissions())?;
    let (_, path) = temp.keep().map_err(|e| e.error)?;
    Ok(path)
}

enum Restore {
    Restored,
    Present,
}

/// Puts the recovery copy back at `exe` only when nothing exists there; never overwrites.
fn restore_if_absent(exe: &Path, backup: &Path) -> io::Result<Restore> {
    match fs::symlink_metadata(exe) {
        Ok(_) => return Ok(Restore::Present),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let dir = exe
        .parent()
        .ok_or_else(|| io::Error::other("the executable has no folder"))?;
    let mut temp = tempfile::Builder::new()
        .prefix(".animatic-restore-")
        .tempfile_in(dir)?;
    io::copy(&mut File::open(backup)?, temp.as_file_mut())?;
    temp.as_file().sync_all()?;
    fs::set_permissions(temp.path(), fs::metadata(backup)?.permissions())?;
    match temp.persist_noclobber(exe) {
        Ok(_) => Ok(Restore::Restored),
        Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => Ok(Restore::Present),
        Err(e) => Err(e.error),
    }
}

fn recovery_message(exe: &Path, backup: Option<&Path>) -> String {
    let Some(backup) = backup else {
        return "The replacement is a single atomic rename on this platform, so the previous \
                executable should be unchanged; restart Animatic, or install the release \
                manually if it does not start."
            .to_owned();
    };
    match restore_if_absent(exe, backup) {
        Ok(Restore::Restored) => format!(
            "The previous executable was restored to {}. A harmless copy of it remains at {}; \
             delete it once Animatic starts normally.",
            exe.display(),
            backup.display()
        ),
        Ok(Restore::Present) => format!(
            "A file is present at {} and was not overwritten. A copy of the previous version \
             is kept at {}.",
            exe.display(),
            backup.display()
        ),
        Err(e) => format!(
            "Restoring the previous executable failed ({e}). To recover manually, copy {} to {}.",
            backup.display(),
            exe.display()
        ),
    }
}

/// Runs `replace(staged)` with a recovery copy of the executable when `backup` is set. On
/// failure the original path is restored only if absent and the copy is kept; on success the
/// copy is removed.
fn replace_with_recovery(
    exe: &Path,
    staged: &Path,
    backup: bool,
    replace: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<()> {
    let backup_path = if backup {
        Some(create_recovery_copy(exe).map_err(install_io)?)
    } else {
        None
    };
    match replace(staged) {
        Ok(()) => {
            if let Some(path) = &backup_path {
                let _ = fs::remove_file(path);
            }
            Ok(())
        }
        Err(e) => Err(UpdateError::Replace {
            cause: e.to_string(),
            recovery: recovery_message(exe, backup_path.as_deref()),
            backup: backup_path,
        }),
    }
}

/// A downloaded release whose archive checksum was verified and whose executable, license and
/// notices are staged in a private temporary directory owned by this value.
#[derive(Debug)]
pub struct VerifiedUpdate {
    version: String,
    sha256: String,
    platform: Platform,
    files: Extracted,
    _dir: TempDir,
}

impl VerifiedUpdate {
    pub fn version(&self) -> &str {
        &self.version
    }

    /// SHA-256 of the verified archive (lowercase hex).
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Installs this update over the executable captured in `target`; returns the installed
    /// version. Refuses when the identity of the executable changed. A failure of the OS
    /// replacement is [`UpdateError::Replace`] (`install_attempted() == true`).
    pub fn install(self, target: &InstallationTarget) -> Result<String> {
        if Platform::current() != Some(self.platform) {
            return Err(UpdateError::Refused(
                "this update was built for a different platform".to_owned(),
            ));
        }
        let current = std::env::current_exe().map_err(|e| {
            UpdateError::Refused(format!("the running executable cannot be determined ({e})"))
        })?;
        self.install_with(target, &current, cfg!(windows), |staged: &Path| {
            self_replace::self_replace(staged)
        })
    }

    fn install_with(
        self,
        target: &InstallationTarget,
        current: &Path,
        backup: bool,
        replace: impl FnOnce(&Path) -> io::Result<()>,
    ) -> Result<String> {
        target.revalidate(current)?;
        let exe = target.path().to_path_buf();
        let dir = exe
            .parent()
            .ok_or_else(|| UpdateError::Refused("the executable has no folder".to_owned()))?;
        let _lock = InstallLock::acquire(dir)?;
        // Another installer may have won the race before the lock was taken.
        target.revalidate(current)?;

        verify_staged(&self.files.exe)?;
        verify_staged(&self.files.license)?;
        verify_staged(&self.files.notices)?;

        install_additive(
            &self.files.license.path,
            &dir.join(format!("LICENSE-v{}", self.version)),
        )?;
        install_additive(
            &self.files.notices.path,
            &dir.join(format!("THIRD-PARTY-NOTICES-v{}.html", self.version)),
        )?;
        replace_with_recovery(&exe, &self.files.exe.path, backup, replace)?;
        Ok(self.version.clone())
    }
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::net::{TcpListener, TcpStream};
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::thread;

    const LICENSE_TEXT: &[u8] = b"synthetic license text\n";
    const NOTICES_TEXT: &[u8] = b"<html>synthetic notices</html>\n";
    const VER: &str = "2099.1.0";
    const CURRENT: &str = "2026.11.0";
    const BOTH: [Platform; 2] = [Platform::Windows, Platform::Linux];

    // ----- loopback HTTP fixture -----

    struct Reply {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        declared_len: Option<usize>,
    }

    impl Reply {
        fn ok(body: Vec<u8>) -> Reply {
            Reply {
                status: 200,
                headers: Vec::new(),
                body,
                declared_len: None,
            }
        }

        fn status(status: u16) -> Reply {
            Reply {
                status,
                ..Reply::ok(Vec::new())
            }
        }

        fn header(mut self, name: &str, value: &str) -> Reply {
            self.headers.push((name.to_owned(), value.to_owned()));
            self
        }
    }

    type Handler = dyn Fn(&str, &str) -> Reply + Send + Sync;

    struct Fixture {
        origin: String,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
    }

    fn serve_one(mut stream: TcpStream, origin: &str, handler: &Handler, log: &Mutex<Vec<String>>) {
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
        let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
        log.lock().unwrap().push(head);
        let reply = handler(origin, &path);
        let length = reply.declared_len.unwrap_or(reply.body.len());
        let mut out = format!(
            "HTTP/1.1 {} Fixture\r\nContent-Length: {length}\r\nConnection: close\r\n",
            reply.status
        );
        for (name, value) in &reply.headers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out.push_str("\r\n");
        let _ = stream.write_all(out.as_bytes());
        let _ = stream.write_all(&reply.body);
        let _ = stream.flush();
    }

    impl Fixture {
        fn start(handler: impl Fn(&str, &str) -> Reply + Send + Sync + 'static) -> Fixture {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let handler: Arc<Handler> = Arc::new(handler);
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let thread = {
                let (origin, requests, stop) = (origin.clone(), requests.clone(), stop.clone());
                thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                let (origin, handler, requests) =
                                    (origin.clone(), handler.clone(), requests.clone());
                                thread::spawn(move || {
                                    serve_one(stream, &origin, &*handler, &requests)
                                });
                            }
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(2));
                            }
                            Err(_) => break,
                        }
                    }
                })
            };
            Fixture {
                origin,
                requests,
                stop,
                thread: Some(thread),
            }
        }

        fn hits(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }

        fn client(&self) -> UpdateClient {
            UpdateClient::with_endpoints(Endpoints::loopback(&self.origin)).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    // ----- synthetic archives -----

    #[derive(Clone)]
    enum Kind {
        File,
        Dir,
        Symlink(String),
        Hardlink(String),
    }

    #[derive(Clone)]
    struct Entry {
        name: String,
        data: Vec<u8>,
        kind: Kind,
        perm: u32,
        declared: Option<u64>,
        deflate: bool,
        encrypted: bool,
    }

    impl Entry {
        fn file(name: &str, data: &[u8]) -> Entry {
            Entry {
                name: name.to_owned(),
                data: data.to_vec(),
                kind: Kind::File,
                perm: 0o644,
                declared: None,
                deflate: false,
                encrypted: false,
            }
        }

        fn exec(name: &str, data: &[u8]) -> Entry {
            Entry {
                perm: 0o755,
                ..Entry::file(name, data)
            }
        }

        fn dir(name: &str) -> Entry {
            Entry {
                kind: Kind::Dir,
                perm: 0o755,
                ..Entry::file(name, b"")
            }
        }

        fn link(name: &str, kind: Kind) -> Entry {
            Entry {
                kind,
                ..Entry::file(name, b"")
            }
        }
    }

    fn fake_exe(platform: Platform) -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        match platform {
            Platform::Windows => {
                bytes[0] = b'M';
                bytes[1] = b'Z';
                bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
                bytes.extend_from_slice(b"PE\0\0");
                bytes.extend_from_slice(&0x8664u16.to_le_bytes());
            }
            Platform::Linux => {
                bytes[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
                bytes[4] = 2;
                bytes[5] = 1;
                bytes[6] = 1;
                bytes[18] = 0x3e;
            }
        }
        bytes.extend_from_slice(b"synthetic-binary");
        bytes
    }

    fn noise(len: usize) -> Vec<u8> {
        let mut x: u32 = 0x1234_5678;
        (0..len)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect()
    }

    fn standard_entries(platform: Platform, version: &str, exe: &[u8]) -> Vec<Entry> {
        let stem = platform.stem(version);
        let mut entries = Vec::new();
        if platform == Platform::Linux {
            entries.push(Entry::dir(&format!("{stem}/")));
            entries.push(Entry::dir(&format!("{stem}/docs")));
        }
        entries.push(Entry::exec(&format!("{stem}/{}", platform.exe_name()), exe));
        entries.push(Entry::file(&format!("{stem}/LICENSE"), LICENSE_TEXT));
        entries.push(Entry::file(
            &format!("{stem}/THIRD-PARTY-NOTICES.html"),
            NOTICES_TEXT,
        ));
        entries.push(Entry::file(&format!("{stem}/README.md"), b"readme"));
        entries.push(Entry::file(&format!("{stem}/CHANGELOG.md"), b"changelog"));
        entries.push(Entry::file(&format!("{stem}/docs/guide.md"), b"guide"));
        entries
    }

    fn octal(field: &mut [u8], value: u64) {
        let text = format!("{:0width$o}", value, width = field.len() - 1);
        field[..text.len()].copy_from_slice(text.as_bytes());
    }

    fn tar_header(name: &str, size: u64, flag: u8, perm: u32, link: &str) -> [u8; 512] {
        assert!(name.len() <= 100 && link.len() <= 100);
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        octal(&mut h[100..108], u64::from(perm));
        octal(&mut h[108..116], 0);
        octal(&mut h[116..124], 0);
        octal(&mut h[124..136], size);
        octal(&mut h[136..148], 315_532_800);
        h[148..156].fill(b' ');
        h[156] = flag;
        h[157..157 + link.len()].copy_from_slice(link.as_bytes());
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
        let text = format!("{sum:06o}\0 ");
        h[148..156].copy_from_slice(text.as_bytes());
        h
    }

    fn tar_gz(entries: &[Entry]) -> Vec<u8> {
        let mut tar = Vec::new();
        for e in entries {
            let (flag, link) = match &e.kind {
                Kind::File => (b'0', ""),
                Kind::Dir => (b'5', ""),
                Kind::Symlink(t) => (b'2', t.as_str()),
                Kind::Hardlink(t) => (b'1', t.as_str()),
            };
            let data: &[u8] = if matches!(e.kind, Kind::File) {
                &e.data
            } else {
                &[]
            };
            let size = e.declared.unwrap_or(data.len() as u64);
            tar.extend_from_slice(&tar_header(&e.name, size, flag, e.perm, link));
            tar.extend_from_slice(data);
            let padded = tar.len() + (512 - data.len() % 512) % 512;
            tar.resize(padded, 0);
        }
        tar.resize(tar.len() + 1024, 0);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    fn put16(out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&v.to_le_bytes());
    }

    fn put32(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_le_bytes());
    }

    fn zip_bytes(entries: &[Entry]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for e in entries {
            let (data, is_dir, type_bits): (Vec<u8>, bool, u32) = match &e.kind {
                Kind::File | Kind::Hardlink(_) => (e.data.clone(), false, 0o100000),
                Kind::Dir => (Vec::new(), true, 0o040000),
                Kind::Symlink(t) => (t.as_bytes().to_vec(), false, 0o120000),
            };
            let crc = crc32fast::hash(&data);
            let payload = if e.deflate {
                let mut enc =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
                enc.write_all(&data).unwrap();
                enc.finish().unwrap()
            } else {
                data.clone()
            };
            let method: u16 = if e.deflate { 8 } else { 0 };
            let flags: u16 = u16::from(e.encrypted);
            let size = e.declared.unwrap_or(data.len() as u64) as u32;
            let offset = out.len() as u32;
            let name = e.name.as_bytes();

            put32(&mut out, 0x0403_4b50);
            put16(&mut out, 20);
            put16(&mut out, flags);
            put16(&mut out, method);
            put16(&mut out, 0);
            put16(&mut out, 0x21);
            put32(&mut out, crc);
            put32(&mut out, payload.len() as u32);
            put32(&mut out, size);
            put16(&mut out, name.len() as u16);
            put16(&mut out, 0);
            out.extend_from_slice(name);
            out.extend_from_slice(&payload);

            put32(&mut central, 0x0201_4b50);
            put16(&mut central, 0x0314);
            put16(&mut central, 20);
            put16(&mut central, flags);
            put16(&mut central, method);
            put16(&mut central, 0);
            put16(&mut central, 0x21);
            put32(&mut central, crc);
            put32(&mut central, payload.len() as u32);
            put32(&mut central, size);
            put16(&mut central, name.len() as u16);
            put16(&mut central, 0);
            put16(&mut central, 0);
            put16(&mut central, 0);
            put16(&mut central, 0);
            put32(
                &mut central,
                ((type_bits | e.perm) << 16) | if is_dir { 0x10 } else { 0 },
            );
            put32(&mut central, offset);
            central.extend_from_slice(name);
        }
        let central_offset = out.len() as u32;
        out.extend_from_slice(&central);
        put32(&mut out, 0x0605_4b50);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, entries.len() as u16);
        put16(&mut out, entries.len() as u16);
        put32(&mut out, central.len() as u32);
        put32(&mut out, central_offset);
        put16(&mut out, 0);
        out
    }

    fn archive_bytes(platform: Platform, entries: &[Entry]) -> Vec<u8> {
        match platform {
            Platform::Windows => zip_bytes(entries),
            Platform::Linux => tar_gz(entries),
        }
    }

    // ----- release site -----

    struct Package {
        platform: Platform,
        version: String,
        name: String,
        archive: Vec<u8>,
        sums: Vec<u8>,
    }

    fn package(platform: Platform, version: &str, entries: &[Entry]) -> Package {
        let archive = archive_bytes(platform, entries);
        let name = platform.archive_name(version);
        let sums = format!("{}  {name}\n", sha256_hex(&archive)).into_bytes();
        Package {
            platform,
            version: version.to_owned(),
            name,
            archive,
            sums,
        }
    }

    fn standard_package(platform: Platform, version: &str) -> Package {
        package(
            platform,
            version,
            &standard_entries(platform, version, &fake_exe(platform)),
        )
    }

    fn release_json(tag: &str, assets: &[(String, u64)]) -> Value {
        let assets: Vec<Value> = assets
            .iter()
            .map(|(name, size)| {
                json!({
                    "name": name,
                    "state": "uploaded",
                    "size": size,
                    "browser_download_url":
                        format!("{{ORIGIN}}/RetroHazard/animatic/releases/download/{tag}/{name}"),
                    "digest": null,
                })
            })
            .collect();
        json!({
            "tag_name": tag,
            "draft": false,
            "prerelease": false,
            "html_url": format!("{{ORIGIN}}/RetroHazard/animatic/releases/tag/{tag}"),
            "assets": assets,
        })
    }

    fn full_release(pkg: &Package) -> Value {
        release_json(
            &format!("v{}", pkg.version),
            &[
                (pkg.name.clone(), pkg.archive.len() as u64),
                (CHECKSUMS_NAME.to_owned(), pkg.sums.len() as u64),
            ],
        )
    }

    /// A release entry without assets; only valid as a lower, never-validated release.
    fn stub_release(tag: &str) -> Value {
        json!({"tag_name": tag, "draft": false, "prerelease": false})
    }

    #[derive(Clone, Default)]
    struct Site {
        pages: Vec<String>,
        files: HashMap<String, Vec<u8>>,
    }

    impl Site {
        fn page(mut self, releases: Vec<Value>) -> Site {
            self.pages.push(Value::Array(releases).to_string());
            self
        }

        fn serve(mut self, pkg: &Package) -> Site {
            let base = format!("/RetroHazard/animatic/releases/download/v{}", pkg.version);
            self.files
                .insert(format!("{base}/{}", pkg.name), pkg.archive.clone());
            self.files
                .insert(format!("{base}/{CHECKSUMS_NAME}"), pkg.sums.clone());
            self
        }

        fn reply(&self, origin: &str, path: &str) -> Reply {
            if let Some(query) = path.strip_prefix("/repos/RetroHazard/animatic/releases?") {
                let page: usize = query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("page="))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
                let body = self
                    .pages
                    .get(page - 1)
                    .cloned()
                    .unwrap_or_else(|| "[]".to_owned())
                    .replace("{ORIGIN}", origin);
                return Reply::ok(body.into_bytes());
            }
            match self.files.get(path) {
                Some(bytes) => Reply::ok(bytes.clone()),
                None => Reply::status(404),
            }
        }

        fn start(self) -> Fixture {
            Fixture::start(move |origin, path| self.reply(origin, path))
        }
    }

    fn cancel_flag() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn check_site(site: Site, current: &str, platform: Platform) -> Result<Option<ReleaseInfo>> {
        let fixture = site.start();
        fixture.client().check(current, platform, &cancel_flag())
    }

    /// Serves `entries` as the newest release and runs check + download.
    fn try_download(platform: Platform, entries: Vec<Entry>) -> Result<VerifiedUpdate> {
        let pkg = package(platform, VER, &entries);
        let fixture = Site::default()
            .page(vec![full_release(&pkg)])
            .serve(&pkg)
            .start();
        let client = fixture.client();
        let cancel = cancel_flag();
        let release = client
            .check(CURRENT, platform, &cancel)
            .unwrap()
            .expect("a newer release is offered");
        client.download(&release, &cancel, &mut |_, _| {})
    }

    fn assert_archive_rejected(platform: Platform, entries: Vec<Entry>, what: &str) {
        match try_download(platform, entries) {
            Err(UpdateError::Archive(_)) => {}
            other => panic!("{what} ({platform:?}): expected an archive error, got {other:?}"),
        }
    }

    // ----- pure rules -----

    #[test]
    fn calver_accepts_documented_forms_and_rejects_the_rest() {
        for ok in [
            "2026.11.0",
            "2026.1.0",
            "2026.12.34",
            "2000.1.0",
            "9999.12.0",
            "2026.11.0-rc.1",
            "2026.11.0-alpha.1.beta",
        ] {
            assert!(parse_calver(ok).is_ok(), "{ok}");
        }
        for bad in [
            "2026.13.0",
            "2026.0.0",
            "1999.12.0",
            "10000.1.0",
            "2026.01.0",
            "2026.1.01",
            "v2026.1.0",
            "2026.1",
            "2026.1.0+build.1",
            "2026.1.0-",
            "2026.1.0-01",
            "",
            " 2026.1.0",
        ] {
            assert!(parse_calver(bad).is_err(), "{bad:?}");
        }
        assert!(parse_tag("v2026.12.0").is_ok());
        assert!(parse_tag("2026.12.0").is_err());
        assert!(parse_tag("V2026.12.0").is_err());
        assert!(parse_calver(env!("CARGO_PKG_VERSION")).is_ok());
    }

    #[test]
    fn redirect_allow_list_is_exact() {
        let e = Endpoints::github();
        for ok in [
            "https://github.com/RetroHazard/animatic/releases/download/v1/x",
            "https://api.github.com/repos/x",
            "https://release-assets.githubusercontent.com/a?b=c",
            "https://objects.githubusercontent.com/a",
            "https://GitHub.com/x",
            "https://github.com:443/x",
        ] {
            assert!(e.allows(ok), "{ok}");
        }
        for bad in [
            "http://github.com/x",
            "https://evil.example/x",
            "https://github.com.evil.example/x",
            "https://evilgithubusercontent.com/x",
            "https://sub.githubusercontent.com/x",
            "https://raw.githubusercontent.com/x",
            "https://user:pw@github.com/x",
            "https://github.com:8443/x",
            "ftp://github.com/x",
            "/relative",
            "https://127.0.0.1/x",
        ] {
            assert!(!e.allows(bad), "{bad}");
        }
    }

    #[test]
    fn checksum_file_is_parsed_strictly() {
        let h = "a".repeat(64);
        let other = "b".repeat(64);
        let name = "animatic-v2099.1.0-linux-x86_64.tar.gz";
        let good = format!("{h}  {name}\n{other}  animatic-v2099.1.0-windows-x86_64.zip\n");
        assert_eq!(parse_checksums(good.as_bytes(), name).unwrap(), h);
        let cases = [
            format!("{h}  {name}\n{h}  {name}\n"),
            format!("{h} {name}\n"),
            format!("{h}\t{name}\n"),
            format!("{}  {name}\n", "A".repeat(64)),
            format!("{}  {name}\n", "a".repeat(63)),
            format!("{h}  {name}\r\n"),
            format!("{h}  *{name}\n").replace(&format!("*{name}"), "dir/x"),
            format!("{h}  {name}\n\n"),
            format!("{other}  other.zip\n"),
            String::new(),
        ];
        for (i, text) in cases.iter().enumerate() {
            assert!(
                matches!(
                    parse_checksums(text.as_bytes(), name),
                    Err(UpdateError::Checksum(_))
                ),
                "case {i}"
            );
        }
        assert!(parse_checksums(&[0xff, 0xfe], name).is_err());
    }

    #[test]
    fn manifest_accepts_only_exact_names() {
        let stem = "animatic-v2099.1.0-linux-x86_64";
        let accept = |name: &str, dir: bool, size: u64| {
            Manifest::new(stem.to_owned(), "animatic").admit(name.as_bytes(), dir, size)
        };
        assert_eq!(
            accept(&format!("{stem}/animatic"), false, 1).unwrap(),
            Role::Exe
        );
        assert_eq!(
            accept(&format!("{stem}/LICENSE"), false, 1).unwrap(),
            Role::License
        );
        assert_eq!(
            accept(&format!("{stem}/THIRD-PARTY-NOTICES.html"), false, 1).unwrap(),
            Role::Notices
        );
        assert_eq!(
            accept(&format!("{stem}/README.md"), false, 1).unwrap(),
            Role::Doc
        );
        assert_eq!(
            accept(&format!("{stem}/docs/a-b_c.1.md"), false, 1).unwrap(),
            Role::Doc
        );
        assert_eq!(accept(&format!("{stem}/"), true, 0).unwrap(), Role::Dir);
        assert_eq!(accept(&format!("{stem}/docs"), true, 0).unwrap(), Role::Dir);
        assert_eq!(
            accept(&format!("{stem}/docs/"), true, 0).unwrap(),
            Role::Dir
        );
        for bad in [
            format!("{stem}/../x"),
            format!("/{stem}/animatic"),
            "other/animatic".to_owned(),
            format!("{stem}\\animatic"),
            format!("{stem}/animatic/"),
            format!("{stem}//LICENSE"),
            format!("{stem}/docs/../LICENSE"),
            format!("{stem}/docs/.hidden.md"),
            format!("{stem}/docs/sub/a.md"),
            format!("{stem}/docs/a..md"),
            format!("{stem}/extra.txt"),
            format!("{stem}/animatic.exe"),
            format!("{stem}/c:evil"),
            format!("{stem}/docs/a.txt"),
            format!("{stem}/LICENSE\0"),
            "animatic".to_owned(),
            stem.to_owned(),
        ] {
            assert!(accept(&bad, false, 1).is_err(), "{bad:?}");
        }
        assert!(accept(&format!("{stem}/other"), true, 0).is_err());
        assert!(accept(&format!("{stem}/"), true, 1).is_err());
        assert!(accept(&format!("{stem}/LICENSE"), true, 0).is_err());
        assert!(accept(&format!("{stem}/animatic"), false, MAX_EXECUTABLE_BYTES + 1).is_err());
        assert!(accept(&format!("{stem}/LICENSE"), false, MAX_LICENSE_BYTES + 1).is_err());
        assert!(
            Manifest::new(stem.to_owned(), "animatic")
                .admit(&[0xff, 0xfe], false, 1)
                .is_err()
        );
    }

    #[test]
    fn manifest_caps_entry_count_total_size_and_duplicates() {
        let stem = "animatic-v2099.1.0-linux-x86_64";
        let mut count = Manifest::new(stem.to_owned(), "animatic");
        let mut outcome = Ok(Role::Dir);
        for i in 0..=MAX_ENTRIES {
            outcome = count.admit(format!("{stem}/docs/d{i}.md").as_bytes(), false, 1);
        }
        assert!(matches!(outcome, Err(UpdateError::Archive(_))));

        let mut total = Manifest::new(stem.to_owned(), "animatic");
        total
            .admit(
                format!("{stem}/animatic").as_bytes(),
                false,
                MAX_EXECUTABLE_BYTES,
            )
            .unwrap();
        let docs = (MAX_EXPANDED_BYTES - MAX_EXECUTABLE_BYTES) / MAX_LICENSE_BYTES;
        for i in 0..docs {
            total
                .admit(
                    format!("{stem}/docs/d{i}.md").as_bytes(),
                    false,
                    MAX_LICENSE_BYTES,
                )
                .unwrap();
        }
        assert!(matches!(
            total.admit(format!("{stem}/docs/over.md").as_bytes(), false, 1),
            Err(UpdateError::Archive(_))
        ));

        let mut dup = Manifest::new(stem.to_owned(), "animatic");
        dup.admit(format!("{stem}/LICENSE").as_bytes(), false, 1)
            .unwrap();
        assert!(
            dup.admit(format!("{stem}/LICENSE").as_bytes(), false, 1)
                .is_err()
        );
        let mut dirs = Manifest::new(stem.to_owned(), "animatic");
        dirs.admit(format!("{stem}/docs").as_bytes(), true, 0)
            .unwrap();
        assert!(
            dirs.admit(format!("{stem}/docs/").as_bytes(), true, 0)
                .is_err()
        );
    }

    #[test]
    fn expand_limit_stops_decompression_bombs() {
        let mut limited = ExpandLimit {
            inner: io::repeat(0),
            remaining: 100,
        };
        let mut sink = Vec::new();
        assert!(limited.read_to_end(&mut sink).is_err());
        assert_eq!(sink.len(), 100);

        let mut exact = ExpandLimit {
            inner: &[7u8; 100][..],
            remaining: 100,
        };
        let mut sink = Vec::new();
        exact.read_to_end(&mut sink).unwrap();
        assert_eq!(sink.len(), 100);
    }

    // ----- release discovery -----

    #[test]
    fn check_selects_the_highest_stable_release_in_any_order() {
        let platform = Platform::Linux;
        let winner = standard_package(platform, "2027.3.1");
        let noise_releases = |winner_at: usize| {
            let mut v = vec![
                stub_release("v2026.12.0"),
                stub_release("v2027.3.0"),
                json!({"tag_name": "v2099.1.0", "draft": true, "prerelease": false}),
                json!({"tag_name": "v2098.1.0", "draft": false, "prerelease": true}),
                stub_release("v2097.1.0-rc.1"),
                stub_release("v2026.13.0"),
                stub_release("2096.1.0"),
                stub_release("v2095.01.0"),
                stub_release("v2094.1.0+build"),
                stub_release("nightly"),
            ];
            v.insert(winner_at, full_release(&winner));
            v
        };
        for at in [0, 4, 10] {
            let site = Site::default().page(noise_releases(at)).serve(&winner);
            let release = check_site(site, CURRENT, platform)
                .unwrap()
                .expect("offered");
            assert_eq!(release.version, "2027.3.1");
            assert_eq!(release.tag, "v2027.3.1");
            assert!(
                release
                    .page_url
                    .ends_with("/RetroHazard/animatic/releases/tag/v2027.3.1")
            );
            assert_eq!(release.archive_size, winner.archive.len() as u64);
        }
    }

    #[test]
    fn check_reads_every_page_not_just_the_first_thirty() {
        let platform = Platform::Linux;
        let newest = standard_package(platform, "2027.1.0");
        let first: Vec<Value> = (0..RELEASES_PER_PAGE)
            .map(|i| stub_release(&format!("v2025.1.{i}")))
            .collect();
        let site = Site::default()
            .page(first)
            .page(vec![full_release(&newest)])
            .serve(&newest);
        let fixture = site.start();
        let release = fixture
            .client()
            .check(CURRENT, platform, &cancel_flag())
            .unwrap()
            .expect("offered from the second page");
        assert_eq!(release.version, "2027.1.0");
        let hits = fixture.hits();
        assert!(hits.iter().any(|h| h.contains("page=2")), "{hits:?}");
    }

    #[test]
    fn check_gives_up_explicitly_after_the_page_limit() {
        let page = Value::Array(
            (0..RELEASES_PER_PAGE)
                .map(|i| stub_release(&format!("v2025.1.{i}")))
                .collect(),
        )
        .to_string();
        let fixture = Fixture::start(move |_, _| Reply::ok(page.clone().into_bytes()));
        let err = fixture
            .client()
            .check(CURRENT, Platform::Linux, &cancel_flag())
            .unwrap_err();
        assert!(matches!(err, UpdateError::Limit(_)), "{err:?}");
        assert_eq!(fixture.hits().len(), MAX_RELEASE_PAGES);
    }

    #[test]
    fn check_never_offers_equal_or_lower_versions() {
        let platform = Platform::Linux;
        let same = standard_package(platform, CURRENT);
        let lower = standard_package(platform, "2026.10.9");
        let site = Site::default()
            .page(vec![full_release(&same), full_release(&lower)])
            .serve(&same)
            .serve(&lower);
        assert!(
            check_site(site.clone(), CURRENT, platform)
                .unwrap()
                .is_none()
        );
        // A newer installed version than anything published is not downgraded.
        assert!(check_site(site, "2027.1.0", platform).unwrap().is_none());
        // A prerelease of the same version is upgraded to the stable release.
        let stable = standard_package(platform, "2026.12.0");
        let site = Site::default()
            .page(vec![full_release(&stable)])
            .serve(&stable);
        assert!(
            check_site(site, "2026.12.0-rc.1", platform)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn check_without_valid_stable_releases_finds_nothing() {
        let site = Site::default().page(vec![
            stub_release("v2026.13.0"),
            stub_release("garbage"),
            json!({"tag_name": "v2099.1.0", "draft": true}),
            stub_release("v2099.2.0-rc.1"),
        ]);
        assert!(
            check_site(site, CURRENT, Platform::Linux)
                .unwrap()
                .is_none()
        );
        assert!(
            check_site(Site::default(), CURRENT, Platform::Linux)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            check_site(Site::default(), "2026.13.0", Platform::Linux),
            Err(UpdateError::InvalidVersion(_))
        ));
    }

    type Mutation = Box<dyn Fn(&mut Value)>;

    fn mutation(f: impl Fn(&mut Value) + 'static) -> Mutation {
        Box::new(f)
    }

    #[test]
    fn check_reports_incomplete_newest_release_instead_of_falling_back() {
        let platform = Platform::Linux;
        let newest = standard_package(platform, VER);
        let older = standard_package(platform, "2027.1.0");
        let mutations: Vec<(&str, Mutation)> = vec![
            (
                "no assets",
                mutation(|v| *v = stub_release(&format!("v{VER}"))),
            ),
            (
                "missing archive",
                mutation(|v| {
                    v["assets"].as_array_mut().unwrap().remove(0);
                }),
            ),
            (
                "missing checksums",
                mutation(|v| {
                    v["assets"].as_array_mut().unwrap().remove(1);
                }),
            ),
            (
                "duplicate archive",
                mutation(|v| {
                    let dup = v["assets"][0].clone();
                    v["assets"].as_array_mut().unwrap().push(dup);
                }),
            ),
            (
                "not uploaded",
                mutation(|v| v["assets"][0]["state"] = json!("starter")),
            ),
            ("zero size", mutation(|v| v["assets"][0]["size"] = json!(0))),
            (
                "archive too big",
                mutation(|v| v["assets"][0]["size"] = json!(MAX_ARCHIVE_BYTES + 1)),
            ),
            (
                "checksums too big",
                mutation(|v| v["assets"][1]["size"] = json!(MAX_CHECKSUM_BYTES + 1)),
            ),
            (
                "foreign download url",
                mutation(|v| {
                    v["assets"][0]["browser_download_url"] =
                        json!("https://evil.example/animatic.tar.gz")
                }),
            ),
            (
                "other repo url",
                mutation(|v| {
                    v["assets"][1]["browser_download_url"] = json!(format!(
                        "{{ORIGIN}}/Someone/else/releases/download/v{VER}/{CHECKSUMS_NAME}"
                    ))
                }),
            ),
            (
                "foreign release page",
                mutation(|v| v["html_url"] = json!("https://evil.example/release")),
            ),
            (
                "malformed digest",
                mutation(|v| v["assets"][0]["digest"] = json!("sha256:xyz")),
            ),
        ];
        for (what, mutate) in mutations {
            let mut release = full_release(&newest);
            mutate(&mut release);
            let site = Site::default()
                .page(vec![full_release(&older), release])
                .serve(&older)
                .serve(&newest);
            match check_site(site, CURRENT, platform) {
                Err(UpdateError::IncompleteRelease { tag, .. }) => {
                    assert_eq!(tag, format!("v{VER}"), "{what}")
                }
                other => panic!("{what}: expected IncompleteRelease, got {other:?}"),
            }
        }
    }

    #[test]
    fn check_maps_http_failures() {
        let respond = |status: u16| {
            let fixture = Fixture::start(move |_, _| Reply::status(status));
            fixture
                .client()
                .check(CURRENT, Platform::Linux, &cancel_flag())
        };
        assert!(matches!(respond(404), Err(UpdateError::NotFound)));
        assert!(matches!(respond(403), Err(UpdateError::RateLimited(403))));
        assert!(matches!(respond(429), Err(UpdateError::RateLimited(429))));
        assert!(matches!(respond(500), Err(UpdateError::Http(500))));
        let text = UpdateError::RateLimited(403).to_string();
        assert!(text.contains("try again later"), "{text}");
    }

    #[test]
    fn check_rejects_oversized_truncated_and_malformed_metadata() {
        let big = Fixture::start(|_, _| Reply::ok(vec![b' '; MAX_METADATA_BYTES as usize + 1]));
        assert!(matches!(
            big.client().check(CURRENT, Platform::Linux, &cancel_flag()),
            Err(UpdateError::TooLarge { .. })
        ));

        let truncated = Fixture::start(|_, _| {
            let mut reply = Reply::ok(b"[{\"tag_name\": \"v2099.1".to_vec());
            reply.declared_len = Some(500);
            reply
        });
        assert!(matches!(
            truncated
                .client()
                .check(CURRENT, Platform::Linux, &cancel_flag()),
            Err(UpdateError::Truncated { .. } | UpdateError::Network(_))
        ));

        let garbage = Fixture::start(|_, _| Reply::ok(b"{\"not\": \"a list\"}".to_vec()));
        assert!(matches!(
            garbage
                .client()
                .check(CURRENT, Platform::Linux, &cancel_flag()),
            Err(UpdateError::Metadata(_))
        ));

        let encoded =
            Fixture::start(|_, _| Reply::ok(b"[]".to_vec()).header("Content-Encoding", "gzip"));
        assert!(matches!(
            encoded
                .client()
                .check(CURRENT, Platform::Linux, &cancel_flag()),
            Err(UpdateError::Network(_))
        ));
    }

    #[test]
    fn check_honors_cancellation_before_any_request() {
        let fixture = Site::default().start();
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            fixture.client().check(CURRENT, Platform::Linux, &cancel),
            Err(UpdateError::Cancelled)
        ));
        assert!(fixture.hits().is_empty());
    }

    #[test]
    fn requests_are_anonymous_and_identify_the_app() {
        let fixture = Site::default().start();
        fixture
            .client()
            .check(CURRENT, Platform::Linux, &cancel_flag())
            .unwrap();
        let hits = fixture.hits();
        assert_eq!(hits.len(), 1);
        let head = hits[0].to_ascii_lowercase();
        assert!(head.contains("user-agent: animatic/"), "{head}");
        assert!(
            head.contains("accept: application/vnd.github+json"),
            "{head}"
        );
        for forbidden in ["authorization", "cookie", "proxy-authorization"] {
            assert!(!head.contains(forbidden), "{head}");
        }
    }

    #[test]
    fn redirects_are_followed_only_to_allowed_hosts() {
        let foreign = Fixture::start(|_, _| {
            Reply::status(302).header("Location", "https://evil.example/releases")
        });
        assert!(matches!(
            foreign
                .client()
                .check(CURRENT, Platform::Linux, &cancel_flag()),
            Err(UpdateError::Redirect(_))
        ));
        let downgrade = Fixture::start(|_, _| {
            Reply::status(302).header("Location", "http://github.com/releases")
        });
        assert!(matches!(
            downgrade
                .client()
                .check(CURRENT, Platform::Linux, &cancel_flag()),
            Err(UpdateError::Redirect(_))
        ));
        let looping = Fixture::start(|origin, path| {
            Reply::status(302).header("Location", &format!("{origin}{path}"))
        });
        assert!(matches!(
            looping
                .client()
                .check(CURRENT, Platform::Linux, &cancel_flag()),
            Err(UpdateError::Redirect(_))
        ));
        let nowhere = Fixture::start(|_, _| Reply::status(302));
        assert!(matches!(
            nowhere
                .client()
                .check(CURRENT, Platform::Linux, &cancel_flag()),
            Err(UpdateError::Redirect(_))
        ));

        let newest = standard_package(Platform::Linux, VER);
        let site = Site::default()
            .page(vec![full_release(&newest)])
            .serve(&newest);
        let fixture = Fixture::start(move |origin, path| {
            if path.starts_with("/repos/") {
                Reply::status(302).header("Location", "/moved")
            } else if path == "/moved" {
                site.reply(
                    origin,
                    "/repos/RetroHazard/animatic/releases?per_page=30&page=1",
                )
            } else {
                site.reply(origin, path)
            }
        });
        let release = fixture
            .client()
            .check(CURRENT, Platform::Linux, &cancel_flag())
            .unwrap();
        assert!(release.is_some());
        assert!(fixture.hits().iter().any(|h| h.starts_with("GET /moved")));
    }

    // ----- anonymous public access -----

    #[test]
    fn downloads_use_canonical_browser_urls_and_carry_no_credentials() {
        let platform = Platform::Linux;
        let pkg = standard_package(platform, VER);
        let fixture = Site::default()
            .page(vec![full_release(&pkg)])
            .serve(&pkg)
            .start();
        let client = fixture.client();
        let cancel = cancel_flag();
        let release = client.check(CURRENT, platform, &cancel).unwrap().unwrap();
        client.download(&release, &cancel, &mut |_, _| {}).unwrap();
        let hits = fixture.hits();
        assert!(
            hits.iter()
                .any(|h| h.contains("/releases/download/v2099.1.0/"))
        );
        for head in &hits {
            let lower = head.to_ascii_lowercase();
            assert!(!lower.contains("\r\nauthorization:"), "{head}");
            assert!(!lower.contains("\r\ncookie:"), "{head}");
            assert!(!head.contains("/releases/assets/"), "{head}");
        }
    }

    // ----- download and verification -----

    #[test]
    fn download_verifies_and_stages_exactly_the_three_files() {
        for platform in BOTH {
            let pkg = standard_package(platform, VER);
            let fixture = Site::default()
                .page(vec![full_release(&pkg)])
                .serve(&pkg)
                .start();
            let client = fixture.client();
            let cancel = cancel_flag();
            let release = client.check(CURRENT, platform, &cancel).unwrap().unwrap();
            let mut calls: Vec<(u64, u64)> = Vec::new();
            let update = client
                .download(&release, &cancel, &mut |done, total| {
                    calls.push((done, total))
                })
                .unwrap();
            assert_eq!(update.version(), VER);
            assert_eq!(update.sha256(), sha256_hex(&pkg.archive));
            assert_eq!(calls.first(), Some(&(0, pkg.archive.len() as u64)));
            assert_eq!(
                calls.last(),
                Some(&(pkg.archive.len() as u64, pkg.archive.len() as u64))
            );
            assert!(calls.windows(2).all(|w| w[0].0 <= w[1].0));

            let exe = &update.files.exe;
            assert_eq!(fs::read(&exe.path).unwrap(), fake_exe(platform));
            assert_eq!(fs::read(&update.files.license.path).unwrap(), LICENSE_TEXT);
            assert_eq!(fs::read(&update.files.notices.path).unwrap(), NOTICES_TEXT);
            let mut names: Vec<String> = fs::read_dir(exe.path.parent().unwrap())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            let mut expected = vec![
                platform.exe_name().to_owned(),
                "LICENSE".to_owned(),
                "THIRD-PARTY-NOTICES.html".to_owned(),
            ];
            expected.sort();
            assert_eq!(
                names, expected,
                "documents must be validated, not extracted"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(&exe.path).unwrap().permissions().mode();
                assert_eq!(mode & 0o111, 0o111);
            }
            let root = exe.path.parent().unwrap().parent().unwrap().to_path_buf();
            drop(update);
            assert!(!root.exists(), "the staging directory is removed on drop");
        }
    }

    #[test]
    fn download_accepts_deflated_zip_entries_and_matching_github_digests() {
        let platform = Platform::Windows;
        let mut entries = standard_entries(platform, VER, &fake_exe(platform));
        for e in &mut entries {
            e.deflate = true;
        }
        let pkg = package(platform, VER, &entries);
        let mut release = full_release(&pkg);
        release["assets"][0]["digest"] = json!(format!("sha256:{}", sha256_hex(&pkg.archive)));
        release["assets"][1]["digest"] = json!(format!("sha256:{}", sha256_hex(&pkg.sums)));
        let fixture = Site::default().page(vec![release]).serve(&pkg).start();
        let client = fixture.client();
        let info = client
            .check(CURRENT, platform, &cancel_flag())
            .unwrap()
            .unwrap();
        let update = client
            .download(&info, &cancel_flag(), &mut |_, _| {})
            .unwrap();
        assert_eq!(
            fs::read(&update.files.exe.path).unwrap(),
            fake_exe(platform)
        );
    }

    /// Runs check + download against a site whose files are altered by `tweak`.
    fn download_with(
        pkg: &Package,
        release: Value,
        tweak: impl Fn(&str, &mut Reply) + Send + Sync + 'static,
    ) -> Result<VerifiedUpdate> {
        let site = Site::default().page(vec![release]).serve(pkg);
        let fixture = Fixture::start(move |origin, path| {
            let mut reply = site.reply(origin, path);
            tweak(path, &mut reply);
            reply
        });
        let client = fixture.client();
        let info = client
            .check(CURRENT, pkg.platform, &cancel_flag())
            .unwrap()
            .unwrap();
        client.download(&info, &cancel_flag(), &mut |_, _| {})
    }

    #[test]
    fn download_rejects_checksum_problems() {
        let platform = Platform::Linux;
        let pkg = standard_package(platform, VER);
        let ok_release = || full_release(&pkg);

        // SHA256SUMS lists another hash.
        let wrong = format!("{}  {}\n", "0".repeat(64), pkg.name).into_bytes();
        let r = download_with(
            &pkg,
            release_with_sums_size(&pkg, wrong.len()),
            move |p, reply| {
                if p.ends_with(CHECKSUMS_NAME) {
                    reply.body = wrong.clone();
                }
            },
        );
        assert!(matches!(r, Err(UpdateError::Checksum(_))), "{r:?}");

        // Archive bytes differ from the listed hash (same length).
        let r = download_with(&pkg, ok_release(), |p, reply| {
            if p.ends_with(".tar.gz") {
                let last = reply.body.len() - 1;
                reply.body[last] ^= 0xff;
            }
        });
        assert!(matches!(r, Err(UpdateError::Checksum(_))), "{r:?}");

        // Malformed, duplicate and missing entries.
        let h = sha256_hex(&pkg.archive);
        for text in [
            format!("{h} {}\n", pkg.name),
            format!("{h}  {n}\n{h}  {n}\n", n = pkg.name),
            format!("{h}  other.zip\n"),
        ] {
            let bytes = text.into_bytes();
            let len = bytes.len();
            let r = download_with(&pkg, release_with_sums_size(&pkg, len), move |p, reply| {
                if p.ends_with(CHECKSUMS_NAME) {
                    reply.body = bytes.clone();
                }
            });
            assert!(matches!(r, Err(UpdateError::Checksum(_))), "{r:?}");
        }

        // GitHub digests that disagree with the bytes or with SHA256SUMS.
        let mut release = ok_release();
        release["assets"][0]["digest"] = json!(format!("sha256:{}", "1".repeat(64)));
        let r = download_with(&pkg, release, |_, _| {});
        assert!(matches!(r, Err(UpdateError::Checksum(_))), "{r:?}");
        let mut release = ok_release();
        release["assets"][1]["digest"] = json!(format!("sha256:{}", "2".repeat(64)));
        let r = download_with(&pkg, release, |_, _| {});
        assert!(matches!(r, Err(UpdateError::Checksum(_))), "{r:?}");
    }

    fn release_with_sums_size(pkg: &Package, sums_len: usize) -> Value {
        let mut release = full_release(pkg);
        release["assets"][1]["size"] = json!(sums_len);
        release
    }

    #[test]
    fn download_rejects_truncated_and_resized_archives() {
        let platform = Platform::Linux;
        let pkg = standard_package(platform, VER);

        let r = download_with(&pkg, full_release(&pkg), |p, reply| {
            if p.ends_with(".tar.gz") {
                reply.declared_len = Some(reply.body.len() + 1000);
            }
        });
        // The exact error category is incidental: a wrong or truncated size must never
        // produce a verified update.
        assert!(
            r.is_err(),
            "an oversized declared length must be rejected: {r:?}"
        );

        let r = download_with(&pkg, full_release(&pkg), |p, reply| {
            if p.ends_with(".tar.gz") {
                let keep = reply.body.len() - 10;
                reply.body.truncate(keep);
            }
        });
        assert!(r.is_err(), "a short body must be rejected: {r:?}");

        let r = download_with(&pkg, full_release(&pkg), |p, reply| {
            if p.ends_with(".tar.gz") {
                reply.body.extend_from_slice(&[0u8; 10]);
            }
        });
        assert!(r.is_err(), "a long body must be rejected: {r:?}");

        let r = download_with(&pkg, full_release(&pkg), |p, reply| {
            if p.ends_with(".tar.gz") {
                *reply = Reply::status(404);
            }
        });
        assert!(matches!(r, Err(UpdateError::NotFound)), "{r:?}");
        let r = download_with(&pkg, full_release(&pkg), |p, reply| {
            if p.ends_with(".tar.gz") {
                *reply = Reply::status(429);
            }
        });
        assert!(matches!(r, Err(UpdateError::RateLimited(429))), "{r:?}");
    }

    #[test]
    fn download_can_be_cancelled_between_chunks() {
        let platform = Platform::Linux;
        let pkg = package(
            platform,
            VER,
            &standard_entries(
                platform,
                VER,
                &[fake_exe(platform), noise(400_000)].concat(),
            ),
        );
        assert!(pkg.archive.len() > 3 * CHUNK);
        let fixture = Site::default()
            .page(vec![full_release(&pkg)])
            .serve(&pkg)
            .start();
        let client = fixture.client();
        let cancel = cancel_flag();
        let info = client.check(CURRENT, platform, &cancel).unwrap().unwrap();
        let mut seen = 0;
        let result = client.download(&info, &cancel, &mut |done, _| {
            seen = done;
            if done > 0 {
                cancel.store(true, Ordering::Release);
            }
        });
        assert!(matches!(result, Err(UpdateError::Cancelled)), "{result:?}");
        assert!(seen < pkg.archive.len() as u64);
    }

    // ----- archive policy -----

    #[test]
    fn archives_with_unsafe_names_are_rejected() {
        for platform in BOTH {
            let stem = platform.stem(VER);
            let base = standard_entries(platform, VER, &fake_exe(platform));
            for name in [
                format!("{stem}/../evil"),
                "/etc/evil".to_owned(),
                "../evil".to_owned(),
                format!("{stem}\\evil"),
                format!("other-root/{}", platform.exe_name()),
                format!("{stem}/sub/dir/evil"),
                format!("{stem}/extra.txt"),
                format!("{stem}/c:evil"),
            ] {
                let mut entries = base.clone();
                entries.push(Entry::file(&name, b"x"));
                assert_archive_rejected(platform, entries, &format!("name {name}"));
            }
        }
    }

    #[test]
    fn archives_with_links_are_rejected() {
        for platform in BOTH {
            let stem = platform.stem(VER);
            let mut entries = standard_entries(platform, VER, &fake_exe(platform));
            entries.push(Entry::link(
                &format!("{stem}/docs/link.md"),
                Kind::Symlink("/etc/passwd".to_owned()),
            ));
            assert_archive_rejected(platform, entries, "symlink");

            // The executable itself replaced by a symlink.
            let mut entries = standard_entries(platform, VER, &fake_exe(platform));
            let exe = format!("{stem}/{}", platform.exe_name());
            entries.retain(|e| e.name != exe);
            entries.push(Entry::link(&exe, Kind::Symlink("/bin/sh".to_owned())));
            assert_archive_rejected(platform, entries, "symlinked executable");
        }
        let stem = Platform::Linux.stem(VER);
        let mut entries = standard_entries(Platform::Linux, VER, &fake_exe(Platform::Linux));
        entries.push(Entry::link(
            &format!("{stem}/docs/hard.md"),
            Kind::Hardlink(format!("{stem}/animatic")),
        ));
        assert_archive_rejected(Platform::Linux, entries, "hardlink");
    }

    #[test]
    fn archives_with_duplicates_are_rejected() {
        for platform in BOTH {
            let stem = platform.stem(VER);
            for name in [
                platform.exe_name().to_owned(),
                "LICENSE".to_owned(),
                "THIRD-PARTY-NOTICES.html".to_owned(),
                "docs/guide.md".to_owned(),
            ] {
                let mut entries = standard_entries(platform, VER, &fake_exe(platform));
                entries.push(Entry::file(&format!("{stem}/{name}"), b"second"));
                assert_archive_rejected(platform, entries, &format!("duplicate {name}"));
            }
        }
    }

    #[test]
    fn archives_missing_required_files_are_rejected() {
        for platform in BOTH {
            let stem = platform.stem(VER);
            for name in [platform.exe_name(), "LICENSE", "THIRD-PARTY-NOTICES.html"] {
                let mut entries = standard_entries(platform, VER, &fake_exe(platform));
                let full = format!("{stem}/{name}");
                entries.retain(|e| e.name != full);
                assert_archive_rejected(platform, entries, &format!("missing {name}"));
            }
        }
    }

    #[test]
    fn archives_with_bad_executables_are_rejected() {
        for platform in BOTH {
            for exe in [
                b"not an executable at all".to_vec(),
                Vec::new(),
                fake_exe(match platform {
                    Platform::Windows => Platform::Linux,
                    Platform::Linux => Platform::Windows,
                }),
            ] {
                let entries = standard_entries(platform, VER, &exe);
                assert_archive_rejected(platform, entries, "wrong executable format");
            }
        }
        // A PE header whose machine type is not x86-64.
        let mut arm = fake_exe(Platform::Windows);
        arm[68] = 0x64;
        arm[69] = 0xaa;
        assert_archive_rejected(
            Platform::Windows,
            standard_entries(Platform::Windows, VER, &arm),
            "arm64 pe",
        );
        // A Linux executable that is not marked executable in the archive.
        let platform = Platform::Linux;
        let mut entries = standard_entries(platform, VER, &fake_exe(platform));
        for e in &mut entries {
            if e.name.ends_with("/animatic") {
                e.perm = 0o644;
            }
        }
        assert_archive_rejected(platform, entries, "no exec bit");
    }

    #[test]
    fn archives_declaring_oversized_or_too_many_entries_are_rejected() {
        for platform in BOTH {
            let stem = platform.stem(VER);
            let mut entries = standard_entries(platform, VER, &fake_exe(platform));
            entries.insert(
                0,
                Entry {
                    declared: Some(MAX_EXECUTABLE_BYTES + 1),
                    ..Entry::file(&format!("{stem}/docs/huge.md"), b"")
                },
            );
            assert_archive_rejected(platform, entries, "oversized ignored document");

            let mut entries = standard_entries(platform, VER, &fake_exe(platform));
            entries.push(Entry {
                declared: Some(MAX_LICENSE_BYTES + 1),
                ..Entry::file(&format!("{stem}/README.md.big"), b"")
            });
            let mut entries2 = standard_entries(platform, VER, &fake_exe(platform));
            for i in 0..=MAX_ENTRIES {
                entries2.push(Entry::file(&format!("{stem}/docs/f{i}.md"), b"x"));
            }
            assert_archive_rejected(platform, entries, "bad extra entry");
            assert_archive_rejected(platform, entries2, "entry count bomb");
        }
        // The executable entry claims more bytes than the archive carries.
        let platform = Platform::Linux;
        let stem = platform.stem(VER);
        let mut entries = standard_entries(platform, VER, &fake_exe(platform));
        for e in &mut entries {
            if e.name == format!("{stem}/animatic") {
                e.declared = Some(e.data.len() as u64 + 4096);
            }
        }
        assert_archive_rejected(platform, entries, "short executable entry");
    }

    #[test]
    fn encrypted_zip_entries_are_rejected() {
        let platform = Platform::Windows;
        let mut entries = standard_entries(platform, VER, &fake_exe(platform));
        entries[1].encrypted = true;
        assert_archive_rejected(platform, entries, "encrypted entry");
    }

    #[test]
    fn corrupt_archives_are_rejected_after_checksum_passes() {
        for platform in BOTH {
            let pkg = {
                let name = platform.archive_name(VER);
                let archive = b"this is not an archive".to_vec();
                let sums = format!("{}  {name}\n", sha256_hex(&archive)).into_bytes();
                Package {
                    platform,
                    version: VER.to_owned(),
                    name,
                    archive,
                    sums,
                }
            };
            let fixture = Site::default()
                .page(vec![full_release(&pkg)])
                .serve(&pkg)
                .start();
            let client = fixture.client();
            let info = client
                .check(CURRENT, platform, &cancel_flag())
                .unwrap()
                .unwrap();
            let r = client.download(&info, &cancel_flag(), &mut |_, _| {});
            assert!(
                matches!(r, Err(UpdateError::Archive(_))),
                "{platform:?}: {r:?}"
            );
        }
    }

    // ----- installation (no real self-replace) -----

    struct InstallFixture {
        // Declared first so the open handles are dropped before the folder is removed.
        target: InstallationTarget,
        update: Option<VerifiedUpdate>,
        exe: PathBuf,
        dir: PathBuf,
        _root: TempDir,
    }

    const OLD_EXE: &[u8] = b"old executable";

    fn install_fixture() -> InstallFixture {
        let platform = Platform::Linux;
        let update = try_download(
            platform,
            standard_entries(platform, VER, &fake_exe(platform)),
        )
        .expect("staging");
        let root = tempfile::tempdir().unwrap();
        let dir = fs::canonicalize(root.path()).unwrap();
        let exe = dir.join(if cfg!(windows) {
            "animatic-fake.exe"
        } else {
            "animatic-fake"
        });
        fs::write(&exe, OLD_EXE).unwrap();
        let target = InstallationTarget::capture_path(&exe).unwrap();
        InstallFixture {
            target,
            update: Some(update),
            exe,
            dir,
            _root: root,
        }
    }

    impl InstallFixture {
        fn staged_exe(&self) -> Vec<u8> {
            fs::read(&self.update.as_ref().unwrap().files.exe.path).unwrap()
        }

        fn install(
            &mut self,
            backup: bool,
            replace: impl FnOnce(&Path) -> io::Result<()>,
        ) -> Result<String> {
            let exe = self.exe.clone();
            self.update
                .take()
                .unwrap()
                .install_with(&self.target, &exe, backup, replace)
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }

        fn license(&self) -> PathBuf {
            self.dir.join(format!("LICENSE-v{VER}"))
        }

        fn notices(&self) -> PathBuf {
            self.dir.join(format!("THIRD-PARTY-NOTICES-v{VER}.html"))
        }
    }

    fn copy_over(exe: &Path) -> impl FnOnce(&Path) -> io::Result<()> + '_ {
        move |staged| fs::copy(staged, exe).map(|_| ())
    }

    #[test]
    fn install_adds_versioned_files_replaces_and_cleans_up() {
        let mut fx = install_fixture();
        let new_exe = fx.staged_exe();
        let exe = fx.exe.clone();
        let version = fx.install(true, copy_over(&exe)).unwrap();
        assert_eq!(version, VER);
        assert_eq!(fs::read(&fx.exe).unwrap(), new_exe);
        assert_eq!(fs::read(fx.license()).unwrap(), LICENSE_TEXT);
        assert_eq!(fs::read(fx.notices()).unwrap(), NOTICES_TEXT);
        let lock = fx.dir.join(LOCK_NAME);
        assert_eq!(
            fs::metadata(&lock).unwrap().len(),
            0,
            "the lock file stays empty"
        );
        let names = fx.names();
        assert_eq!(
            names.len(),
            4,
            "no recovery copy or temp files remain: {names:?}"
        );
    }

    #[test]
    fn install_accepts_identical_existing_license_files() {
        let mut fx = install_fixture();
        fs::write(fx.license(), LICENSE_TEXT).unwrap();
        fs::write(fx.notices(), NOTICES_TEXT).unwrap();
        let exe = fx.exe.clone();
        fx.install(false, copy_over(&exe)).unwrap();
        assert_eq!(fs::read(&fx.exe).unwrap(), fake_exe(Platform::Linux));
    }

    #[test]
    fn install_never_overwrites_different_license_files() {
        for target in ["license", "notices"] {
            let mut fx = install_fixture();
            let path = if target == "license" {
                fx.license()
            } else {
                fx.notices()
            };
            fs::write(&path, b"someone else's file").unwrap();
            let called = AtomicBool::new(false);
            let result = fx.install(false, |_| {
                called.store(true, Ordering::Relaxed);
                Ok(())
            });
            let err = result.unwrap_err();
            assert!(matches!(err, UpdateError::Refused(_)), "{err:?}");
            assert!(!err.install_attempted());
            assert!(
                !called.load(Ordering::Relaxed),
                "the executable must not be replaced"
            );
            assert_eq!(fs::read(&path).unwrap(), b"someone else's file");
            assert_eq!(fs::read(&fx.exe).unwrap(), OLD_EXE);
        }
    }

    #[cfg(unix)]
    #[test]
    fn install_refuses_symlinked_license_files() {
        let mut fx = install_fixture();
        let elsewhere = fx.dir.join("elsewhere");
        fs::write(&elsewhere, LICENSE_TEXT).unwrap();
        std::os::unix::fs::symlink(&elsewhere, fx.license()).unwrap();
        let err = fx.install(false, |_| Ok(())).unwrap_err();
        assert!(matches!(err, UpdateError::Refused(_)), "{err:?}");
        assert!(
            fs::symlink_metadata(fx.license())
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn install_refuses_a_different_running_executable() {
        let mut fx = install_fixture();
        let other = fx.dir.join("other-binary");
        fs::write(&other, OLD_EXE).unwrap();
        let update = fx.update.take().unwrap();
        let err = update
            .install_with(&fx.target, &other, false, |_| Ok(()))
            .unwrap_err();
        assert!(matches!(err, UpdateError::Refused(_)), "{err:?}");
        assert!(!fx.license().exists() && !fx.notices().exists());
        assert_eq!(fs::read(&fx.exe).unwrap(), OLD_EXE);
    }

    #[test]
    fn install_refuses_when_the_executable_was_replaced_since_startup() {
        let mut fx = install_fixture();
        // Another installer swapped the file at the path: same name, different file.
        let aside = fx.dir.join("moved-aside");
        let fresh = fx.dir.join("fresh");
        fs::write(&fresh, b"updated by someone else").unwrap();
        fs::rename(&fx.exe, &aside).unwrap();
        fs::rename(&fresh, &fx.exe).unwrap();
        let called = AtomicBool::new(false);
        let err = fx
            .install(true, |_| {
                called.store(true, Ordering::Relaxed);
                Ok(())
            })
            .unwrap_err();
        assert!(matches!(err, UpdateError::Refused(_)), "{err:?}");
        assert!(!err.install_attempted());
        assert!(!called.load(Ordering::Relaxed));
        assert!(!fx.license().exists());
        assert_eq!(fs::read(&fx.exe).unwrap(), b"updated by someone else");
    }

    #[test]
    fn install_refuses_a_missing_executable() {
        let mut fx = install_fixture();
        let aside = fx.dir.join("moved-aside");
        fs::rename(&fx.exe, &aside).unwrap();
        let err = fx.install(false, |_| Ok(())).unwrap_err();
        assert!(matches!(err, UpdateError::Refused(_)), "{err:?}");
    }

    #[test]
    fn install_is_serialized_by_the_lock_file() {
        let mut fx = install_fixture();
        let held = InstallLock::acquire(&fx.dir).unwrap();
        let called = AtomicBool::new(false);
        let err = fx
            .install(false, |_| {
                called.store(true, Ordering::Relaxed);
                Ok(())
            })
            .unwrap_err();
        assert!(matches!(err, UpdateError::Busy), "{err:?}");
        assert!(!called.load(Ordering::Relaxed));
        assert!(!fx.license().exists());
        drop(held);
        // Released on drop: a new acquisition works.
        drop(InstallLock::acquire(&fx.dir).unwrap());
    }

    #[test]
    fn install_rejects_a_lock_path_that_is_not_a_regular_file() {
        let mut fx = install_fixture();
        fs::create_dir(fx.dir.join(LOCK_NAME)).unwrap();
        let err = fx.install(false, |_| Ok(())).unwrap_err();
        assert!(matches!(err, UpdateError::Refused(_)), "{err:?}");
        assert!(!fx.license().exists());
    }

    #[cfg(unix)]
    #[test]
    fn install_rejects_a_symlinked_lock_path() {
        let mut fx = install_fixture();
        let elsewhere = fx.dir.join("elsewhere");
        fs::write(&elsewhere, b"").unwrap();
        std::os::unix::fs::symlink(&elsewhere, fx.dir.join(LOCK_NAME)).unwrap();
        let err = fx.install(false, |_| Ok(())).unwrap_err();
        assert!(matches!(err, UpdateError::Refused(_)), "{err:?}");
    }

    #[test]
    fn install_detects_staged_files_changed_after_verification() {
        let mut fx = install_fixture();
        let staged = fx.update.as_ref().unwrap().files.exe.path.clone();
        fs::OpenOptions::new()
            .append(true)
            .open(&staged)
            .unwrap()
            .write_all(b"tampered")
            .unwrap();
        let err = fx.install(false, |_| Ok(())).unwrap_err();
        assert!(matches!(err, UpdateError::Checksum(_)), "{err:?}");
        assert!(!fx.license().exists());
    }

    #[test]
    fn a_failed_replacement_is_reported_as_attempted_and_keeps_the_recovery_copy() {
        let mut fx = install_fixture();
        let err = fx
            .install(true, |_| {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected"))
            })
            .unwrap_err();
        assert!(err.install_attempted(), "{err:?}");
        let UpdateError::Replace {
            cause,
            recovery,
            backup,
        } = &err
        else {
            panic!("{err:?}");
        };
        assert!(cause.contains("injected"));
        let backup = backup.clone().expect("a recovery copy exists");
        assert!(
            recovery.contains(&backup.display().to_string()),
            "{recovery}"
        );
        assert_eq!(fs::read(&backup).unwrap(), OLD_EXE);
        assert_eq!(
            fs::read(&fx.exe).unwrap(),
            OLD_EXE,
            "an untouched original stays"
        );
        // Versioned licenses may remain; that is harmless.
        assert!(fx.license().exists());
    }

    #[test]
    fn a_failed_replacement_restores_a_missing_original_without_removing_the_copy() {
        let mut fx = install_fixture();
        let exe = fx.exe.clone();
        let err = fx
            .install(true, |_| {
                // What self-replace does on Windows before it fails: the old file is moved away.
                fs::rename(&exe, exe.with_extension("moved"))?;
                Err(io::Error::other("injected after rename"))
            })
            .unwrap_err();
        assert!(err.install_attempted());
        let UpdateError::Replace {
            recovery, backup, ..
        } = &err
        else {
            panic!("{err:?}");
        };
        assert!(recovery.contains("restored"), "{recovery}");
        assert_eq!(fs::read(&fx.exe).unwrap(), OLD_EXE);
        let backup = backup.clone().unwrap();
        assert_eq!(
            fs::read(&backup).unwrap(),
            OLD_EXE,
            "the recovery copy is always kept"
        );
        assert!(
            !fx.names()
                .iter()
                .any(|n| n.starts_with(".animatic-restore-")),
            "{:?}",
            fx.names()
        );
    }

    #[test]
    fn restore_never_overwrites_a_file_another_writer_created() {
        let mut fx = install_fixture();
        let exe = fx.exe.clone();
        let err = fx
            .install(true, |_| {
                fs::rename(&exe, exe.with_extension("moved"))?;
                fs::write(&exe, b"another writer's file")?;
                Err(io::Error::other("injected"))
            })
            .unwrap_err();
        let UpdateError::Replace {
            recovery, backup, ..
        } = &err
        else {
            panic!("{err:?}");
        };
        assert!(recovery.contains("not overwritten"), "{recovery}");
        assert_eq!(fs::read(&fx.exe).unwrap(), b"another writer's file");
        assert_eq!(fs::read(backup.as_ref().unwrap()).unwrap(), OLD_EXE);
    }

    #[test]
    fn a_failed_restore_reports_the_manual_recovery_path() {
        let mut fx = install_fixture();
        let exe = fx.exe.clone();
        let dir = fx.dir.clone();
        let err = fx
            .install(true, |_| {
                fs::rename(&exe, exe.with_extension("moved"))?;
                // Make the recovery copy unreadable so that the automatic restore fails.
                for entry in fs::read_dir(&dir)? {
                    let path = entry?.path();
                    let name = path.file_name().unwrap().to_string_lossy().into_owned();
                    if name.starts_with(".animatic-recovery-") {
                        fs::remove_file(&path)?;
                    }
                }
                Err(io::Error::other("injected"))
            })
            .unwrap_err();
        let UpdateError::Replace {
            recovery, backup, ..
        } = &err
        else {
            panic!("{err:?}");
        };
        let backup = backup.as_ref().unwrap();
        assert!(recovery.contains("manually"), "{recovery}");
        assert!(
            recovery.contains(&backup.display().to_string()),
            "{recovery}"
        );
        assert!(recovery.contains(&exe.display().to_string()), "{recovery}");
        assert!(!exe.exists());
    }

    #[test]
    fn without_a_recovery_copy_a_failure_is_still_reported_as_attempted() {
        let mut fx = install_fixture();
        let err = fx
            .install(false, |_| Err(io::Error::other("injected")))
            .unwrap_err();
        let UpdateError::Replace {
            backup, recovery, ..
        } = &err
        else {
            panic!("{err:?}");
        };
        assert!(backup.is_none());
        assert!(recovery.contains("unchanged"), "{recovery}");
        assert!(err.install_attempted());
        assert_eq!(
            fx.names()
                .iter()
                .filter(|n| n.starts_with(".animatic-recovery-"))
                .count(),
            0
        );
    }

    #[test]
    fn only_replacement_failures_count_as_install_attempted() {
        assert!(!UpdateError::Busy.install_attempted());
        assert!(!UpdateError::Refused("x".into()).install_attempted());
        assert!(!UpdateError::NotWritable("x".into()).install_attempted());
        assert!(!UpdateError::Cancelled.install_attempted());
        assert!(!UpdateError::Io(io::Error::other("x")).install_attempted());
    }

    // ----- real replacement in a child process -----

    const MODE: &str = "ANIMATIC_UPDATE_TEST_MODE";
    const ORIGIN: &str = "ANIMATIC_UPDATE_TEST_ORIGIN";
    const TARGET: &str = "ANIMATIC_UPDATE_TEST_TARGET";
    const PARENT: &str = "ANIMATIC_UPDATE_TEST_PARENT";
    const MARKER: &str = "ANIMATIC_UPDATE_TEST_MARKER";
    const TEST_NAME: &str = "update::tests::real_self_replacement_in_child_process";

    fn required_env(name: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| panic!("missing test variable {name}"))
    }

    /// One test, three roles chosen by a test-only environment variable that exists only in
    /// the processes this test spawns: the parent prepares everything and spawns a COPY of
    /// the test executable (child, `install`), which replaces itself through the real
    /// `VerifiedUpdate::install`; the parent then runs the replaced executable again
    /// (`probe`) to prove it launches and is the new binary.
    #[test]
    fn real_self_replacement_in_child_process() {
        match std::env::var(MODE).as_deref() {
            Ok("install") => child_install(),
            Ok("probe") => child_probe(),
            Ok(other) => panic!("unknown mode {other}"),
            Err(_) => parent_drives_replacement(),
        }
    }

    fn child_install() {
        let target = PathBuf::from(required_env(TARGET));
        let parent = PathBuf::from(required_env(PARENT));
        let me = fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        assert_eq!(
            me,
            fs::canonicalize(&target).unwrap(),
            "must run from the copied executable"
        );
        assert_ne!(
            me,
            fs::canonicalize(&parent).unwrap(),
            "must not be the parent's executable"
        );

        let platform = Platform::current().expect("supported platform");
        let client =
            UpdateClient::with_endpoints(Endpoints::loopback(&required_env(ORIGIN))).unwrap();
        let cancel = cancel_flag();
        let release = client
            .check(env!("CARGO_PKG_VERSION"), platform, &cancel)
            .unwrap()
            .expect("the fixture release is newer");
        assert_eq!(release.version, VER);
        let verified = client.download(&release, &cancel, &mut |_, _| {}).unwrap();
        let installation = InstallationTarget::capture().unwrap();
        assert_eq!(installation.path(), me);
        let installed = verified.install(&installation).unwrap();
        assert_eq!(installed, VER);
    }

    fn child_probe() {
        let target = PathBuf::from(required_env(TARGET));
        let me = fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        assert_eq!(me, fs::canonicalize(&target).unwrap());
        let bytes = fs::read(&me).unwrap();
        let marker = required_env(MARKER);
        assert!(
            bytes.ends_with(marker.as_bytes()),
            "the replaced executable carries the overlay marker"
        );
    }

    fn run_child(target: &Path, mode: &str, log: &Path, envs: &[(&str, String)]) -> bool {
        let out = File::create(log).unwrap();
        let err = out.try_clone().unwrap();
        let status = Command::new(target)
            .args([TEST_NAME, "--exact", "--nocapture"])
            .env(MODE, mode)
            .envs(envs.iter().map(|(k, v)| (*k, v.as_str())))
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .status()
            .unwrap();
        status.success()
    }

    fn parent_drives_replacement() {
        let Some(platform) = Platform::current() else {
            return;
        };
        let parent_exe = std::env::current_exe().unwrap();
        let root = tempfile::tempdir().unwrap();
        let install_dir = root.path().join("install");
        fs::create_dir(&install_dir).unwrap();
        let target = install_dir.join(if cfg!(windows) {
            "animatic-update-probe.exe"
        } else {
            "animatic-update-probe"
        });
        fs::copy(&parent_exe, &target).unwrap();
        let (_, old_hash) = file_sha256(&target).unwrap();

        // The "new release" is this very test executable plus a harmless overlay.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let marker = format!("ANIMATIC-UPDATE-OVERLAY-{}-{nanos}", std::process::id());
        let mut new_bytes = fs::read(&parent_exe).unwrap();
        new_bytes.extend_from_slice(marker.as_bytes());
        let new_hash = sha256_hex(&new_bytes);
        assert_ne!(old_hash, new_hash);

        let pkg = package(platform, VER, &standard_entries(platform, VER, &new_bytes));
        let fixture = Site::default()
            .page(vec![full_release(&pkg)])
            .serve(&pkg)
            .start();
        drop(new_bytes);

        let envs = [
            (ORIGIN, fixture.origin.clone()),
            (TARGET, target.display().to_string()),
            (PARENT, parent_exe.display().to_string()),
            (MARKER, marker.clone()),
        ];
        let install_log = root.path().join("install.log");
        let installed = run_child(&target, "install", &install_log, &envs);
        assert!(
            installed,
            "the child failed:\n{}",
            fs::read_to_string(&install_log).unwrap_or_default()
        );

        let (_, hash_after) = file_sha256(&target).unwrap();
        assert_eq!(
            hash_after, new_hash,
            "the executable was replaced by the new binary"
        );
        assert_eq!(
            fs::read(install_dir.join(format!("LICENSE-v{VER}"))).unwrap(),
            LICENSE_TEXT
        );
        assert_eq!(
            fs::read(install_dir.join(format!("THIRD-PARTY-NOTICES-v{VER}.html"))).unwrap(),
            NOTICES_TEXT
        );

        let probe_log = root.path().join("probe.log");
        let launched = run_child(&target, "probe", &probe_log, &envs);
        assert!(
            launched,
            "the replaced executable failed to launch:\n{}",
            fs::read_to_string(&probe_log).unwrap_or_default()
        );
        // On Windows the helper that removes the old file may still hold the folder briefly;
        // the temporary directory is cleaned up best-effort when `root` is dropped.
    }
}
