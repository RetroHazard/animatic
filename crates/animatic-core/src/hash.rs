//! Simultaneous ED2K / CRC32 / MD5 / SHA-1 hashing of files.
//!
//! The reader thread feeds 4 MiB blocks to one worker per algorithm group so a file is
//! read exactly once and hashing runs at disk speed on multi-core machines.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crossbeam_channel::bounded;
use md4::{Digest, Md4};

use crate::error::{Error, Result};

/// eDonkey chunk size.
pub const ED2K_CHUNK: u64 = 9_728_000;
const BLOCK: usize = 4 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Hashes {
    pub size: u64,
    /// Lowercase hex.
    pub ed2k: String,
    /// Uppercase hex, zero padded to 8 digits.
    pub crc32: String,
    /// Uppercase hex.
    pub md5: String,
    /// Uppercase hex.
    pub sha1: String,
}

impl Hashes {
    pub fn ed2k_link(&self, file_name: &str) -> String {
        format!("ed2k://|file|{}|{}|{}|/", file_name, self.size, self.ed2k)
    }

    /// Expand the legacy hash-export template in its original substitution order. The media
    /// tokens keep their constants; see [`Hashes::export_with_media`].
    pub fn export(&self, path: &Path, template: &str, copy_length_as_minutes: bool) -> String {
        self.export_with_media(path, template, copy_length_as_minutes, None)
    }

    /// [`Hashes::export`] with the media properties read through `ffprobe`. A media token
    /// without a value in `media` (the file was not probed, or has no such stream) keeps its
    /// constant.
    pub fn export_with_media(
        &self,
        path: &Path,
        template: &str,
        copy_length_as_minutes: bool,
        media: Option<&crate::media::MediaInfo>,
    ) -> String {
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        let full = self.ed2k_link(&file_name);
        let replacements = [
            ("%filename%", file_name.into_owned()),
            ("%filepath%", path.to_string_lossy().into_owned()),
            ("%ed2k%", self.ed2k.clone()),
            ("%md5%", self.md5.clone()),
            ("%sha1%", self.sha1.clone()),
            ("%crc32%", self.crc32.clone()),
            ("%size%", crate::format::size_short(self.size as i64)),
            ("%fulled2k%", full),
            ("%fps%", "0.00".into()),
            (
                "%length%",
                if copy_length_as_minutes {
                    "0".into()
                } else {
                    String::new()
                },
            ),
            ("%height%", "0".into()),
            ("%width%", "0".into()),
            ("%vcodec%", String::new()),
            ("%vbitrate%", "0.00".into()),
            ("%acodec%", String::new()),
            ("%asample%", "0".into()),
            ("%abitrate%", "0.00".into()),
        ];
        let mut out = if template.is_empty() {
            "%fulled2k%".to_string()
        } else {
            template.replace('\n', "\r\n")
        };
        for (token, constant) in replacements {
            let value = media
                .and_then(|m| m.token(token, copy_length_as_minutes))
                .unwrap_or(constant);
            out = out.replace(token, &value);
        }
        out
    }
}

/// How to treat files whose size is an exact multiple of [`ED2K_CHUNK`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum Ed2kVariant {
    /// Original eDonkey/eMule behaviour (used by AniDB O'Matic and AniDB): an MD4 of an
    /// empty chunk is appended to the chunk hash list.
    #[default]
    Red,
    /// Newer eMule behaviour: no trailing empty chunk.
    Blue,
}

/// Incremental ED2K hasher.
#[derive(Clone)]
pub struct Ed2k {
    variant: Ed2kVariant,
    chunk: Md4,
    in_chunk: u64,
    chunk_hashes: Vec<u8>,
    total: u64,
}

impl Ed2k {
    pub fn new(variant: Ed2kVariant) -> Self {
        Ed2k {
            variant,
            chunk: Md4::new(),
            in_chunk: 0,
            chunk_hashes: Vec::new(),
            total: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        while !data.is_empty() {
            let room = (ED2K_CHUNK - self.in_chunk) as usize;
            let take = room.min(data.len());
            self.chunk.update(&data[..take]);
            self.in_chunk += take as u64;
            data = &data[take..];
            if self.in_chunk == ED2K_CHUNK {
                let h = std::mem::replace(&mut self.chunk, Md4::new()).finalize();
                self.chunk_hashes.extend_from_slice(&h);
                self.in_chunk = 0;
            }
        }
    }

    pub fn finalize(mut self) -> [u8; 16] {
        let exact_multiple = self.total > 0 && self.total.is_multiple_of(ED2K_CHUNK);
        if self.in_chunk > 0
            || self.total == 0
            || (exact_multiple && self.variant == Ed2kVariant::Red)
        {
            let h = self.chunk.finalize();
            self.chunk_hashes.extend_from_slice(&h);
        }
        if self.chunk_hashes.len() == 16 {
            let mut out = [0u8; 16];
            out.copy_from_slice(&self.chunk_hashes);
            return out;
        }
        Md4::digest(&self.chunk_hashes).into()
    }
}

/// Progress/cancellation hooks for [`hash_file`].
pub struct Control<'a> {
    pub cancel: &'a AtomicBool,
    pub pause: &'a AtomicBool,
    pub progress: &'a mut dyn FnMut(u64, u64),
}

