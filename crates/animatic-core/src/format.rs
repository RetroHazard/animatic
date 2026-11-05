//! Text formatting shared by the UI, exports and statistics.

use chrono::{DateTime, Local, TimeZone, Utc};

/// "1.23 GB" style size with binary units.
pub fn size_short(bytes: i64) -> String {
    const UNITS: [&str; 6] = ["bytes", "KB", "MB", "GB", "TB", "PB"];
    if bytes.abs() < 1024 {
        return format!("{bytes} {}", if bytes == 1 { "byte" } else { "bytes" });
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v.abs() >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    let digits = if v.abs() >= 100.0 {
        0
    } else if v.abs() >= 10.0 {
        1
    } else {
        2
    };
    format!("{v:.digits$} {}", UNITS[u])
}

/// Thousands separated integer: 132823040 -> "132,823,040".
pub fn thousands(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// "132,823,040 bytes (126 MB)" — the legacy hash info panel format.
pub fn size_long(bytes: i64) -> String {
    format!("{} bytes ({})", thousands(bytes), size_short(bytes))
}

/// "2 hours, 15 minutes, 39 seconds" (legacy) or "135 min" when `as_minutes`.
pub fn length(seconds: i64, as_minutes: bool) -> String {
    if as_minutes {
        return format!("{} min", (seconds + 30) / 60);
    }
    if seconds <= 0 {
        return "0s".into();
    }
    let d = seconds / 86_400;
    let h = seconds % 86_400 / 3600;
    let m = seconds % 3600 / 60;
    let s = seconds % 60;
    let mut parts = Vec::new();
    let mut push = |v: i64, unit: &str| {
        if v > 0 {
            parts.push(format!("{v} {unit}{}", if v == 1 { "" } else { "s" }));
        }
    };
    push(d, "day");
    push(h, "hour");
    push(m, "minute");
    push(s, "second");
    parts.join(", ")
}

/// Compact duration for tables: "3d 4h", "1h 05m", "24m".
pub fn length_compact(seconds: i64) -> String {
    if seconds <= 0 {
        return String::new();
    }
    let d = seconds / 86_400;
    let h = seconds % 86_400 / 3600;
    let m = seconds % 3600 / 60;
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{}m", m.max(1))
    }
}

pub fn date(ts: i64) -> String {
    if ts == 0 {
        return String::new();
    }
    match Utc.timestamp_opt(ts, 0).single() {
        Some(t) => t.format("%Y-%m-%d").to_string(),
        None => String::new(),
    }
}

pub fn datetime_local(ts: i64) -> String {
    if ts == 0 {
        return String::new();
    }
    match Local.timestamp_opt(ts, 0).single() {
        Some(t) => t.format("%Y-%m-%d %H:%M").to_string(),
        None => String::new(),
    }
}

pub fn now() -> i64 {
    Utc::now().timestamp()
}

/// Parse "YYYY-MM-DD" (HTTP API dates) into a unix timestamp at midnight UTC.
pub fn parse_date(s: &str) -> i64 {
    let s = s.trim();
    let parts: Vec<i32> = s.split('-').filter_map(|p| p.parse().ok()).collect();
    let (y, m, d) = match parts.as_slice() {
        [y, m, d, ..] => (*y, *m as u32, *d as u32),
        [y, m] => (*y, *m as u32, 1),
        [y] => (*y, 1, 1),
        _ => return 0,
    };
    Utc.with_ymd_and_hms(y, m.max(1), d.max(1), 0, 0, 0)
        .single()
        .map(|t: DateTime<Utc>| t.timestamp())
        .unwrap_or(0)
}

/// AniDB rating (x100 integer) for display: 823 -> "8.23".
pub fn rating(r: i64) -> String {
    if r <= 0 {
        String::new()
    } else {
        format!("{:.2}", r as f64 / 100.0)
    }
}

pub fn percent(part: f64, total: f64) -> String {
    if total <= 0.0 {
        "0.00%".into()
    } else {
        format!("{:.2}%", part * 100.0 / total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_the_legacy_client() {
        assert_eq!(size_long(132_823_040), "132,823,040 bytes (127 MB)");
        assert_eq!(length(8139, false), "2 hours, 15 minutes, 39 seconds");
        assert_eq!(length(60, false), "1 minute");
        assert_eq!(length(8139, true), "136 min");
        assert_eq!(thousands(-1234567), "-1,234,567");
        assert_eq!(size_short(512), "512 bytes");
        assert_eq!(size_short(1536), "1.50 KB");
        assert_eq!(parse_date("1999-01-03"), 915_321_600);
        assert_eq!(date(915_321_600), "1999-01-03");
        assert_eq!(rating(823), "8.23");
    }
}
