//! Running `ffprobe` on one file and understanding its JSON.
//!
//! The command is an argument vector (no shell): `ffprobe -v error -protocol_whitelist file
//! -print_format json -show_format -show_streams -i <file>`. The protocol whitelist keeps a
//! playlist-like file from making `ffprobe` open network locations, so probing never sends
//! anything. The process gets no standard input, its output is read in full but bounded, and a
//! timeout and the caller's cancel flag stop it.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{MediaError, MediaInfo, Result};

/// Longest a single `ffprobe` run may take before it is killed.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
/// Largest JSON document accepted from `ffprobe`. Typical output is a few kilobytes; files with
/// many streams stay far below this.
pub const MAX_PROBE_OUTPUT: usize = 4 * 1024 * 1024;
/// Longest a `-version` run may take.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_VERSION_OUTPUT: usize = 64 * 1024;
const MAX_STDERR: usize = 16 * 1024;
/// How many streams of a document are looked at.
const MAX_STREAMS: usize = 1024;
const POLL: Duration = Duration::from_millis(10);

/// Time and size bounds of one `ffprobe` run.
#[derive(Debug, Clone, Copy)]
pub struct ProbeLimits {
    pub timeout: Duration,
    pub max_output: usize,
}

impl Default for ProbeLimits {
    fn default() -> Self {
        ProbeLimits {
            timeout: PROBE_TIMEOUT,
            max_output: MAX_PROBE_OUTPUT,
        }
    }
}

/// Probe `media` (an absolute path) with the `ffprobe` executable at `ffprobe`.
pub fn probe_file(ffprobe: &Path, media: &Path, cancel: &AtomicBool) -> Result<MediaInfo> {
    probe_file_with(ffprobe, media, ProbeLimits::default(), cancel)
}

/// [`probe_file`] with explicit bounds.
pub fn probe_file_with(
    ffprobe: &Path,
    media: &Path,
    limits: ProbeLimits,
    cancel: &AtomicBool,
) -> Result<MediaInfo> {
    if !media.is_absolute() {
        // A relative name could be taken for an option or a protocol by ffprobe.
        return Err(MediaError::NotAbsolute);
    }
    let mut command = Command::new(ffprobe);
    command
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-i",
        ])
        .arg(media);
    let output = run_bounded(command, limits.max_output, limits.timeout, cancel)?;
    if !output.status.success() {
        return Err(MediaError::Unreadable(first_line(&output.stderr)));
    }
    parse_probe_json(&output.stdout)
}

