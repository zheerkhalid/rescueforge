//! Small helpers: bounds-checked integer readers, formatting, time conversion.
//!
//! Every reader returns `None` instead of panicking when the slice is too
//! short, so parsers can use `?` on data that comes straight from a damaged disk.

pub fn le16(b: &[u8], o: usize) -> Option<u16> {
    let s = b.get(o..o.checked_add(2)?)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

pub fn le32(b: &[u8], o: usize) -> Option<u32> {
    let s = b.get(o..o.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn le64(b: &[u8], o: usize) -> Option<u64> {
    let s = b.get(o..o.checked_add(8)?)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Some(u64::from_le_bytes(a))
}

pub fn be16(b: &[u8], o: usize) -> Option<u16> {
    let s = b.get(o..o.checked_add(2)?)?;
    Some(u16::from_be_bytes([s[0], s[1]]))
}

pub fn be32(b: &[u8], o: usize) -> Option<u32> {
    let s = b.get(o..o.checked_add(4)?)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn be64(b: &[u8], o: usize) -> Option<u64> {
    let s = b.get(o..o.checked_add(8)?)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Some(u64::from_be_bytes(a))
}

/// Decodes UTF-16LE bytes (an odd trailing byte is ignored).
pub fn utf16le(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// "1.4 GB", "512 bytes".
pub fn fmt_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return if n == 1 {
            "1 byte".to_string()
        } else {
            format!("{n} bytes")
        };
    }
    let mut v = n as f64 / 1024.0;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[u])
    } else {
        format!("{v:.2} {}", UNITS[u])
    }
}

/// "12,482".
pub fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "2 h 05 min", "3 min 20 s", "12 s".
pub fn fmt_duration(secs: u64) -> String {
    if secs >= 3600 {
        format!("{} h {:02} min", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{} min {:02} s", secs / 60, secs % 60)
    } else {
        format!("{secs} s")
    }
}

/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn unix_from_civil(y: i64, m: i64, d: i64, h: i64, mi: i64, s: i64) -> i64 {
    days_from_civil(y, m, d) * 86400 + h * 3600 + mi * 60 + s
}

/// `(year, month, day, hour, minute, second)` for a Unix time.
pub fn civil(unix: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = unix.div_euclid(86400);
    let rem = unix.rem_euclid(86400);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 { y + 1 } else { y },
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
    )
}

/// "2026-03-14 09:21", or an empty string when the time is unknown (0).
/// `tz` is the local offset from UTC in seconds.
pub fn fmt_time(unix: i64, tz: i32) -> String {
    if unix <= 0 {
        return String::new();
    }
    let (y, m, d, h, mi, _) = civil(unix + tz as i64);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
}

/// Windows FILETIME (100 ns ticks since 1601) to Unix seconds; 0 when the
/// value is empty or implausible (before 1980 or after 2200).
pub fn filetime_to_unix(ft: u64) -> i64 {
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    if (315_532_800..7_258_118_400).contains(&secs) {
        secs
    } else {
        0
    }
}

/// DOS date and time words to seconds (as if the local time were UTC); 0 when invalid.
pub fn dos_to_unix(date: u16, time: u16) -> i64 {
    let (y, m, d) = (
        1980 + (date >> 9) as i64,
        ((date >> 5) & 15) as i64,
        (date & 31) as i64,
    );
    let (h, mi, s) = (
        (time >> 11) as i64,
        ((time >> 5) & 63) as i64,
        ((time & 31) * 2) as i64,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 {
        return 0;
    }
    unix_from_civil(y, m, d, h, mi, s)
}

/// CRC-32 (IEEE 802.3), used by PNG, ZIP, 7z, xz and the output check.
#[derive(Clone, Copy)]
pub struct Crc32(u32);

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC_TABLE: [u32; 256] = crc_table();

impl Default for Crc32 {
    fn default() -> Self {
        Crc32(0xFFFF_FFFF)
    }
}

impl Crc32 {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn update(&mut self, data: &[u8]) {
        let mut c = self.0;
        for &b in data {
            c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
        self.0 = c;
    }
    pub fn finish(self) -> u32 {
        !self.0
    }
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(data);
    c.finish()
}

/// Lower-case extension without the dot ("" when there is none).
pub fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() && name.len() - i <= 12 => {
            name[i + 1..].to_lowercase()
        }
        _ => String::new(),
    }
}

/// Makes one path component safe to create on Windows.
pub fn sanitize_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 || c == '\u{7f}' => '_',
            c => c,
        })
        .collect();
    while out.ends_with(' ') || out.ends_with('.') {
        out.pop();
    }
    let out_trim = out.trim_start().to_string();
    let mut out = out_trim;
    if out.chars().count() > 180 {
        // Keep the extension when shortening.
        let ext = ext_of(&out);
        let keep: String = out.chars().take(170).collect();
        // Windows drops spaces and dots at the end of a name by itself.
        let keep = keep.trim_end_matches([' ', '.']).to_string();
        out = if ext.is_empty() {
            keep
        } else {
            format!("{keep}.{ext}")
        };
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if out.is_empty() {
        "_".to_string()
    } else if reserved {
        format!("_{out}")
    } else {
        out
    }
}

