//! Decryption of the legacy AniDB O'Matic databank (`kowai/*.data`).
//!
//! The legacy client stored every collection in an encrypted SQLite database.
//! Two page codecs were in use (reverse engineered from `AOM.exe` / `aomdbi4.dll`):
//!
//! * `local_*.data` — opened through `aomdbi4.dll` (SQLite 3.0.8 + Gladman AES).
//!   `sqlite3_key` derives a 256-bit key with PBKDF2-HMAC-SHA256 (5000 rounds) from a
//!   32-byte constant embedded in `AOM.exe`; pages are encrypted with AES-256 in CTR
//!   mode where the counter block is `file_offset / 16` as a little-endian 128-bit
//!   integer (the whole file is one continuous key stream).
//! * every other `*.data` file — opened through the statically linked DISQLite3 engine.
//!   Its key is a 16-byte secret stored AES-256-CBC encrypted inside `AOM.exe`;
//!   PBKDF2-HMAC-SHA256 (1024 rounds, 60 bytes) stretches it and the first 32 bytes
//!   key AES-256-CBC per page with IV = page number repeated as four little-endian u32.
//!
//! Both variants share the same 16-byte salt. Decrypted output is a standard SQLite 3
//! database that any modern SQLite can read.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use aes::Aes256;
use aes::cipher::array::Array;
use aes::cipher::{BlockCipherEncrypt, BlockModeDecrypt, KeyInit, KeyIvInit};

use crate::error::{Error, Result};

const SALT: [u8; 16] = [
    0x5c, 0x08, 0xeb, 0x61, 0xfd, 0xf7, 0x1e, 0x4e, 0x4e, 0xc3, 0xcf, 0x6b, 0xa1, 0xf5, 0x51, 0x2b,
];

/// `sqlite3_key` password used by AOM.exe for the local_* databases (string literal at 0x67c928).
const LOCAL_PASSWORD: [u8; 32] = [
    0xb4, 0x88, 0x94, 0x4a, 0x4d, 0xc6, 0x03, 0xc2, 0xd0, 0xb5, 0x0b, 0x69, 0xdc, 0xca, 0x5f, 0xf6,
    0xdf, 0x0e, 0xf3, 0x5d, 0x86, 0xc4, 0x0d, 0x97, 0x77, 0x4c, 0x9f, 0xa5, 0x43, 0x47, 0xd4, 0xd3,
];

/// AES-256 key recovered from the decryption key schedule embedded at 0x7d9928.
const KEY_ENCRYPTION_KEY: [u8; 32] = [
    0x1a, 0x76, 0xbc, 0x36, 0x4d, 0xde, 0xe2, 0x7a, 0xd1, 0xbc, 0xb2, 0x8a, 0xa4, 0x50, 0xad, 0x5c,
    0x4a, 0xb5, 0x2a, 0xc0, 0xf8, 0x16, 0xd6, 0x2d, 0x3f, 0xc7, 0xbc, 0xb4, 0xf3, 0x3d, 0x3f, 0x09,
];
/// CBC IV stored in the same context record.
const KEY_ENCRYPTION_IV: [u8; 16] = [
    0x01, 0x71, 0x93, 0x54, 0x44, 0x50, 0x3c, 0x7a, 0x71, 0x13, 0x25, 0x79, 0x74, 0x5f, 0x0c, 0x04,
];
/// Encrypted DISQLite3 password (string literal at 0x7d9a50).
const ENCRYPTED_ANIDB_PASSWORD: [u8; 16] = [
    0xc3, 0xb5, 0x3f, 0xdd, 0x68, 0xdd, 0x15, 0x3c, 0x11, 0x29, 0xd7, 0x51, 0x77, 0x75, 0x07, 0x3a,
];

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// Which page codec a legacy databank file uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// aomdbi4.dll: AES-256-CTR over the whole file.
    LocalCtr,
    /// DISQLite3: AES-256-CBC per page.
    AnidbCbc,
    /// Already a plain SQLite database.
    Plain,
}

fn pbkdf2_sha256(password: &[u8], rounds: u32, out: &mut [u8]) {
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(password, &SALT, rounds, out);
}

fn local_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2_sha256(&LOCAL_PASSWORD, 5000, &mut key);
    key
}