/// Run `ffprobe -version` and return the version token of its first line (`n9.0.2-39-g...`).
pub fn ffprobe_version(ffprobe: &Path, cancel: &AtomicBool) -> Result<String> {
    let mut command = Command::new(ffprobe);
    command.arg("-version");
    let output = run_bounded(command, MAX_VERSION_OUTPUT, VERSION_TIMEOUT, cancel)?;
    let shown = || ffprobe.display().to_string();
    if !output.status.success() {
        return Err(MediaError::NotFfprobe(shown()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().next().unwrap_or_default();
    first
        .strip_prefix("ffprobe version ")
        .and_then(|rest| rest.split_whitespace().next())
        .map(str::to_owned)
        .ok_or_else(|| MediaError::NotFfprobe(shown()))
}

fn first_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(300).collect())
}

// ---------------------------------------------------------------------------------------------
// Bounded process execution
// ---------------------------------------------------------------------------------------------

/// What a finished process printed.
pub(crate) struct Captured {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

struct Pipe {
    data: Vec<u8>,
    exceeded: bool,
}

/// Reads `source` to its end, keeping at most `limit` bytes; the rest is read and dropped so the
/// process never blocks on a full pipe. `exceeded` is raised as soon as the limit is passed.
fn drain(
    mut source: impl Read + Send + 'static,
    limit: usize,
    exceeded: Arc<AtomicBool>,
) -> crossbeam_channel::Receiver<Pipe> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let mut data = Vec::new();
        let mut over = false;
        let mut buf = [0u8; 8192];
        loop {
            match source.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if data.len() + n > limit {
                        let room = limit - data.len();
                        data.extend_from_slice(&buf[..room]);
                        over = true;
                        exceeded.store(true, Ordering::Release);
                    } else if !over {
                        data.extend_from_slice(&buf[..n]);
                    }
                }
            }
        }
        let _ = tx.send(Pipe {
            data,
            exceeded: over,
        });
    });
    rx
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Run `command` with no standard input, capture at most `max_stdout` bytes of standard output
/// (more is [`MediaError::OutputTooLarge`]) and a short part of standard error, and kill it when
/// `timeout` passes or `cancel` is raised.
pub(crate) fn run_bounded(
    mut command: Command,
    max_stdout: usize,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<Captured> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        // The kind only: the message may name paths of the user's system.
        .map_err(|e| MediaError::Spawn(e.kind().to_string()))?;
    let exceeded = Arc::new(AtomicBool::new(false));
    let out = drain(
        child.stdout.take().expect("piped"),
        max_stdout,
        exceeded.clone(),
    );
    let err = drain(
        child.stderr.take().expect("piped"),
        MAX_STDERR,
        Arc::new(AtomicBool::new(false)),
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill(&mut child);
                return Err(MediaError::Io(e));
            }
        }
        if cancel.load(Ordering::Acquire) {
            kill(&mut child);
            return Err(MediaError::Cancelled);
        }
        if exceeded.load(Ordering::Acquire) {
            kill(&mut child);
            return Err(MediaError::OutputTooLarge { limit: max_stdout });
        }
        if Instant::now() >= deadline {
            kill(&mut child);
            return Err(MediaError::Timeout(timeout.as_secs().max(1)));
        }
        std::thread::sleep(POLL);
    };
    // A descendant that inherited the pipes could keep them open; do not wait for it forever.
    let settle = Duration::from_secs(2);
    let stdout = out
        .recv_timeout(settle)
        .map_err(|_| MediaError::Timeout(settle.as_secs()))?;
    let stderr = err.recv_timeout(settle).ok();
    if stdout.exceeded {
        return Err(MediaError::OutputTooLarge { limit: max_stdout });
    }
    Ok(Captured {
        status,
        stdout: stdout.data,
        stderr: stderr.map(|p| p.data).unwrap_or_default(),
    })
}

// ---------------------------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------------------------

/// A JSON number or a string holding one (`ffprobe` prints most figures as strings).
fn number(value: Option<&Value>) -> Option<f64> {
    let n = match value? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

fn integer(value: Option<&Value>) -> Option<i64> {
    let n = number(value)?;
    (n >= 0.0 && n <= i64::MAX as f64 / 2.0 && n.fract() == 0.0).then_some(n as i64)
}

/// `24000/1001` or `25`. Zero or undefined rates (`0/0`) give `None`.
fn rate(value: Option<&Value>) -> Option<f64> {
    let text = value?.as_str()?.trim();
    let v = match text.split_once('/') {
        Some((n, d)) => {
            let (n, d) = (n.trim().parse::<f64>().ok()?, d.trim().parse::<f64>().ok()?);
            if d == 0.0 { return None } else { n / d }
        }
        None => text.parse::<f64>().ok()?,
    };
    (v.is_finite() && v > 0.0 && v <= 10_000.0).then_some(v)
}

/// A codec name. Only the characters real names use are kept: the value ends up in exports
/// whose templates are expanded token by token, so it must never contain a `%`.
fn text(value: Option<&Value>) -> Option<String> {
    let s = value?.as_str()?.trim();
    let plain = s
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'+'));
    (!s.is_empty() && s != "none" && s.len() <= 64 && plain).then(|| s.to_owned())
}

fn flag(stream: &Value, name: &str) -> bool {
    stream
        .get("disposition")
        .and_then(|d| d.get(name))
        .and_then(Value::as_i64)
        == Some(1)
}

/// Bitrate in kbit/s of a stream: its own figure, or the `BPS` statistics tag some muxers write.
fn bitrate(stream: &Value) -> Option<f64> {
    let bits = number(stream.get("bit_rate"))
        .filter(|v| *v > 0.0)
        .or_else(|| {
            let tags = stream.get("tags")?;
            let tag = ["BPS", "bps", "BPS-eng"]
                .iter()
                .find_map(|name| number(tags.get(name)))?;
            (tag > 0.0).then_some(tag)
        })?;
    Some(bits / 1000.0)
}