/// Hash a file with all supported algorithms.
pub fn hash_file(path: &Path, variant: Ed2kVariant, ctl: Control) -> Result<Hashes> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();

    let (tx_ed2k, rx_ed2k) = bounded::<Arc<Vec<u8>>>(4);
    let (tx_md5, rx_md5) = bounded::<Arc<Vec<u8>>>(4);
    let (tx_sha, rx_sha) = bounded::<Arc<Vec<u8>>>(4);

    let ed2k_worker = thread::spawn(move || {
        let mut h = Ed2k::new(variant);
        for block in rx_ed2k {
            h.update(&block);
        }
        h.finalize()
    });
    let md5_worker = thread::spawn(move || {
        let mut h = md5::Md5::new();
        for block in rx_md5 {
            h.update(&block[..]);
        }
        h.finalize()
    });
    let sha_worker = thread::spawn(move || {
        let mut h = sha1::Sha1::new();
        let mut crc = crc32fast::Hasher::new();
        for block in rx_sha {
            h.update(&block[..]);
            crc.update(&block);
        }
        (h.finalize(), crc.finalize())
    });

    let mut done: u64 = 0;
    let mut result: Result<()> = Ok(());
    loop {
        if ctl.cancel.load(Ordering::Relaxed) {
            result = Err(Error::Cancelled);
            break;
        }
        while ctl.pause.load(Ordering::Relaxed) && !ctl.cancel.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(100));
        }
        let mut buf = vec![0u8; BLOCK];
        let mut filled = 0;
        while filled < BLOCK {
            match file.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    result = Err(e.into());
                    break;
                }
            }
        }
        if result.is_err() || filled == 0 {
            break;
        }
        buf.truncate(filled);
        let block = Arc::new(buf);
        done += filled as u64;
        if tx_ed2k.send(block.clone()).is_err()
            || tx_md5.send(block.clone()).is_err()
            || tx_sha.send(block).is_err()
        {
            result = Err(Error::Other("hash worker stopped".into()));
            break;
        }
        (ctl.progress)(done, size.max(done));
        if filled < BLOCK {
            break;
        }
    }
    drop((tx_ed2k, tx_md5, tx_sha));
    let ed2k = ed2k_worker
        .join()
        .map_err(|_| Error::Other("ed2k worker panicked".into()))?;
    let md5 = md5_worker
        .join()
        .map_err(|_| Error::Other("md5 worker panicked".into()))?;
    let (sha, crc) = sha_worker
        .join()
        .map_err(|_| Error::Other("sha1 worker panicked".into()))?;
    result?;
    Ok(Hashes {
        size: done,
        ed2k: hex::encode(ed2k),
        crc32: format!("{crc:08X}"),
        md5: hex::encode_upper(md5),
        sha1: hex::encode_upper(sha),
    })
}

/// Hash an in-memory buffer (tests and small files).
pub fn hash_bytes(data: &[u8], variant: Ed2kVariant) -> Hashes {
    let mut e = Ed2k::new(variant);
    e.update(data);
    Hashes {
        size: data.len() as u64,
        ed2k: hex::encode(e.finalize()),
        crc32: format!("{:08X}", crc32fast::hash(data)),
        md5: hex::encode_upper(md5::Md5::digest(data)),
        sha1: hex::encode_upper(sha1::Sha1::digest(data)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_small_inputs() {
        let h = hash_bytes(b"", Ed2kVariant::Red);
        assert_eq!(h.ed2k, "31d6cfe0d16ae931b73c59d7e0c089c0");
        assert_eq!(h.crc32, "00000000");
        let h = hash_bytes(b"abc", Ed2kVariant::Red);
        assert_eq!(h.ed2k, "a448017aaf21d8525fc10ae87aa6729d");
        assert_eq!(h.crc32, "352441C2");
        assert_eq!(h.md5, "900150983CD24FB0D6963F7D28E17F72");
        assert_eq!(h.sha1, "A9993E364706816ABA3E25717850C26C9CD0D89D");
    }

    #[test]
    fn exact_chunk_multiple_differs_between_variants() {
        let data = vec![0u8; ED2K_CHUNK as usize];
        let red = hash_bytes(&data, Ed2kVariant::Red);
        let blue = hash_bytes(&data, Ed2kVariant::Blue);
        // Well known values for one chunk of zero bytes.
        assert_eq!(blue.ed2k, "d7def262a127cd79096a108e7a9fc138");
        assert_eq!(red.ed2k, "fc21d9af828f92a8df64beac3357425d");
    }

    #[test]
    fn chunk_boundaries_are_independent_of_block_splits() {
        let data: Vec<u8> = (0..(ED2K_CHUNK as usize + 12345))
            .map(|i| (i * 31 % 251) as u8)
            .collect();
        let whole = hash_bytes(&data, Ed2kVariant::Red).ed2k;
        let mut e = Ed2k::new(Ed2kVariant::Red);
        for part in data.chunks(1_000_003) {
            e.update(part);
        }
        assert_eq!(hex::encode(e.finalize()), whole);
    }

    #[test]
    fn hash_file_matches_in_memory_hashing() {
        let dir = std::env::temp_dir().join(format!("animatic-hash-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.bin");
        let data: Vec<u8> = (0..(BLOCK * 2 + 777)).map(|i| (i % 253) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        let cancel = AtomicBool::new(false);
        let pause = AtomicBool::new(false);
        let mut last = 0;
        let h = hash_file(
            &p,
            Ed2kVariant::Red,
            Control {
                cancel: &cancel,
                pause: &pause,
                progress: &mut |d, _| last = d,
            },
        )
        .unwrap();
        assert_eq!(h, hash_bytes(&data, Ed2kVariant::Red));
        assert_eq!(last, data.len() as u64);
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn legacy_export_tokens_expand_in_order() {
        let h = hash_bytes(b"abc", Ed2kVariant::Red);
        let text = h.export(
            Path::new("/media/a.mkv"),
            "%filename% %size% %crc32% %fulled2k%",
            false,
        );
        assert_eq!(
            text,
            "a.mkv 3 bytes 352441C2 ed2k://|file|a.mkv|3|a448017aaf21d8525fc10ae87aa6729d|/"
        );
    }
}