fn anidb_password() -> [u8; 16] {
    let mut block = Array::from(ENCRYPTED_ANIDB_PASSWORD);
    let mut dec =
        cbc::Decryptor::<Aes256>::new(&KEY_ENCRYPTION_KEY.into(), &KEY_ENCRYPTION_IV.into());
    dec.decrypt_blocks(std::slice::from_mut(&mut block));
    block.into()
}

fn anidb_key() -> [u8; 32] {
    let mut material = [0u8; 60];
    pbkdf2_sha256(&anidb_password(), 1024, &mut material);
    let mut key = [0u8; 32];
    key.copy_from_slice(&material[..32]);
    key
}

/// Stateful decryptor for one file.
pub struct Decryptor {
    codec: Codec,
    local: Aes256,
    anidb_key: [u8; 32],
}

impl Decryptor {
    pub fn detect(first_page: &[u8]) -> Result<Self> {
        if first_page.len() < 32 {
            return Err(Error::Legacy("file too small to be a databank".into()));
        }
        let mut d = Decryptor {
            codec: Codec::Plain,
            local: Aes256::new(&local_key().into()),
            anidb_key: anidb_key(),
        };
        if &first_page[..16] == SQLITE_MAGIC {
            return Ok(d);
        }
        let mut probe = [0u8; 32];
        probe.copy_from_slice(&first_page[..32]);
        d.ctr_apply(0, &mut probe);
        if &probe[..16] == SQLITE_MAGIC {
            d.codec = Codec::LocalCtr;
            return Ok(d);
        }
        probe.copy_from_slice(&first_page[..32]);
        d.cbc_page(1, &mut probe);
        if &probe[..16] == SQLITE_MAGIC {
            d.codec = Codec::AnidbCbc;
            return Ok(d);
        }
        Err(Error::Legacy("unrecognised databank encryption".into()))
    }

    pub fn codec(&self) -> Codec {
        self.codec
    }

    /// XOR `buf` (which starts at absolute file `offset`, a multiple of 16) with the CTR key stream.
    fn ctr_apply(&self, offset: u64, buf: &mut [u8]) {
        debug_assert_eq!(offset % 16, 0);
        let mut counter = u128::from(offset / 16);
        let mut ks = [Array::default(); 64];
        for chunk in buf.chunks_mut(16 * 64) {
            let blocks = chunk.len().div_ceil(16);
            for b in ks.iter_mut().take(blocks) {
                *b = Array::from(counter.to_le_bytes());
                counter = counter.wrapping_add(1);
            }
            self.local.encrypt_blocks(&mut ks[..blocks]);
            for (i, byte) in chunk.iter_mut().enumerate() {
                *byte ^= ks[i / 16][i % 16];
            }
        }
    }

    fn cbc_page(&self, pgno: u32, page: &mut [u8]) {
        let p = pgno.to_le_bytes();
        let mut iv = [0u8; 16];
        for i in 0..4 {
            iv[i * 4..i * 4 + 4].copy_from_slice(&p);
        }
        let mut dec = cbc::Decryptor::<Aes256>::new(&self.anidb_key.into(), &iv.into());
        let (blocks, _) = Array::slice_as_chunks_mut(page);
        dec.decrypt_blocks(blocks);
    }

    /// Decrypt a page in place. `pgno` is 1-based, `page.len()` is the page size.
    pub fn decrypt_page(&self, pgno: u32, page: &mut [u8]) {
        match self.codec {
            Codec::Plain => {}
            Codec::LocalCtr => self.ctr_apply(u64::from(pgno - 1) * page.len() as u64, page),
            Codec::AnidbCbc => self.cbc_page(pgno, page),
        }
    }

    /// Page size of the database, read from the (decrypted) header of page 1.
    pub fn page_size(&self, first_page: &[u8]) -> Result<usize> {
        let mut head = [0u8; 32];
        head.copy_from_slice(&first_page[..32]);
        match self.codec {
            Codec::Plain => {}
            Codec::LocalCtr => self.ctr_apply(0, &mut head),
            Codec::AnidbCbc => self.cbc_page(1, &mut head),
        }
        let raw = u16::from_be_bytes([head[16], head[17]]);
        let size = if raw == 1 { 65536 } else { usize::from(raw) };
        if !size.is_power_of_two() || !(512..=65536).contains(&size) {
            return Err(Error::Legacy(format!("invalid page size {size}")));
        }
        Ok(size)
    }
}