/// The stream of `kind` that players would choose: the default one, else the first. Cover art
/// (an attached picture) is never a video stream.
fn pick<'a>(streams: &'a [Value], kind: &str) -> Option<&'a Value> {
    let matching = || {
        streams.iter().filter(move |s| {
            s.get("codec_type").and_then(Value::as_str) == Some(kind) && !flag(s, "attached_pic")
        })
    };
    matching()
        .find(|s| flag(s, "default"))
        .or_else(|| matching().next())
}

/// Understand the document printed by `ffprobe -print_format json -show_format -show_streams`.
pub fn parse_probe_json(bytes: &[u8]) -> Result<MediaInfo> {
    // Failure class and position only: serde messages can quote the offending value.
    let document: Value = serde_json::from_slice(bytes).map_err(|e| {
        MediaError::Parse(format!(
            "invalid JSON at line {} column {}",
            e.line(),
            e.column()
        ))
    })?;
    let Some(root) = document.as_object() else {
        return Err(MediaError::Parse("the document is not an object".into()));
    };
    let streams: Vec<Value> = root
        .get("streams")
        .and_then(Value::as_array)
        .map(|all| all.iter().take(MAX_STREAMS).cloned().collect())
        .unwrap_or_default();
    let mut info = MediaInfo {
        length: number(root.get("format").and_then(|f| f.get("duration"))),
        ..MediaInfo::default()
    };
    if info.length.is_none() {
        info.length = streams
            .iter()
            .filter_map(|s| number(s.get("duration")))
            .reduce(f64::max);
    }
    info.length = info.length.filter(|v| *v >= 0.0);
    if let Some(video) = pick(&streams, "video") {
        info.video_width = integer(video.get("width")).filter(|v| *v > 0);
        info.video_height = integer(video.get("height")).filter(|v| *v > 0);
        info.video_codec = text(video.get("codec_name"));
        info.fps = rate(video.get("avg_frame_rate")).or_else(|| rate(video.get("r_frame_rate")));
        info.video_bitrate = bitrate(video);
    }
    if let Some(audio) = pick(&streams, "audio") {
        info.audio_codec = text(audio.get("codec_name"));
        info.audio_sample_rate = integer(audio.get("sample_rate")).filter(|v| *v > 0);
        info.audio_bitrate = bitrate(audio);
    }
    Ok(info)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;

    const SAMPLE: &str = r#"{
        "streams": [
            {"index": 0, "codec_name": "mjpeg", "codec_type": "video", "width": 600, "height": 600,
             "avg_frame_rate": "0/0", "disposition": {"default": 0, "attached_pic": 1}},
            {"index": 1, "codec_name": "h264", "codec_type": "video", "width": 1920, "height": 1080,
             "r_frame_rate": "24000/1001", "avg_frame_rate": "24000/1001", "bit_rate": "3500000",
             "disposition": {"default": 1}},
            {"index": 2, "codec_name": "flac", "codec_type": "audio", "sample_rate": "44100",
             "bit_rate": "900000", "disposition": {"default": 0}},
            {"index": 3, "codec_name": "aac", "codec_type": "audio", "sample_rate": "48000",
             "bit_rate": "128000", "disposition": {"default": 1}},
            {"index": 4, "codec_name": "ass", "codec_type": "subtitle"}
        ],
        "format": {"duration": "1425.426000", "bit_rate": "4000000", "size": "123456"}
    }"#;

    #[test]
    fn synthetic_json_gives_the_default_streams_and_skips_cover_art() {
        let info = parse_probe_json(SAMPLE.as_bytes()).unwrap();
        assert_eq!(info.length, Some(1425.426));
        assert_eq!(
            (info.video_width, info.video_height),
            (Some(1920), Some(1080))
        );
        assert_eq!(info.video_codec.as_deref(), Some("h264"));
        assert!((info.fps.unwrap() - 23.976_023_976).abs() < 1e-6);
        assert_eq!(info.video_bitrate, Some(3500.0));
        assert_eq!(info.audio_codec.as_deref(), Some("aac"));
        assert_eq!(info.audio_sample_rate, Some(48_000));
        assert_eq!(info.audio_bitrate, Some(128.0));
    }

    #[test]
    fn audio_only_and_sparse_documents_keep_what_is_known() {
        let wav = r#"{"streams":[{"codec_type":"audio","codec_name":"pcm_s16le",
            "sample_rate":"8000","bit_rate":"128000"}],"format":{"duration":"2.000000"}}"#;
        let info = parse_probe_json(wav.as_bytes()).unwrap();
        assert_eq!(info.length, Some(2.0));
        assert_eq!(info.audio_codec.as_deref(), Some("pcm_s16le"));
        assert_eq!(info.audio_sample_rate, Some(8000));
        assert_eq!(info.audio_bitrate, Some(128.0));
        assert_eq!(info.fps, None);
        assert_eq!(info.video_width, None);

        let none = parse_probe_json(br#"{"streams":[],"format":{}}"#).unwrap();
        assert!(none.is_empty());
        assert!(parse_probe_json(b"{}").unwrap().is_empty());
    }

    #[test]
    fn container_duration_missing_falls_back_to_the_longest_stream() {
        let doc = r#"{"streams":[{"codec_type":"video","duration":"10.5"},
            {"codec_type":"audio","duration":"12.25"}],"format":{}}"#;
        assert_eq!(
            parse_probe_json(doc.as_bytes()).unwrap().length,
            Some(12.25)
        );
    }

    #[test]
    fn matroska_statistics_tag_supplies_a_missing_bitrate() {
        let doc = r#"{"streams":[{"codec_type":"video","codec_name":"hevc","width":10,"height":10,
            "tags":{"BPS":"2000000"}}],"format":{"duration":"1"}}"#;
        assert_eq!(
            parse_probe_json(doc.as_bytes()).unwrap().video_bitrate,
            Some(2000.0)
        );
    }

    #[test]
    fn implausible_numbers_are_dropped_instead_of_stored() {
        let doc = r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":-5,"height":0,
            "avg_frame_rate":"1/0","r_frame_rate":"1000000/1","bit_rate":"-1"},
            {"codec_type":"audio","sample_rate":"abc","bit_rate":"N/A"}],
            "format":{"duration":"-3"}}"#;
        let info = parse_probe_json(doc.as_bytes()).unwrap();
        assert_eq!(info.video_codec.as_deref(), Some("h264"));
        assert_eq!(info.video_width, None);
        assert_eq!(info.video_height, None);
        assert_eq!(info.fps, None);
        assert_eq!(info.video_bitrate, None);
        assert_eq!(info.audio_sample_rate, None);
        assert_eq!(info.audio_bitrate, None);
        assert_eq!(info.length, None);
    }

    #[test]
    fn malformed_documents_are_errors_without_echoing_their_content() {
        for bad in [
            &b""[..],
            b"not json",
            b"[1,2]",
            b"{\"streams\": [",
            b"\"secret-text\"",
        ] {
            let e = parse_probe_json(bad).unwrap_err();
            assert!(matches!(e, MediaError::Parse(_)), "{e}");
            assert!(!e.to_string().contains("secret-text"), "{e}");
        }
    }

    // ---- process handling -----------------------------------------------------------------

    /// Helper mode of this test binary, used as a child process by the tests below. Without the
    /// variable the test returns at once.
    #[test]
    fn child_helper() {
        match std::env::var("ANIMATIC_MEDIA_TEST_CHILD").as_deref() {
            Ok("sleep") => std::thread::sleep(Duration::from_secs(60)),
            Ok("flood") => {
                let chunk = [b'x'; 8192];
                let mut out = std::io::stdout();
                loop {
                    if std::io::Write::write_all(&mut out, &chunk).is_err() {
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    fn helper_command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "media::probe::tests::child_helper",
                "--nocapture",
            ])
            .env("ANIMATIC_MEDIA_TEST_CHILD", mode);
        command
    }

    #[test]
    fn a_process_that_does_not_finish_is_killed_at_the_timeout() {
        let started = Instant::now();
        let error = run_bounded(
            helper_command("sleep"),
            1024,
            Duration::from_millis(300),
            &AtomicBool::new(false),
        )
        .err()
        .expect("must time out");
        assert!(matches!(error, MediaError::Timeout(_)), "{error}");
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn endless_output_is_cut_off_at_the_limit() {
        let started = Instant::now();
        let error = run_bounded(
            helper_command("flood"),
            64 * 1024,
            Duration::from_secs(30),
            &AtomicBool::new(false),
        )
        .err()
        .expect("must be refused");
        assert!(
            matches!(error, MediaError::OutputTooLarge { limit: 65536 }),
            "{error}"
        );
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn output_within_the_limit_is_returned_whole() {
        let captured = run_bounded(
            helper_command("none"),
            1024 * 1024,
            Duration::from_secs(30),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(captured.status.success());
        assert!(String::from_utf8_lossy(&captured.stdout).contains("child_helper"));
    }

    #[test]
    fn cancel_stops_a_running_process() {
        let cancel = Arc::new(AtomicBool::new(false));
        let raise = cancel.clone();
        let timer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            raise.store(true, Ordering::Release);
        });
        let error = run_bounded(
            helper_command("sleep"),
            1024,
            Duration::from_secs(60),
            &cancel,
        )
        .err()
        .expect("must be cancelled");
        timer.join().unwrap();
        assert!(matches!(error, MediaError::Cancelled), "{error}");
    }

    #[test]
    fn a_missing_executable_is_a_spawn_error() {
        let dir = tempfile::tempdir().unwrap();
        let error = probe_file(
            &dir.path().join("no-such-ffprobe"),
            &dir.path().join("a.mkv"),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(matches!(error, MediaError::Spawn(_)), "{error}");
    }

    #[test]
    fn relative_media_paths_are_refused_before_anything_starts() {
        let dir = tempfile::tempdir().unwrap();
        let error = probe_file(
            &dir.path().join("no-such-ffprobe"),
            Path::new("-i.mkv"),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.to_string().contains("not absolute"), "{error}");
    }

    /// A stand-in `ffprobe`: prints the JSON in `out.json` and records its arguments. A shell
    /// script on Unix and a batch file on Windows (both run without extra tooling).
    pub(crate) fn fake_ffprobe(dir: &Path, json: &str, exit_code: i32) -> std::path::PathBuf {
        fs::write(dir.join("out.json"), json).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join("fake-ffprobe");
            let script = format!(
                "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" > \"$d/args.txt\"\n\
                 cat \"$d/out.json\"\nexit {exit_code}\n"
            );
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
        #[cfg(windows)]
        {
            let path = dir.join("fake-ffprobe.cmd");
            let script = format!(
                "@echo off\r\n(for %%a in (%*) do echo %%a) > \"%~dp0args.txt\"\r\n\
                 type \"%~dp0out.json\"\r\nexit /b {exit_code}\r\n"
            );
            fs::write(&path, script).unwrap();
            path
        }
    }

    #[test]
    fn probing_runs_the_documented_argument_vector_and_parses_the_output() {
        let dir = tempfile::tempdir().unwrap();
        let ffprobe = fake_ffprobe(dir.path(), SAMPLE, 0);
        let media = dir.path().join("episode one.mkv");
        let info = probe_file(&ffprobe, &media, &AtomicBool::new(false)).unwrap();
        assert_eq!(info.video_codec.as_deref(), Some("h264"));

        let recorded = fs::read_to_string(dir.path().join("args.txt")).unwrap();
        let args: Vec<&str> = recorded.lines().map(|l| l.trim_matches('"')).collect();
        let expected = [
            "-v",
            "error",
            "-protocol_whitelist",
            "file",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-i",
        ];
        assert_eq!(&args[..expected.len()], &expected);
        // The path is the last argument, in one piece although it contains a space.
        assert_eq!(args.len(), expected.len() + 1);
        assert_eq!(args[expected.len()], media.to_string_lossy());
    }

    #[test]
    fn a_failing_ffprobe_reports_its_message_as_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let ffprobe = fake_ffprobe(dir.path(), "", 1);
        let error =
            probe_file(&ffprobe, &dir.path().join("a.mkv"), &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(error, MediaError::Unreadable(_)), "{error}");
    }

    #[test]
    fn oversized_probe_documents_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let big = format!("{{\"streams\":[],\"pad\":\"{}\"}}", "x".repeat(4096));
        let ffprobe = fake_ffprobe(dir.path(), &big, 0);
        let error = probe_file_with(
            &ffprobe,
            &dir.path().join("a.mkv"),
            ProbeLimits {
                timeout: Duration::from_secs(30),
                max_output: 1024,
            },
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(
            matches!(error, MediaError::OutputTooLarge { limit: 1024 }),
            "{error}"
        );
    }
}
