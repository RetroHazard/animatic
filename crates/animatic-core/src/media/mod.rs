//! Opt-in media properties through the external `ffprobe` tool (FFmpeg).
//!
//! Animatic never reads media properties on its own. The user enables *Read media properties* in
//! Options, which stays disabled until an `ffprobe` executable is available. Everything here is
//! split by responsibility:
//!
//! * [`tool`]: finding `ffprobe` (configured path, Animatic's own copy, `PATH`) and its version.
//! * [`install`]: the one-click install of a pinned FFmpeg build into the data folder
//!   (confirmed, anonymous, size-limited, SHA-256 verified, only `ffprobe` and the license kept).
//! * [`probe`]: running `ffprobe` on one file (argument vector, no shell, timeout, bounded output)
//!   and parsing its JSON into [`MediaInfo`].
//! * [`store`]: the `media_info` table, and [`read_files`], the batch that fills it.
//!
//! Probing is a separate step after hashing and never part of the hash read: `ffprobe` opens the
//! container and reads its headers (and index), it does not read the file a second time from
//! start to end.
//!
//! # Trust model of the install
//!
//! The pinned URL points at a dated, retained release asset of the BtbN FFmpeg-Builds project on
//! GitHub, and the expected SHA-256 and size are constants in this crate (see
//! [`install::WINDOWS_X64`] and [`install::LINUX_X64`]). A download whose size or SHA-256 differs
//! is refused before the archive is opened. This proves the file is the one that was pinned; it
//! does not authenticate the publisher.

pub mod install;
pub mod probe;
pub mod store;
pub mod tool;

pub use probe::{ProbeLimits, probe_file, probe_file_with};
pub use store::{MediaTarget, ReadReport, read_files};

/// What `ffprobe` reported about one file. Every value is optional: a file may have no video or
/// no audio stream, or `ffprobe` may not know a figure (for example a bitrate of a stream).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaInfo {
    /// Duration in seconds.
    pub length: Option<f64>,
    /// Frames per second of the first video stream.
    pub fps: Option<f64>,
    pub video_width: Option<i64>,
    pub video_height: Option<i64>,
    /// `ffprobe` codec name of the video stream, for example `h264`.
    pub video_codec: Option<String>,
    /// kbit/s.
    pub video_bitrate: Option<f64>,
    /// `ffprobe` codec name of the audio stream, for example `aac`.
    pub audio_codec: Option<String>,
    /// Hz.
    pub audio_sample_rate: Option<i64>,
    /// kbit/s.
    pub audio_bitrate: Option<f64>,
}

impl MediaInfo {
    /// True when `ffprobe` reported no usable figure at all.
    pub fn is_empty(&self) -> bool {
        *self == MediaInfo::default()
    }

    /// Text of a hash export media token, or `None` when this file has no value for it (the
    /// export then keeps its constant). `copy_length_as_minutes` selects whole minutes (rounded
    /// to the nearest) instead of whole seconds for `%length%`.
    pub fn token(&self, token: &str, copy_length_as_minutes: bool) -> Option<String> {
        match token {
            "%fps%" => self.fps.map(|v| format!("{v:.2}")),
            "%length%" => self.length.map(|seconds| {
                if copy_length_as_minutes {
                    format!("{:.0}", (seconds / 60.0).round())
                } else {
                    format!("{:.0}", seconds.round())
                }
            }),
            "%height%" => self.video_height.map(|v| v.to_string()),
            "%width%" => self.video_width.map(|v| v.to_string()),
            "%vcodec%" => self.video_codec.clone(),
            "%vbitrate%" => self.video_bitrate.map(|v| format!("{v:.2}")),
            "%acodec%" => self.audio_codec.clone(),
            "%asample%" => self.audio_sample_rate.map(|v| v.to_string()),
            "%abitrate%" => self.audio_bitrate.map(|v| format!("{v:.2}")),
            _ => None,
        }
    }
}

/// Why a media operation failed. Independent of the AniDB error types: nothing here affects the
/// AniDB cooldown, spacing or queue.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("cancelled")]
    Cancelled,
    #[error("the ffprobe download is not available for this platform")]
    UnsupportedPlatform,
    #[error("could not start ffprobe ({0})")]
    Spawn(String),
    #[error("the file path is not absolute")]
    NotAbsolute,
    #[error("the file does not exist (or its drive is not available)")]
    FileMissing,
    #[error("ffprobe did not finish within {0} seconds")]
    Timeout(u64),
    #[error("ffprobe printed more than {limit} bytes")]
    OutputTooLarge { limit: usize },
    /// `ffprobe` ran and refused the file (not a media file, unreadable, damaged).
    #[error("ffprobe could not read the file{}", .0.as_deref().map(|m| format!(": {m}")).unwrap_or_default())]
    Unreadable(Option<String>),
    #[error("ffprobe printed output that could not be understood ({0})")]
    Parse(String),
    #[error("{0} is not ffprobe")]
    NotFfprobe(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("the server answered with HTTP status {0}")]
    Http(u16),
    #[error("redirect refused: {0}")]
    Redirect(String),
    #[error("the download is larger than allowed ({limit} bytes)")]
    TooLarge { limit: u64 },
    #[error("the download has {received} bytes, expected {expected}")]
    Size { expected: u64, received: u64 },
    #[error("the download does not match the pinned SHA-256 checksum; it was discarded")]
    Checksum,
    #[error("the archive is not usable: {0}")]
    Archive(String),
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Db(#[from] crate::error::Error),
}

pub type Result<T, E = MediaError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MediaInfo {
        MediaInfo {
            length: Some(1425.4),
            fps: Some(23.976_023_976),
            video_width: Some(1920),
            video_height: Some(1080),
            video_codec: Some("h264".into()),
            video_bitrate: Some(3500.0),
            audio_codec: Some("aac".into()),
            audio_sample_rate: Some(48_000),
            audio_bitrate: Some(128.0),
        }
    }

    #[test]
    fn tokens_are_formatted_like_the_constants_they_replace() {
        let info = sample();
        let get = |t: &str, minutes: bool| info.token(t, minutes).unwrap();
        assert_eq!(get("%fps%", false), "23.98");
        assert_eq!(get("%length%", false), "1425");
        assert_eq!(get("%length%", true), "24");
        assert_eq!(get("%height%", false), "1080");
        assert_eq!(get("%width%", false), "1920");
        assert_eq!(get("%vcodec%", false), "h264");
        assert_eq!(get("%vbitrate%", false), "3500.00");
        assert_eq!(get("%acodec%", false), "aac");
        assert_eq!(get("%asample%", false), "48000");
        assert_eq!(get("%abitrate%", false), "128.00");
        assert_eq!(info.token("%size%", false), None);
    }

    #[test]
    fn missing_values_have_no_token_text() {
        let info = MediaInfo {
            length: Some(90.0),
            ..MediaInfo::default()
        };
        assert_eq!(info.token("%fps%", false), None);
        assert_eq!(info.token("%vcodec%", false), None);
        assert_eq!(info.token("%length%", true).as_deref(), Some("2"));
        assert!(MediaInfo::default().is_empty());
        assert!(!info.is_empty());
    }
}
