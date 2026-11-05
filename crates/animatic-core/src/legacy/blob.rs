//! Parser for the legacy `TPBMemoryBlocks` record format stored in `dataTable.data`.
//!
//! Layout (little endian):
//! ```text
//! "data" | u32 field_count | u32 total_value_bytes
//! repeated: u32 key | u32 value_len | u8 type | 3 opaque bytes | value[value_len]
//! key = (class_id << 24) | (field_id << 8)   (field_id is 16 bits; high byte = sub record)
//! type: 0 = int64, 1 = double, 2 = AnsiString, 3 = UTF-16LE WideString, 5 = bool (int32), 6 = int32
//! ```
//! Some legacy records repeat a key; the first occurrence wins (matches the reader in AOM.exe).

use std::collections::HashMap;

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    Raw(Vec<u8>),
}

impl Value {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            Value::Bool(b) => Some(i64::from(*b)),
            Value::Float(f) => Some(*f as i64),
            Value::Text(s) => s.trim().parse().ok(),
            Value::Raw(_) => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(v) => Some(*v as f64),
            Value::Float(f) => Some(*f),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            Value::Text(s) => s.trim().parse().ok(),
            Value::Raw(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }
}

/// One decoded record: field id (lower 16 bits of key >> 8) -> value.
#[derive(Debug, Default, Clone)]
pub struct Record {
    pub class_id: u8,
    fields: HashMap<u16, Value>,
}

impl Record {
    pub fn get(&self, field: u16) -> Option<&Value> {
        self.fields.get(&field)
    }

    pub fn int(&self, field: u16) -> i64 {
        self.get(field).and_then(Value::as_i64).unwrap_or(0)
    }

    pub fn float(&self, field: u16) -> Option<f64> {
        self.get(field).and_then(Value::as_f64)
    }

    pub fn text(&self, field: u16) -> Option<String> {
        match self.get(field)? {
            Value::Text(s) if !s.is_empty() => Some(s.clone()),
            Value::Text(_) => None,
            Value::Int(v) => Some(v.to_string()),
            _ => None,
        }
    }

    pub fn bool(&self, field: u16) -> bool {
        self.int(field) != 0
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

fn rd_u32(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| Error::Legacy("truncated record".into()))
}

pub fn parse(b: &[u8]) -> Result<Record> {
    if b.len() < 12 || &b[..4] != b"data" {
        return Err(Error::Legacy("record does not start with 'data'".into()));
    }
    let mut rec = Record::default();
    let mut pos = 12;
    while pos + 12 <= b.len() {
        let key = rd_u32(b, pos)?;
        let len = rd_u32(b, pos + 4)? as usize;
        let ty = b[pos + 8];
        let start = pos + 12;
        let end = start
            .checked_add(len)
            .filter(|&e| e <= b.len())
            .ok_or_else(|| Error::Legacy("field overruns record".into()))?;
        let raw = &b[start..end];
        pos = end;
        let field = ((key >> 8) & 0xffff) as u16;
        rec.class_id = (key >> 24) as u8;
        if rec.fields.contains_key(&field) {
            continue;
        }
        let value = match ty {
            0 if len == 8 => Value::Int(i64::from_le_bytes(raw.try_into().unwrap())),
            0 | 6 if len == 4 => Value::Int(i64::from(i32::from_le_bytes(raw.try_into().unwrap()))),
            5 if len == 4 => Value::Bool(i32::from_le_bytes(raw.try_into().unwrap()) != 0),
            5 | 6 if len == 8 => Value::Int(i64::from_le_bytes(raw.try_into().unwrap())),
            1 if len == 8 => Value::Float(f64::from_le_bytes(raw.try_into().unwrap())),
            2 => Value::Text(raw.iter().map(|&c| windows_1252(c)).collect()),
            3 => {
                let units: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                Value::Text(String::from_utf16_lossy(&units))
            }
            _ => Value::Raw(raw.to_vec()),
        };
        rec.fields.insert(field, value);
    }
    Ok(rec)
}

/// AnsiString fields were written with the Windows ANSI code page; the ASCII range is
/// what matters in practice (hashes, URLs, IRC channels) but map the 0x80-0x9f block too.
fn windows_1252(c: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    match c {
        0x80..=0x9f => HIGH[usize::from(c - 0x80)],
        _ => char::from(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(class_id: u8, fields: &[(u16, u8, &[u8])]) -> Vec<u8> {
        let value_bytes = fields
            .iter()
            .map(|(_, _, value)| value.len())
            .sum::<usize>();
        let mut bytes = Vec::with_capacity(12 + (fields.len() * 12) + value_bytes);
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(fields.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(value_bytes as u32).to_le_bytes());

        for &(field_id, value_type, value) in fields {
            let key = (u32::from(class_id) << 24) | (u32::from(field_id) << 8);
            bytes.extend_from_slice(&key.to_le_bytes());
            bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
            bytes.push(value_type);
            bytes.extend_from_slice(&[0; 3]);
            bytes.extend_from_slice(value);
        }

        bytes
    }

    #[test]
    fn parses_synthetic_mylist_record_and_ignores_duplicate_keys() {
        let list_id = 101_i64.to_le_bytes();
        let entry_id = 202_i64.to_le_bytes();
        let watched = 1_i32.to_le_bytes();
        let state = 1_i64.to_le_bytes();
        let later_state = 3_i64.to_le_bytes();
        let b = record(
            7,
            &[
                (1, 0, &list_id),
                (2, 0, &entry_id),
                (7, 5, &watched),
                (11, 0, &state),
                (11, 0, &later_state),
            ],
        );

        let r = parse(&b).unwrap();
        assert_eq!(r.class_id, 7);
        assert_eq!(r.int(1), 101);
        assert_eq!(r.int(2), 202);
        assert!(r.bool(7));
        assert_eq!(r.int(11), 1, "first occurrence wins");
        assert_eq!(r.get(9), None);
    }

    #[test]
    fn decodes_synthetic_wide_and_ansi_strings() {
        let wide: Vec<u8> = "animatic-wide"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let b = record(4, &[(2, 3, &wide), (6, 2, b"animatic-ansi")]);

        let r = parse(&b).unwrap();
        assert_eq!(r.text(2).as_deref(), Some("animatic-wide"));
        assert_eq!(r.text(6).as_deref(), Some("animatic-ansi"));
    }
}