/// Decrypt a legacy `.data` file into a plain SQLite database at `dst`.
/// `progress` receives (bytes done, bytes total).
pub fn decrypt_file(src: &Path, dst: &Path, mut progress: impl FnMut(u64, u64)) -> Result<Codec> {
    let total = std::fs::metadata(src)?.len();
    let mut input = BufReader::with_capacity(1 << 20, File::open(src)?);
    let mut head = vec![0u8; 512];
    input.read_exact(&mut head)?;
    let dec = Decryptor::detect(&head)?;
    let page_size = dec.page_size(&head)?;
    drop(input);

    let mut input = BufReader::with_capacity(1 << 20, File::open(src)?);
    let mut output = BufWriter::with_capacity(1 << 20, File::create(dst)?);
    let mut page = vec![0u8; page_size];
    let mut pgno: u32 = 1;
    let mut done: u64 = 0;
    loop {
        let mut filled = 0;
        while filled < page_size {
            let n = input.read(&mut page[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            break;
        }
        if filled < page_size {
            return Err(Error::Legacy(format!(
                "{} is truncated (partial page {pgno})",
                src.display()
            )));
        }
        dec.decrypt_page(pgno, &mut page);
        output.write_all(&page)?;
        done += page_size as u64;
        if pgno.is_multiple_of(4096) {
            progress(done, total);
        }
        pgno += 1;
    }
    output.flush()?;
    progress(total, total);
    Ok(dec.codec())
}

#[cfg(test)]
mod tests {
    use aes::cipher::BlockModeEncrypt;

    use super::*;

    fn synthetic_sqlite_page(page_size: usize, counter: u8) -> Vec<u8> {
        let mut page = (0..page_size)
            .map(|i| counter.wrapping_add((i as u8).wrapping_mul(37)))
            .collect::<Vec<_>>();
        page[..16].copy_from_slice(SQLITE_MAGIC);
        page[16..18].copy_from_slice(&(page_size as u16).to_be_bytes());
        page
    }

    fn encrypt_anidb_page(pgno: u32, page: &mut [u8]) {
        let p = pgno.to_le_bytes();
        let mut iv = [0u8; 16];
        for i in 0..4 {
            iv[i * 4..i * 4 + 4].copy_from_slice(&p);
        }
        let mut enc = cbc::Encryptor::<Aes256>::new(&anidb_key().into(), &iv.into());
        let (blocks, _) = Array::slice_as_chunks_mut(page);
        enc.encrypt_blocks(blocks);
    }

    #[test]
    fn anidb_password_decrypts_to_known_secret() {
        assert_eq!(
            hex::encode(anidb_password()),
            "dc4ffff13cd468c5a68981b3ede280bd"
        );
    }

    #[test]
    fn derived_keys_match_reverse_engineered_values() {
        assert_eq!(
            hex::encode(local_key()),
            "c97896fa773be3f4e0dd721041801340a67a9ea1b48999e56f5c700fab9841e2"
        );
        assert_eq!(
            hex::encode(anidb_key()),
            "c86da8c1f0afbdb12952c9c867cdeb11f5b1d22340705915b801addf7e4ba111"
        );
    }

    #[test]
    fn detects_both_codecs_from_synthetic_headers() {
        let anidb_plaintext = synthetic_sqlite_page(1024, 0x41);
        let mut anidb = anidb_plaintext.clone();
        encrypt_anidb_page(1, &mut anidb);

        let local_plaintext = synthetic_sqlite_page(2048, 0x7e);
        let mut local = local_plaintext.clone();
        let local_encryptor = Decryptor {
            codec: Codec::LocalCtr,
            local: Aes256::new(&local_key().into()),
            anidb_key: anidb_key(),
        };
        local_encryptor.ctr_apply(0, &mut local);

        let d = Decryptor::detect(&anidb).unwrap();
        assert_eq!(d.codec(), Codec::AnidbCbc);
        assert_eq!(d.page_size(&anidb).unwrap(), 1024);
        let mut decrypted = anidb;
        d.decrypt_page(1, &mut decrypted);
        assert_eq!(decrypted, anidb_plaintext);

        let d = Decryptor::detect(&local).unwrap();
        assert_eq!(d.codec(), Codec::LocalCtr);
        assert_eq!(d.page_size(&local).unwrap(), 2048);
        let mut decrypted = local;
        d.decrypt_page(1, &mut decrypted);
        assert_eq!(decrypted, local_plaintext);
    }
}