/// Lower-case hex of a byte slice (session files store small file contents this way).
pub fn to_hex(b: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(HEX[(x >> 4) as usize] as char);
        s.push(HEX[(x & 15) as usize] as char);
    }
    s
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(2) {
        return None;
    }
    let val = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    b.chunks_exact(2)
        .map(|p| Some(val(p[0])? << 4 | val(p[1])?))
        .collect()
}

/// A small deterministic generator for tests, demo data and fuzz-style inputs.
#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0.max(1);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }
    pub fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let v = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        let mut v = vec![0u8; n];
        self.fill(&mut v);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_never_panic() {
        let b = [1u8, 2, 3];
        assert_eq!(le16(&b, 0), Some(0x0201));
        assert_eq!(le16(&b, 2), None);
        assert_eq!(le32(&b, 0), None);
        assert_eq!(le64(&b, usize::MAX), None);
        assert_eq!(be16(&b, 1), Some(0x0203));
        assert_eq!(be32(&b, usize::MAX - 1), None);
    }

    #[test]
    fn sizes_counts_durations() {
        assert_eq!(fmt_size(0), "0 bytes");
        assert_eq!(fmt_size(1), "1 byte");
        assert_eq!(fmt_size(1536), "1.50 KB");
        assert_eq!(fmt_size(20 * 1024 * 1024), "20.0 MB");
        assert_eq!(fmt_size(931 * 1024 * 1024 * 1024), "931 GB");
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(1234567), "1,234,567");
        assert_eq!(fmt_duration(7), "7 s");
        assert_eq!(fmt_duration(200), "3 min 20 s");
        assert_eq!(fmt_duration(7500), "2 h 05 min");
    }

    #[test]
    fn times() {
        assert_eq!(unix_from_civil(1970, 1, 1, 0, 0, 0), 0);
        let t = unix_from_civil(2026, 3, 14, 9, 21, 7);
        assert_eq!(civil(t), (2026, 3, 14, 9, 21, 7));
        assert_eq!(fmt_time(t, 0), "2026-03-14 09:21");
        assert_eq!(fmt_time(t, 3 * 3600), "2026-03-14 12:21");
        assert_eq!(fmt_time(0, 0), "");
        // 2026-03-14 09:21:07 UTC as FILETIME.
        let ft = (t as u64 + 11_644_473_600) * 10_000_000;
        assert_eq!(filetime_to_unix(ft), t);
        assert_eq!(filetime_to_unix(0), 0);
        assert_eq!(filetime_to_unix(u64::MAX), 0);
        let date = ((2026 - 1980) << 9 | 3 << 5 | 14) as u16;
        let time = (9 << 11 | 21 << 5 | 3) as u16;
        assert_eq!(dos_to_unix(date, time), t - 1);
        assert_eq!(dos_to_unix(0, 0), 0);
        assert_eq!(dos_to_unix(0xFFFF, 0xFFFF), 0);
    }

    #[test]
    fn crc_and_hex() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(to_hex(&[0, 0xab, 0xff]), "00abff");
        assert_eq!(from_hex("00ABff"), Some(vec![0, 0xab, 0xff]));
        assert_eq!(from_hex("0"), None);
        assert_eq!(from_hex("zz"), None);
    }

    #[test]
    fn names() {
        assert_eq!(ext_of("Photo.JPG"), "jpg");
        assert_eq!(ext_of(".hidden"), "");
        assert_eq!(ext_of("noext"), "");
        assert_eq!(sanitize_name("a<b>:c?.txt"), "a_b__c_.txt");
        assert_eq!(sanitize_name("con.txt"), "_con.txt");
        assert_eq!(sanitize_name("COM1"), "_COM1");
        assert_eq!(sanitize_name("trail. . "), "trail");
        assert_eq!(sanitize_name(""), "_");
        assert_eq!(sanitize_name("..."), "_");
        let long = "x".repeat(400) + ".jpeg";
        let s = sanitize_name(&long);
        assert!(s.chars().count() <= 180 && s.ends_with(".jpeg"));
        // Shortening never leaves a name that ends in a space or a dot.
        let spaced = format!("{}{}", "y".repeat(160), " ".repeat(60));
        assert_eq!(sanitize_name(&format!("{spaced}tail")), "y".repeat(160));
        assert_eq!(
            sanitize_name(&format!("{spaced}tail.pdf")),
            format!("{}.pdf", "y".repeat(160))
        );
        let kurdish = "ڕاپۆرتی ساڵانە ٢٠٢٥.pdf";
        assert_eq!(sanitize_name(kurdish), kurdish);
        assert_eq!(ext_of(kurdish), "pdf");
    }
}
