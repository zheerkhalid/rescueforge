//! Format readers: each one checks that a candidate really is a file of its
//! type and works out where the file ends from the format's own structure.
//!
//! All of them read through [`Win`], which returns `None` instead of reading
//! past the end, so damaged or random input ends in "not a file", never a panic.

use super::{hit, is_text, partial, strong_magic, Hit, Win};
use crate::util::{be16, be32, crc32, le16, le32, le64};

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

// ------------------------------------------------------------------------------------
// Pictures
// ------------------------------------------------------------------------------------

/// Skips entropy-coded data and returns the position of the next real marker.
fn jpeg_next_marker(w: &mut Win, mut pos: u64, max: u64) -> Option<u64> {
    loop {
        let chunk = w.some(pos, 1 << 16);
        let n = chunk.len();
        if n < 2 {
            return None;
        }
        let mut i = 0usize;
        while i + 1 < n {
            match chunk[i..n - 1].iter().position(|b| *b == 0xFF) {
                None => {
                    i = n - 1;
                }
                Some(j) => {
                    i += j;
                    match chunk[i + 1] {
                        0x00 | 0xD0..=0xD7 => i += 2,
                        0xFF => i += 1,
                        _ => return Some(pos + i as u64),
                    }
                }
            }
        }
        pos += i as u64;
        if pos > max {
            return None;
        }
    }
}

pub fn jpeg(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 512 << 20;
    let broken = |pos: u64, scans: u32| {
        // The picture data was cut off: keep what is there if it is substantial.
        if scans > 0 && pos >= 4096 {
            partial(pos, "jpg")
        } else {
            None
        }
    };
    let mut pos = 2u64;
    let mut frame = false;
    let mut scans = 0u32;
    loop {
        if pos > MAX {
            return None;
        }
        let Some(m) = w.get(pos, 2) else {
            return broken(w.limit(), scans);
        };
        if m[0] != 0xFF {
            return broken(pos, scans);
        }
        let k = m[1];
        match k {
            0xFF => {
                pos += 1;
                continue;
            }
            0xD9 => return if scans > 0 { hit(pos + 2, "jpg") } else { None },
            0xD0..=0xD7 | 0x01 => {
                pos += 2;
                continue;
            }
            0xC0..=0xCF | 0xDA..=0xDF | 0xE0..=0xEF | 0xFE => {}
            _ => return broken(pos, scans),
        }
        let Some(len) = w.be16(pos + 2) else {
            return broken(w.limit(), scans);
        };
        if len < 2 {
            return broken(pos, scans);
        }
        if matches!(k, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            if len < 8 {
                return None;
            }
            frame = true;
        }
        pos += 2 + len as u64;
        if k == 0xDA {
            if !frame {
                return None;
            }
            scans += 1;
            pos = match jpeg_next_marker(w, pos, MAX) {
                Some(p) => p,
                None => return broken(w.limit().min(MAX), scans),
            };
        }
    }
}

pub fn png(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 1 << 30;
    let head = w.get(8, 25)?;
    if be32(head, 0)? != 13 || &head[4..8] != b"IHDR" {
        return None;
    }
    if be32(head, 8)? == 0 || be32(head, 12)? == 0 || crc32(&head[4..21]) != be32(head, 21)? {
        return None;
    }
    let mut pos = 8u64;
    loop {
        let h = w.get(pos, 8)?;
        let len = be32(h, 0)? as u64;
        let ty = [h[4], h[5], h[6], h[7]];
        if len > 0x7FFF_FFFF || !ty.iter().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        pos += 12 + len;
        if pos > MAX || pos > w.limit() {
            return None;
        }
        if &ty == b"IEND" {
            return hit(pos, "png");
        }
    }
}

pub fn gif(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 512 << 20;
    let h = w.get(0, 13)?;
    if !(&h[..6] == b"GIF87a" || &h[..6] == b"GIF89a") || le16(h, 6)? == 0 || le16(h, 8)? == 0 {
        return None;
    }
    let flags = h[10];
    let mut pos = 13u64;
    if flags & 0x80 != 0 {
        pos += 3u64 << ((flags & 7) + 1);
    }
    let sub_blocks = |w: &mut Win, mut pos: u64| -> Option<u64> {
        loop {
            let n = w.u8(pos)? as u64;
            pos += 1 + n;
            if n == 0 {
                return Some(pos);
            }
            if pos > MAX {
                return None;
            }
        }
    };
    let mut images = 0;
    loop {
        match w.u8(pos)? {
            0x3B => {
                return if images > 0 {
                    hit(pos + 1, "gif")
                } else {
                    None
                }
            }
            0x21 => pos = sub_blocks(w, pos + 2)?,
            0x2C => {
                let d = w.get(pos, 10)?;
                let lf = d[9];
                pos += 10;
                if lf & 0x80 != 0 {
                    pos += 3u64 << ((lf & 7) + 1);
                }
                let code_size = w.u8(pos)?;
                if !(1..=12).contains(&code_size) {
                    return None;
                }
                pos = sub_blocks(w, pos + 1)?;
                images += 1;
            }
            _ => return None,
        }
        if pos > MAX {
            return None;
        }
    }
}

pub fn bmp(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 34)?;
    let size = le32(h, 2)? as u64;
    let data = le32(h, 10)? as u64;
    let dib = le32(h, 14)? as u64;
    if le32(h, 6)? != 0 || ![12, 40, 52, 56, 64, 108, 124].contains(&dib) {
        return None;
    }
    let (width, height, planes, bpp) = if dib == 12 {
        (
            le16(h, 18)? as i64,
            le16(h, 20)? as i64,
            le16(h, 22)?,
            le16(h, 24)?,
        )
    } else {
        (
            le32(h, 18)? as i32 as i64,
            (le32(h, 22)? as i32 as i64).abs(),
            le16(h, 26)?,
            le16(h, 28)?,
        )
    };
    if planes != 1
        || ![1, 4, 8, 16, 24, 32].contains(&bpp)
        || !(1..=65_535).contains(&width)
        || !(1..=65_535).contains(&height)
        || data < 14 + dib
    {
        return None;
    }
    let compression = if dib >= 40 { le32(h, 30)? } else { 0 };
    let total = if compression == 0 || compression == 3 {
        let row = (width as u64 * bpp as u64).div_ceil(32) * 4;
        let expected = data + row * height as u64;
        if size != 0 && (size < expected || size > expected + 65_536) {
            return None;
        }
        expected.max(size)
    } else {
        if size <= data {
            return None;
        }
        size
    };
    if total > 1 << 30 {
        return None;
    }
    hit(total, "bmp")
}

/// TIFF and the camera RAW formats built on it.
pub fn tiff(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 2 << 30;
    let h = w.get(0, 16)?.to_vec();
    let le = h[0] == b'I';
    let r16 = |b: &[u8], o: usize| if le { le16(b, o) } else { be16(b, o) };
    let r32 = |b: &[u8], o: usize| if le { le32(b, o) } else { be32(b, o) };
    let mut ext = if &h[8..10] == b"CR" {
        "cr2"
    } else if &h[..4] == b"IIRO" || &h[..4] == b"IIRS" {
        "orf"
    } else if &h[..4] == b"IIU\0" {
        "rw2"
    } else {
        "tif"
    };
    const SIZES: [u64; 14] = [0, 1, 1, 2, 4, 8, 1, 1, 2, 4, 8, 4, 8, 4];
    let mut queue = vec![r32(&h, 4)? as u64];
    let mut seen: Vec<u64> = Vec::new();
    let mut end = 8u64;
    let mut make = String::new();
    let mut dng = false;
    while let Some(ifd) = queue.pop() {
        if seen.contains(&ifd) || seen.len() >= 64 || ifd < 8 {
            continue;
        }
        let first = seen.is_empty();
        seen.push(ifd);
        let Some(count) = w.get(ifd, 2).and_then(|b| r16(b, 0)) else {
            if first {
                return None;
            }
            continue;
        };
        let count = count as u64;
        if count == 0 || count > 2000 {
            if first {
                return None;
            }
            continue;
        }
        let Some(table) = w
            .get(ifd + 2, (count * 12 + 4) as usize)
            .map(|b| b.to_vec())
        else {
            if first {
                return None;
            }
            continue;
        };
        end = end.max(ifd + 2 + count * 12 + 4);
        // Offsets and byte counts of strips / tiles / embedded JPEG.
        let mut offsets: Vec<u64> = Vec::new();
        let mut counts: Vec<u64> = Vec::new();
        for e in table[..(count * 12) as usize].chunks_exact(12) {
            let tag = r16(e, 0)?;
            let ty = r16(e, 2)? as usize;
            let n = r32(e, 4)? as u64;
            if ty == 0 || ty >= SIZES.len() {
                if first {
                    return None;
                }
                continue;
            }
            let bytes = n.checked_mul(SIZES[ty])?;
            let value = r32(e, 8)? as u64;
            if bytes > 4 {
                if value + bytes > w.limit() || bytes > MAX {
                    if first {
                        return None;
                    }
                    continue;
                }
                end = end.max(value + bytes);
            }
            // Reads an array of SHORT or LONG values.
            let values = |w: &mut Win| -> Vec<u64> {
                let n = n.min(100_000) as usize;
                let size = SIZES[ty] as usize;
                if size != 2 && size != 4 {
                    return Vec::new();
                }
                let raw: Vec<u8> = if bytes <= 4 {
                    e[8..12].to_vec()
                } else {
                    match w.get(value, n * size) {
                        Some(b) => b.to_vec(),
                        None => return Vec::new(),
                    }
                };
                raw.chunks_exact(size)
                    .take(n)
                    .filter_map(|c| {
                        if size == 2 {
                            r16(c, 0).map(u64::from)
                        } else {
                            r32(c, 0).map(u64::from)
                        }
                    })
                    .collect()
            };
            match tag {
                0x0111 | 0x0144 | 0x0201 => offsets = values(w),
                0x0117 | 0x0145 | 0x0202 => counts = values(w),
                0x014A | 0x8769 | 0x8825 => queue.extend(values(w)),
                0x010F if bytes > 4 && bytes < 64 => {
                    if let Some(b) = w.get(value, bytes as usize) {
                        make = String::from_utf8_lossy(b).to_uppercase();
                    }
                }
                0xC612 => dng = true,
                _ => {}
            }
            if offsets.len() == counts.len() && !offsets.is_empty() {
                for (o, c) in offsets.iter().zip(&counts) {
                    if o + c <= w.limit() {
                        end = end.max(o + c);
                    }
                }
                offsets.clear();
                counts.clear();
            }
        }
        let next = r32(&table, (count * 12) as usize)? as u64;
        if next != 0 {
            queue.insert(0, next);
        }
    }
    if ext == "tif" {
        ext = if dng {
            "dng"
        } else if make.starts_with("NIKON") {
            "nef"
        } else if make.starts_with("SONY") {
            "arw"
        } else if make.starts_with("PENTAX") || make.starts_with("RICOH") {
            "pef"
        } else if make.starts_with("SAMSUNG") {
            "srw"
        } else {
            "tif"
        };
    }
    if !(1024..=MAX).contains(&end) {
        return None;
    }
    hit(end, ext)
}

pub fn psd(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 26)?;
    let version = be16(h, 4)?;
    let channels = be16(h, 12)? as u64;
    let height = be32(h, 14)? as u64;
    let width = be32(h, 18)? as u64;
    let depth = be16(h, 22)? as u64;
    if !(version == 1 || version == 2)
        || h[6..12].iter().any(|b| *b != 0)
        || !(1..=56).contains(&channels)
        || !(1..=300_000).contains(&height)
        || !(1..=300_000).contains(&width)
        || ![1, 8, 16, 32].contains(&depth)
        || be16(h, 24)? > 15
    {
        return None;
    }
    let mut pos = 26u64;
    pos += 4 + w.be32(pos)? as u64;
    pos += 4 + w.be32(pos)? as u64;
    pos = pos.checked_add(if version == 1 {
        4 + w.be32(pos)? as u64
    } else {
        8u64.checked_add(w.be64(pos)?)?
    })?;
    let compression = w.be16(pos)?;
    pos += 2;
    let rows = channels * height;
    match compression {
        0 => pos += rows * (width * depth).div_ceil(8),
        1 => {
            // A table with the packed length of every row, then the rows.
            let entry = if version == 1 { 2u64 } else { 4 };
            let mut total = 0u64;
            let mut done = 0u64;
            while done < rows {
                let n = (rows - done).min(16_384);
                let b = w.get(pos + done * entry, (n * entry) as usize)?;
                for c in b.chunks_exact(entry as usize) {
                    total += if entry == 2 {
                        be16(c, 0)? as u64
                    } else {
                        be32(c, 0)? as u64
                    };
                }
                done += n;
            }
            pos += rows * entry + total;
        }
        _ => return None,
    }
    if pos > 8 << 30 {
        return None;
    }
    hit(pos, if version == 1 { "psd" } else { "psb" })
}

pub fn raf(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 108)?;
    let mut end = 0u64;
    for o in [84usize, 92, 100] {
        let off = be32(h, o)? as u64;
        let len = be32(h, o + 4)? as u64;
        if off != 0 {
            end = end.max(off + len);
        }
    }
    if !(4096..=1 << 30).contains(&end) {
        return None;
    }
    hit(end, "raf")
}

// ------------------------------------------------------------------------------------
// Containers: RIFF, ISO base media (MP4), Matroska, ASF
// ------------------------------------------------------------------------------------

pub fn riff(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 16)?;
    let size = le32(h, 4)? as u64;
    let ext = match &h[8..12] {
        b"AVI " => "avi",
        b"WAVE" => "wav",
        b"WEBP" => "webp",
        _ => return None,
    };
    if size < 12 || size == 0xFFFF_FFFF || !h[12..16].iter().all(|c| (0x20..0x7F).contains(c)) {
        return None;
    }
    let mut total = 8 + size + (size & 1);
    if total > w.limit() {
        // An odd length without the padding byte at the very end of the source.
        total = 8 + size;
    }
    if ext == "avi" {
        // Large AVI files continue in further "RIFF....AVIX" blocks.
        while let Some(n) = w.get(total, 12) {
            if &n[..4] != b"RIFF" || &n[8..12] != b"AVIX" {
                break;
            }
            let s = le32(n, 4)? as u64;
            total += 8 + s + (s & 1);
        }
    }
    hit(total, ext)
}

pub fn aiff(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 12)?;
    let size = be32(h, 4)? as u64;
    if !(&h[8..12] == b"AIFF" || &h[8..12] == b"AIFC") || size < 12 {
        return None;
    }
    hit(8 + size + (size & 1), "aif")
}

pub fn mp4(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 256 << 30;
    const KNOWN: [&[u8; 4]; 20] = [
        b"ftyp", b"moov", b"mdat", b"free", b"skip", b"wide", b"uuid", b"meta", b"moof", b"mfra",
        b"sidx", b"styp", b"pnot", b"PICT", b"pdin", b"udta", b"emsg", b"prft", b"ssix", b"junk",
    ];
    let first = w.get(0, 16)?.to_vec();
    let mut brand = [0u8; 4];
    let mut compat_avif = false;
    if &first[4..8] == b"ftyp" {
        let size = be32(&first, 0)? as u64;
        if !(12..=4096).contains(&size) {
            return None;
        }
        brand.copy_from_slice(&first[8..12]);
        if !brand
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b' ')
        {
            return None;
        }
        let body = w.get(0, size as usize)?;
        compat_avif = body[8..]
            .chunks_exact(4)
            .any(|c| c == b"avif" || c == b"avis");
    } else if &first[4..8] == b"moov" {
        // QuickTime without a type box: the movie header follows directly.
        if !matches!(
            &first[12..16],
            b"mvhd" | b"cmov" | b"trak" | b"udta" | b"meta"
        ) {
            return None;
        }
    }
    let (mut pos, mut moov, mut mdat, mut meta) = (0u64, false, false, false);
    while let Some(h) = w.get(pos, 8) {
        let size32 = be32(h, 0)? as u64;
        let ty = [h[4], h[5], h[6], h[7]];
        if !KNOWN.iter().any(|k| **k == ty) {
            break;
        }
        let size = match size32 {
            1 => match w.be64(pos + 8) {
                Some(s) if s >= 16 => s,
                _ => break,
            },
            0..=7 => break,
            s => s,
        };
        if size > w.limit() - pos || pos + size > MAX {
            break;
        }
        match &ty {
            b"moov" => moov = true,
            b"mdat" => mdat = true,
            b"meta" => meta = true,
            _ => {}
        }
        pos += size;
    }
    let still = matches!(
        &brand,
        b"heic"
            | b"heix"
            | b"heim"
            | b"heis"
            | b"hevc"
            | b"hevx"
            | b"mif1"
            | b"msf1"
            | b"avif"
            | b"avis"
    );
    if !(moov && mdat || moov && pos > 1024 || still && meta) {
        return None;
    }
    let ext = match &brand {
        b"avif" | b"avis" => "avif",
        _ if still && compat_avif => "avif",
        _ if still => "heic",
        b"qt  " | [0, 0, 0, 0] => "mov",
        b"M4A " | b"M4B " | b"M4P " => "m4a",
        b"M4V " | b"M4VH" | b"M4VP" => "m4v",
        b"crx " => "cr3",
        [b'3', b'g', ..] => "3gp",
        _ => "mp4",
    };
    hit(pos, ext)
}

/// EBML variable-length integer: `(value, length, all-ones "unknown")`.
fn vint(w: &mut Win, pos: u64) -> Option<(u64, u64, bool)> {
    let first = w.u8(pos)?;
    let len = first.leading_zeros() as u64 + 1;
    if len > 8 {
        return None;
    }
    let b = w.get(pos, len as usize)?;
    let mut v = (first as u64) & (0xFFu64 >> len);
    let mut ones = v == (0xFFu64 >> len);
    for x in &b[1..] {
        v = v << 8 | *x as u64;
        ones &= *x == 0xFF;
    }
    Some((v, len, ones))
}

fn ebml_id(w: &mut Win, pos: u64) -> Option<(u32, u64)> {
    let first = w.u8(pos)?;
    let len = first.leading_zeros() as u64 + 1;
    if len > 4 {
        return None;
    }
    let b = w.get(pos, len as usize)?;
    Some((b.iter().fold(0u32, |a, x| a << 8 | *x as u32), len))
}

pub fn mkv(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 256 << 30;
    let (hsize, hl, unknown) = vint(w, 4)?;
    if unknown || hsize > 4096 {
        return None;
    }
    let header = w.get(4 + hl, hsize as usize)?;
    let webm = find(header, b"webm").is_some();
    if !webm && find(header, b"matroska").is_none() {
        return None;
    }
    let seg = 4 + hl + hsize;
    if ebml_id(w, seg)? != (0x1853_8067, 4) {
        return None;
    }
    let (ssize, sl, unknown) = vint(w, seg + 4)?;
    let body = seg + 4 + sl;
    let ext = if webm { "webm" } else { "mkv" };
    if !unknown {
        let total = body + ssize;
        return if total <= MAX { hit(total, ext) } else { None };
    }
    // A segment of unknown length (live recording): walk its top-level parts.
    const TOP: [u32; 10] = [
        0x114D_9B74,
        0x1549_A966,
        0x1654_AE6B,
        0x1F43_B675,
        0x1C53_BB6B,
        0x1941_A469,
        0x1043_A770,
        0x1254_C367,
        0xEC,
        0xBF,
    ];
    let mut pos = body;
    let mut clusters = 0;
    while let Some((id, il)) = ebml_id(w, pos) {
        if !TOP.contains(&id) {
            break;
        }
        let Some((size, l, unknown)) = vint(w, pos + il) else {
            break;
        };
        if unknown || pos + il + l + size > w.limit().min(MAX) {
            break;
        }
        if id == 0x1F43_B675 {
            clusters += 1;
        }
        pos += il + l + size;
    }
    if clusters == 0 {
        return None;
    }
    hit(pos, ext)
}

pub const ASF_HEADER: [u8; 16] = [
    0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
];
const ASF_DATA: [u8; 16] = [
    0x36, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
];
const ASF_FILE_PROPS: [u8; 16] = [
    0xA1, 0xDC, 0xAB, 0x8C, 0x47, 0xA9, 0xCF, 0x11, 0x8E, 0xE4, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65,
];
const ASF_STREAM_PROPS: [u8; 16] = [
    0x91, 0x07, 0xDC, 0xB7, 0xB7, 0xA9, 0xCF, 0x11, 0x8E, 0xE6, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65,
];
const ASF_VIDEO: [u8; 16] = [
    0xC0, 0xEF, 0x19, 0xBC, 0x4D, 0x5B, 0xCF, 0x11, 0xA8, 0xFD, 0x00, 0x80, 0x5F, 0x5C, 0x44, 0x2B,
];

pub fn asf(w: &mut Win) -> Option<Hit> {
    let hsize = w.le64(16)?;
    if !(30..=64 << 20).contains(&hsize) {
        return None;
    }
    let header = w.get(0, hsize as usize)?.to_vec();
    let mut pos = 30usize;
    let mut file_size = 0u64;
    let mut video = false;
    while pos + 24 <= header.len() {
        let size = le64(&header, pos + 16)? as usize;
        if size < 24 || size > header.len() - pos {
            break;
        }
        let obj = &header[pos..pos + size];
        if obj[..16] == ASF_FILE_PROPS {
            file_size = le64(obj, 40).unwrap_or(0);
        } else if obj[..16] == ASF_STREAM_PROPS && obj.len() >= 40 && obj[24..40] == ASF_VIDEO {
            video = true;
        }
        pos += size;
    }
    let ext = if video { "wmv" } else { "wma" };
    if file_size >= hsize && file_size <= w.limit() {
        return hit(file_size, ext);
    }
    // Live or broken header: header object + data object + whatever index follows.
    let d = w.get(hsize, 24)?;
    if d[..16] != ASF_DATA {
        return None;
    }
    let dsize = le64(d, 16)?;
    if dsize < 50 || dsize > w.limit() - hsize {
        return None;
    }
    hit(hsize + dsize, ext)
}

pub fn flv(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 9)?;
    if h[4] & 0xFA != 0 || be32(h, 5)? != 9 {
        return None;
    }
    let mut pos = 9u64;
    let mut tags = 0u32;
    let mut prev = 0u32;
    while let Some(t) = w.get(pos, 15) {
        if be32(t, 0)? != prev || !matches!(t[4], 8 | 9 | 18) || t[12..15] != [0, 0, 0] {
            break;
        }
        let size = (t[5] as u32) << 16 | (t[6] as u32) << 8 | t[7] as u32;
        prev = 11 + size;
        pos += 4 + prev as u64;
        tags += 1;
        if pos > 64 << 30 {
            return None;
        }
    }
    if tags < 2 {
        return None;
    }
    // The last "previous tag size" field closes the file.
    let end = match w.be32(pos) {
        Some(p) if p == prev => pos + 4,
        _ => pos,
    };
    hit(end, "flv")
}

pub fn mpeg_ts(w: &mut Win) -> Option<Hit> {
    let head = w.get(0, 8)?;
    let (packet, lead, ext) = if head[0] == 0x47 && head[4] != 0x47 {
        (188u64, 0u64, "ts")
    } else if head[4] == 0x47 {
        (192, 4, "m2ts")
    } else {
        (188, 0, "ts")
    };
    let mut count = 0u64;
    'outer: loop {
        let batch = 512u64;
        let chunk = w.some(count * packet, (batch * packet) as usize);
        let whole = chunk.len() as u64 / packet;
        for i in 0..whole {
            if chunk[(i * packet + lead) as usize] != 0x47 {
                count += i;
                break 'outer;
            }
        }
        count += whole;
        if whole < batch || count * packet > 256 << 30 {
            break;
        }
    }
    // Short runs of 0x47 at the right spacing happen by chance; a real
    // recording has thousands of packets.
    if count < 2800 {
        return None;
    }
    hit(count * packet, ext)
}

// ------------------------------------------------------------------------------------
// Audio
// ------------------------------------------------------------------------------------

/// `(frame length, version, layer, sample rate)` of an MPEG audio frame header.
fn mp3_frame(h: &[u8]) -> Option<(u64, u8, u8, u32)> {
    if h.len() < 4 || h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = (h[1] >> 3) & 3; // 0 = 2.5, 2 = 2, 3 = 1
    let layer = (h[1] >> 1) & 3; // 1 = III, 2 = II, 3 = I
    let bitrate_index = (h[2] >> 4) as usize;
    let rate_index = ((h[2] >> 2) & 3) as usize;
    if version == 1 || layer == 0 || bitrate_index == 0 || bitrate_index == 15 || rate_index == 3 {
        return None;
    }
    const V1_L1: [u32; 14] = [
        32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
    ];
    const V1_L2: [u32; 14] = [
        32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
    ];
    const V1_L3: [u32; 14] = [
        32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
    ];
    const V2_L1: [u32; 14] = [
        32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
    ];
    const V2_L23: [u32; 14] = [8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
    let table = match (version, layer) {
        (3, 3) => V1_L1,
        (3, 2) => V1_L2,
        (3, 1) => V1_L3,
        (_, 3) => V2_L1,
        _ => V2_L23,
    };
    let bitrate = table[bitrate_index - 1] * 1000;
    let rate = match version {
        3 => [44_100, 48_000, 32_000][rate_index],
        2 => [22_050, 24_000, 16_000][rate_index],
        _ => [11_025, 12_000, 8_000][rate_index],
    };
    let pad = ((h[2] >> 1) & 1) as u32;
    let len = match (layer, version) {
        (3, _) => (12 * bitrate / rate + pad) * 4,
        (2, _) | (1, 3) => 144 * bitrate / rate + pad,
        _ => 72 * bitrate / rate + pad,
    };
    if len < 24 {
        return None;
    }
    Some((len as u64, version, layer, rate))
}

pub fn mp3(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 2 << 30;
    let mut pos = 0u64;
    let h = w.get(0, 10)?;
    let tagged = &h[..3] == b"ID3";
    if tagged {
        if !(2..=4).contains(&h[3]) || h[4] == 0xFF || h[6..10].iter().any(|b| *b & 0x80 != 0) {
            return None;
        }
        let size = (h[6] as u64) << 21 | (h[7] as u64) << 14 | (h[8] as u64) << 7 | h[9] as u64;
        pos = 10 + size + if h[5] & 0x10 != 0 { 10 } else { 0 };
        // Some writers leave padding between the tag and the first frame.
        let pad = w.some(pos, 4096);
        pos += pad.iter().take_while(|b| **b == 0).count() as u64;
    }
    let mut frames = 0u32;
    let mut kind: Option<(u8, u8, u32)> = None;
    while let Some(h) = w.get(pos, 4) {
        let Some((len, version, layer, rate)) = mp3_frame(h) else {
            break;
        };
        match kind {
            None => kind = Some((version, layer, rate)),
            Some(k) if k != (version, layer, rate) => break,
            _ => {}
        }
        if pos + len > w.limit() {
            break;
        }
        pos += len;
        frames += 1;
        if pos > MAX {
            return None;
        }
    }
    // Without a tag only a long run of consistent Layer III frames is believed.
    let enough = if tagged {
        frames >= 4
    } else {
        frames >= 32 && kind.map(|k| k.1) == Some(1)
    };
    if !enough {
        return None;
    }
    if w.get(pos, 3) == Some(b"TAG") && pos + 128 <= w.limit() {
        pos += 128;
    }
    hit(pos, "mp3")
}

pub fn flac(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 4 << 30;
    let info = w.get(4, 38)?;
    if info[0] & 0x7F != 0 || info[1..4] != [0, 0, 34] {
        return None;
    }
    let s = &info[4..];
    let rate = (s[10] as u32) << 12 | (s[11] as u32) << 4 | (s[12] as u32) >> 4;
    let channels = ((s[12] >> 1) & 7) as u64 + 1;
    let bits = (((s[12] & 1) << 4) | (s[13] >> 4)) as u64 + 1;
    let samples = ((s[13] & 0x0F) as u64) << 32 | be32(s, 14)? as u64;
    if rate == 0 || rate > 655_350 {
        return None;
    }
    let mut pos = 4u64;
    for _ in 0..512 {
        let h = w.get(pos, 4)?;
        let last = h[0] & 0x80 != 0;
        if h[0] & 0x7F == 127 {
            return None;
        }
        pos += 4 + ((h[1] as u64) << 16 | (h[2] as u64) << 8 | h[3] as u64);
        if last {
            break;
        }
    }
    let sync = w.get(pos, 2)?;
    if sync[0] != 0xFF || sync[1] & 0xFC != 0xF8 {
        return None;
    }
    // FLAC does not store its length. The audio cannot be larger than the
    // uncompressed samples; inside that bound it ends where a blank sector or
    // another file begins.
    let bound = if samples > 0 {
        pos + samples * channels * bits.div_ceil(8) + 65_536
    } else {
        MAX
    }
    .min(MAX)
    .min(w.limit());
    let mut end = pos.div_ceil(512) * 512;
    while end < bound {
        let sec = w.some(end, 64);
        if sec.iter().all(|b| *b == 0) || strong_magic(sec) {
            break;
        }
        end += 512;
    }
    Some(Hit {
        size: end.min(bound),
        ext: "flac",
        partial: false,
        estimated: true,
    })
}

pub fn ogg(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 16 << 30;
    let mut pos = 0u64;
    let mut pages = 0u32;
    let mut ext = "ogg";
    while let Some(h) = w.get(pos, 27) {
        if &h[..4] != b"OggS" || h[4] != 0 || h[5] & 0xF8 != 0 {
            break;
        }
        if pages == 0 && h[5] & 0x02 == 0 {
            return None;
        }
        let nseg = h[26] as usize;
        let Some(segs) = w.get(pos + 27, nseg) else {
            break;
        };
        let body: u64 = segs.iter().map(|b| *b as u64).sum();
        let start = pos + 27 + nseg as u64;
        if start + body > w.limit() {
            break;
        }
        if pages == 0 {
            let p = w.some(start, 16);
            ext = if p.starts_with(b"OpusHead") {
                "opus"
            } else if p.starts_with(b"\x80theora") {
                "ogv"
            } else if p.starts_with(b"\x7fFLAC") {
                "oga"
            } else {
                "ogg"
            };
        }
        pos = start + body;
        pages += 1;
        if pos > MAX {
            return None;
        }
    }
    if pages < 3 {
        return None;
    }
    hit(pos, ext)
}

pub fn midi(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 14)?;
    let tracks = be16(h, 10)?;
    if be32(h, 4)? != 6 || be16(h, 8)? > 2 || tracks == 0 {
        return None;
    }
    let mut pos = 14u64;
    for _ in 0..tracks {
        let t = w.get(pos, 8)?;
        if &t[..4] != b"MTrk" {
            return None;
        }
        pos += 8 + be32(t, 4)? as u64;
    }
    hit(pos, "mid")
}

// ------------------------------------------------------------------------------------
// Documents
// ------------------------------------------------------------------------------------

/// Does the text after an end-of-file mark look like a further PDF revision?
fn pdf_continues(after: &[u8]) -> bool {
    let t: Vec<u8> = after
        .iter()
        .copied()
        .skip_while(|b| b.is_ascii_whitespace())
        .take(40)
        .collect();
    if t.starts_with(b"xref") || t.starts_with(b"startxref") || t.starts_with(b"trailer") {
        return true;
    }
    // "12 0 obj"
    let mut i = 0;
    let digits = |i: &mut usize| {
        let s = *i;
        while *i < t.len() && t[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > s
    };
    digits(&mut i)
        && t.get(i) == Some(&b' ')
        && {
            i += 1;
            digits(&mut i)
        }
        && t[i..].starts_with(b" obj")
}

pub fn pdf(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 2 << 30;
    let head = w.some(0, 2048).to_vec();
    if head.len() < 16 || !head[5].is_ascii_digit() || head[6] != b'.' {
        return None;
    }
    // A "linearized" PDF states its total length in the first object.
    if let Some(p) = find(&head, b"/Linearized") {
        if let Some(l) = find(&head[p..], b"/L ") {
            let digits: String = head[p + l + 3..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .map(|b| *b as char)
                .collect();
            if let Ok(len) = digits.parse::<u64>() {
                if len > 64 && len <= w.limit() && len <= MAX {
                    let tail = w.get(len - 32, 32)?;
                    if find(tail, b"%%EOF").is_some() {
                        return hit(len, "pdf");
                    }
                }
            }
        }
    }
    let mut pos = 0u64;
    let limit = w.limit();
    loop {
        let chunk = w.some(pos, 1 << 20);
        if chunk.len() < 5 {
            return None;
        }
        let n = chunk.len();
        let mut from = 0usize;
        while let Some(i) = find(&chunk[from..], b"%%EOF") {
            let mut end = from + i + 5;
            // The mark is followed by at most one line break.
            if chunk.get(end) == Some(&b'\r') {
                end += 1;
            }
            if chunk.get(end) == Some(&b'\n') {
                end += 1;
            }
            if end + 48 > n && (n as u64) < limit - pos && n == 1 << 20 {
                // Too close to the edge of this block to judge: re-read from here.
                break;
            }
            if !pdf_continues(&chunk[end.min(n)..]) {
                return hit(pos + end as u64, "pdf");
            }
            from = end;
        }
        if (n as u64) >= limit - pos {
            return None;
        }
        // Step on, keeping enough overlap for a mark that straddles two blocks.
        pos += (from.max(n - 64)) as u64;
        if pos > MAX {
            return None;
        }
    }
}

/// OLE compound files: DOC, XLS, PPT, MSG and friends.
pub fn ole(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 512)?.to_vec();
    let major = le16(&h, 26)?;
    let shift = le16(&h, 30)?;
    if le16(&h, 28)? != 0xFFFE
        || !((major == 3 && shift == 9) || (major == 4 && shift == 12))
        || le16(&h, 32)? != 6
    {
        return None;
    }
    let ss = 1u64 << shift;
    let per = (ss / 4) as usize;
    let nfat = le32(&h, 44)? as usize;
    let dir_start = le32(&h, 48)? as u64;
    let mut difat_sector = le32(&h, 68)? as u64;
    let ndifat = le32(&h, 72)? as usize;
    if nfat == 0 || nfat > 1 << 20 || ndifat > 1 << 14 {
        return None;
    }
    let sector_off = |n: u64| (n + 1) * ss;
    // Where the sectors of the allocation table are.
    let mut fat_sectors: Vec<u64> = (0..109)
        .filter_map(|i| le32(&h, 76 + i * 4))
        .filter(|v| *v < 0xFFFF_FFF0)
        .map(u64::from)
        .take(nfat)
        .collect();
    let mut hops = 0;
    while fat_sectors.len() < nfat && difat_sector < 0xFFFF_FFF0 && hops <= ndifat {
        let d = w.get(sector_off(difat_sector), ss as usize)?.to_vec();
        for i in 0..per - 1 {
            let v = le32(&d, i * 4)?;
            if v < 0xFFFF_FFF0 && fat_sectors.len() < nfat {
                fat_sectors.push(v as u64);
            }
        }
        difat_sector = le32(&d, (per - 1) * 4)? as u64;
        hops += 1;
    }
    if fat_sectors.len() != nfat {
        return None;
    }
    // The file ends after the last sector the table marks as used.
    let mut last_used: Option<u64> = None;
    let mut first_fat: Vec<u8> = Vec::new();
    for (i, s) in fat_sectors.iter().enumerate() {
        let f = w.get(sector_off(*s), ss as usize)?;
        if i == 0 {
            first_fat = f.to_vec();
        }
        for (j, e) in f.chunks_exact(4).enumerate() {
            if e != [0xFF, 0xFF, 0xFF, 0xFF] {
                last_used = Some((i * per + j) as u64);
            }
        }
    }
    let size = (last_used? + 2) * ss;
    if size < 3 * ss || size > 2 << 30 {
        return None;
    }
    // The directory tells the kind of document. Its first sectors are enough.
    let mut names: Vec<String> = Vec::new();
    let mut sector = dir_start;
    for _ in 0..8 {
        if sector >= 0xFFFF_FFF0 {
            break;
        }
        let Some(d) = w.get(sector_off(sector), ss as usize) else {
            break;
        };
        for e in d.chunks_exact(128) {
            let len = (le16(e, 64).unwrap_or(0) as usize).min(64);
            if len >= 2 && e[66] != 0 {
                names.push(crate::util::utf16le(&e[..len - 2]));
            }
        }
        sector = match le32(&first_fat, sector as usize * 4) {
            Some(n) if (sector as usize) < per => n as u64,
            _ => break,
        };
    }
    if names.is_empty() {
        return None;
    }
    // Windows Installer packages are told apart by the class id of the root entry.
    const MSI: [u8; 16] = [
        0x84, 0x10, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    let installer = w
        .get(sector_off(dir_start) + 80, 16)
        .map(|c| c == MSI)
        .unwrap_or(false);
    let has = |n: &str| names.iter().any(|x| x == n);
    let ext = if installer {
        "msi"
    } else if has("WordDocument") {
        "doc"
    } else if has("Workbook") || has("Book") {
        "xls"
    } else if has("PowerPoint Document") {
        "ppt"
    } else if names.iter().any(|x| x.starts_with("__substg1.0_")) {
        "msg"
    } else if has("VisioDocument") {
        "vsd"
    } else if has("Quill") {
        "pub"
    } else {
        "ole"
    };
    hit(size, ext)
}

pub fn rtf(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 512 << 20;
    let mut pos = 0u64;
    let mut depth = 0i64;
    let mut escaped = false;
    loop {
        let chunk = w.some(pos, 1 << 16);
        if chunk.is_empty() {
            return None;
        }
        for (i, b) in chunk.iter().enumerate() {
            if escaped {
                escaped = false;
                continue;
            }
            match b {
                b'\\' => escaped = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        // Keep the line break that usually follows the last brace.
                        let mut end = i + 1;
                        if chunk.get(end) == Some(&b'\r') {
                            end += 1;
                        }
                        if chunk.get(end) == Some(&b'\n') {
                            end += 1;
                        }
                        return hit(pos + end as u64, "rtf");
                    }
                }
                0 => return None,
                _ => {}
            }
        }
        pos += chunk.len() as u64;
        if pos > MAX {
            return None;
        }
    }
}

/// How far the data keeps looking like text.
fn text_extent(w: &mut Win, max: u64) -> u64 {
    let mut pos = 0u64;
    loop {
        let chunk = w.some(pos, 1 << 16);
        if chunk.is_empty() {
            return pos;
        }
        if let Some(i) = chunk.iter().position(|b| !is_text(*b)) {
            return pos + i as u64;
        }
        pos += chunk.len() as u64;
        if pos >= max {
            return max;
        }
    }
}

/// Offset just past the last occurrence of `needle` (ASCII, case-insensitive) in `[0, end)`.
fn find_last_ci(w: &mut Win, end: u64, needle: &[u8]) -> Option<u64> {
    let mut best = None;
    let mut pos = 0u64;
    let overlap = needle.len() as u64 - 1;
    while pos < end {
        let want = ((end - pos) as usize).min(1 << 16);
        let chunk = w.some(pos, want);
        if chunk.len() < needle.len() {
            break;
        }
        for (i, win) in chunk.windows(needle.len()).enumerate() {
            if win.eq_ignore_ascii_case(needle) {
                best = Some(pos + (i + needle.len()) as u64);
            }
        }
        if chunk.len() < want || pos + chunk.len() as u64 >= end {
            break;
        }
        pos += chunk.len() as u64 - overlap;
    }
    best
}

/// XML (including SVG and friends) and HTML.
pub fn markup(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 256 << 20;
    let head = w.some(0, 4096).to_vec();
    let lower = head.to_ascii_lowercase();
    let is_xml = lower.starts_with(b"<?xml ") || lower.starts_with(b"<svg ");
    let is_html = lower.starts_with(b"<!doctype html") || lower.starts_with(b"<html");
    if !is_xml && !is_html {
        return None;
    }
    // The name of the root element.
    let mut root: Option<String> = None;
    let mut i = 0usize;
    while i + 1 < head.len() {
        if head[i] != b'<' {
            i += 1;
            continue;
        }
        match head[i + 1] {
            b'?' => i += find(&head[i..], b"?>")? + 2,
            b'!' if head[i..].starts_with(b"<!--") => i += find(&head[i..], b"-->")? + 3,
            b'!' => i += find(&head[i..], b">")? + 1,
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let name: String = head[i + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphanumeric() || b"_:.-".contains(c))
                    .map(|c| *c as char)
                    .collect();
                root = Some(name);
                break;
            }
            _ => return None,
        }
    }
    let root = root?;
    if root.len() > 64 {
        return None;
    }
    let extent = text_extent(w, MAX);
    let close = format!("</{root}>");
    let mut end = find_last_ci(w, extent, close.as_bytes())?;
    // Keep the final line break if there is one.
    let tail = w.some(end, 2);
    if tail.starts_with(b"\r\n") {
        end += 2;
    } else if tail.starts_with(b"\n") {
        end += 1;
    }
    let ext = match root.to_ascii_lowercase().as_str() {
        "html" => "html",
        "svg" => "svg",
        "plist" => "plist",
        "kml" => "kml",
        "gpx" => "gpx",
        _ if is_html => "html",
        _ => "xml",
    };
    hit(end, ext)
}

/// Plain text: a run of text that starts on a sector boundary.
pub fn text(w: &mut Win) -> Option<Hit> {
    let head = w.get(0, 512)?;
    if !head.iter().all(|b| is_text(*b)) {
        return None;
    }
    let spaces = head
        .iter()
        .filter(|b| matches!(**b, b' ' | b'\n' | b'\t'))
        .count();
    let high = head.iter().filter(|b| **b >= 0x80).count();
    // Valid UTF-8, apart from a character cut in two by the end of the sector.
    let utf8 = match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(e) => e.valid_up_to() >= 508,
    };
    if spaces < 26 || high > 150 || !utf8 {
        return None;
    }
    let size = text_extent(w, 256 << 20);
    Some(Hit {
        size,
        ext: "txt",
        partial: false,
        estimated: true,
    })
}

pub fn sqlite(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 100)?;
    let page = match be16(h, 16)? {
        1 => 65_536u64,
        p if p.is_power_of_two() && p >= 512 => p as u64,
        _ => return None,
    };
    let pages = be32(h, 28)? as u64;
    if h[21] != 64 || h[22] != 32 || h[23] != 32 || pages == 0 || be32(h, 24)? != be32(h, 92)? {
        return None;
    }
    hit(page * pages, "sqlite")
}

pub fn pst(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 0xC0)?;
    let ext = match &h[8..10] {
        b"SM" => "pst",
        b"SO" => "ost",
        _ => return None,
    };
    let size = match le16(h, 10)? {
        14 | 15 => le32(h, 0xA8)? as u64,
        v if v >= 23 => le64(h, 0xB8)?,
        _ => return None,
    };
    if size < 1024 {
        return None;
    }
    hit(size, ext)
}

// ------------------------------------------------------------------------------------
// Archives
// ------------------------------------------------------------------------------------

/// Finds the data descriptor that ends a ZIP entry of unknown length.
fn zip_descriptor(w: &mut Win, data_start: u64) -> Option<u64> {
    const MAX: u64 = 4 << 30;
    let mut pos = data_start;
    loop {
        let chunk = w.some(pos, 1 << 20).to_vec();
        if chunk.len() < 16 {
            return None;
        }
        let mut from = 0usize;
        while let Some(i) = find(&chunk[from..], b"PK\x07\x08") {
            let at = from + i;
            if at + 24 > chunk.len() && chunk.len() == 1 << 20 {
                break;
            }
            let dist = pos + at as u64 - data_start;
            if le32(&chunk, at + 8).map(u64::from) == Some(dist) {
                return Some(pos + at as u64 + 16);
            }
            if le64(&chunk, at + 8) == Some(dist) {
                return Some(pos + at as u64 + 24);
            }
            from = at + 4;
        }
        if chunk.len() < 1 << 20 {
            return None;
        }
        pos += (from.max(chunk.len() - 32)) as u64;
        if pos - data_start > MAX {
            return None;
        }
    }
}

/// ZIP and everything built on it (DOCX, XLSX, PPTX, ODF, EPUB, JAR, APK).
pub fn zip(w: &mut Win) -> Option<Hit> {
    let mut pos = 0u64;
    let mut entries = 0u32;
    let mut ext = "zip";
    let mut mime: Vec<u8> = Vec::new();
    let mut jar = false;
    let failed = |pos: u64, entries: u32, ext: &'static str| {
        // Without its directory the archive is damaged, but repair tools can
        // still read the complete entries.
        if entries >= 1 && pos >= 512 {
            partial(pos, ext)
        } else {
            None
        }
    };
    loop {
        let Some(sig) = w.le32(pos) else {
            return failed(pos.min(w.limit()), entries, ext);
        };
        match sig {
            0x0403_4B50 => {
                let Some(h) = w.get(pos, 30).map(|b| b.to_vec()) else {
                    return failed(pos, entries, ext);
                };
                let flags = le16(&h, 6)?;
                let method = le16(&h, 8)?;
                let mut csize = le32(&h, 18)? as u64;
                let usize32 = le32(&h, 22)?;
                let nlen = le16(&h, 26)? as usize;
                let xlen = le16(&h, 28)? as usize;
                if nlen == 0 || nlen > 2048 || method > 99 {
                    return failed(pos, entries, ext);
                }
                let Some(tail) = w.get(pos + 30, nlen + xlen).map(|b| b.to_vec()) else {
                    return failed(pos, entries, ext);
                };
                let name = &tail[..nlen];
                if name.contains(&0) {
                    return failed(pos, entries, ext);
                }
                if csize == 0xFFFF_FFFF {
                    // ZIP64: the real sizes are in the extra field.
                    let mut x = &tail[nlen..];
                    while x.len() >= 4 {
                        let (id, len) = (le16(x, 0)?, le16(x, 2)? as usize);
                        if id == 1 {
                            let skip = if usize32 == 0xFFFF_FFFF { 8 } else { 0 };
                            csize = le64(x, 4 + skip)?;
                            break;
                        }
                        x = x.get(4 + len..)?;
                    }
                }
                let data = pos + 30 + (nlen + xlen) as u64;
                if name.starts_with(b"word/") {
                    ext = "docx";
                } else if name.starts_with(b"xl/") {
                    ext = "xlsx";
                } else if name.starts_with(b"ppt/") {
                    ext = "pptx";
                } else if name.starts_with(b"visio/") {
                    ext = "vsdx";
                } else if name == b"AndroidManifest.xml" {
                    ext = "apk";
                } else if name == b"META-INF/MANIFEST.MF" {
                    jar = true;
                } else if entries == 0 && name == b"mimetype" && method == 0 && csize <= 80 {
                    mime = w.get(data, csize as usize)?.to_vec();
                }
                let next = if flags & 8 != 0 && csize == 0 {
                    match zip_descriptor(w, data) {
                        Some(n) => n,
                        None => return failed(pos, entries, ext),
                    }
                } else {
                    let mut next = data.checked_add(csize)?;
                    if flags & 8 != 0 {
                        // A descriptor follows even though the sizes were known.
                        next += match w.le32(next) {
                            Some(0x0807_4B50) => 16,
                            _ => 12,
                        };
                    }
                    next
                };
                if next > w.limit() {
                    return failed(pos, entries, ext);
                }
                entries += 1;
                pos = next;
            }
            0x0201_4B50 => {
                let Some(h) = w.get(pos, 46) else {
                    return failed(pos, entries, ext);
                };
                pos += 46 + le16(h, 28)? as u64 + le16(h, 30)? as u64 + le16(h, 32)? as u64;
            }
            0x0606_4B50 => pos = pos.checked_add(12)?.checked_add(w.le64(pos + 4)?)?,
            0x0706_4B50 => pos += 20,
            0x0505_4B50 => pos += 6 + w.le16(pos + 4)? as u64,
            0x0806_4B50 => pos += 8 + w.le32(pos + 4)? as u64,
            0x0605_4B50 => {
                let comment = w.le16(pos + 20)? as u64;
                if entries == 0 {
                    return None;
                }
                if ext == "zip" {
                    ext = if mime.starts_with(b"application/vnd.oasis.opendocument.text") {
                        "odt"
                    } else if mime.starts_with(b"application/vnd.oasis.opendocument.spreadsheet") {
                        "ods"
                    } else if mime.starts_with(b"application/vnd.oasis.opendocument.presentation") {
                        "odp"
                    } else if mime.starts_with(b"application/epub+zip") {
                        "epub"
                    } else if jar {
                        "jar"
                    } else {
                        "zip"
                    };
                }
                return hit((pos + 22 + comment).min(w.limit()), ext);
            }
            _ => return failed(pos, entries, ext),
        }
        if pos > 64 << 30 {
            return None;
        }
    }
}

/// RAR 5 variable-length integer.
fn rar_vint(w: &mut Win, pos: u64) -> Option<(u64, u64)> {
    let mut v = 0u64;
    for i in 0..10u64 {
        let b = w.u8(pos + i)?;
        v |= ((b & 0x7F) as u64) << (7 * i);
        if b & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

pub fn rar(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 64 << 30;
    let h = w.get(0, 8)?;
    if h[6] == 0 {
        // RAR 1.5 - 4.x: a chain of blocks, each with its own size.
        let mut pos = 7u64;
        let mut files = 0u32;
        while let Some(b) = w.get(pos, 7) {
            let (ty, flags, size) = (b[2], le16(b, 3)?, le16(b, 5)? as u64);
            if !(0x72..=0x7B).contains(&ty) || size < 7 {
                break;
            }
            let mut add = 0u64;
            if flags & 0x8000 != 0 || ty == 0x74 || ty == 0x7A {
                add = w.le32(pos + 7)? as u64;
                if (ty == 0x74 || ty == 0x7A) && flags & 0x100 != 0 {
                    add |= (w.le32(pos + 32)? as u64) << 32;
                }
            }
            let next = pos.saturating_add(size).saturating_add(add);
            if next > w.limit().min(MAX) {
                break;
            }
            pos = next;
            if ty == 0x74 {
                files += 1;
            }
            if ty == 0x7B {
                return hit(pos, "rar");
            }
        }
        // Old archives have no end block.
        return if files > 0 { hit(pos, "rar") } else { None };
    }
    if h[6] != 1 || h[7] != 0 {
        return None;
    }
    // RAR 5.
    let mut pos = 8u64;
    let mut blocks = 0u32;
    loop {
        let (hsize, l1) = rar_vint(w, pos + 4)?;
        if hsize == 0 || hsize > 2 << 20 {
            return None;
        }
        let body = pos + 4 + l1;
        let (ty, l2) = rar_vint(w, body)?;
        let (flags, l3) = rar_vint(w, body + l2)?;
        let mut p = body + l2 + l3;
        if flags & 1 != 0 {
            p += rar_vint(w, p)?.1;
        }
        let data = if flags & 2 != 0 { rar_vint(w, p)?.0 } else { 0 };
        if !(1..=5).contains(&ty) {
            return None;
        }
        pos = body.checked_add(hsize)?.checked_add(data)?;
        if pos > w.limit().min(MAX) {
            return None;
        }
        blocks += 1;
        if ty == 5 {
            return hit(pos, "rar");
        }
        if blocks > 10_000_000 {
            return None;
        }
    }
}

pub fn sevenz(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 32)?;
    if crc32(&h[12..32]) != le32(h, 8)? {
        return None;
    }
    let total = 32u64.checked_add(le64(h, 12)?)?.checked_add(le64(h, 20)?)?;
    hit(total, "7z")
}

pub fn gzip(w: &mut Win) -> Option<Hit> {
    use miniz_oxide::inflate::core::{decompress, inflate_flags, DecompressorOxide};
    use miniz_oxide::inflate::TINFLStatus;
    const MAX: u64 = 8 << 30;
    let h = w.get(0, 10)?;
    let flags = h[3];
    if flags & 0xE0 != 0 {
        return None;
    }
    let mut pos = 10u64;
    if flags & 4 != 0 {
        pos += 2 + w.le16(pos)? as u64;
    }
    for bit in [8u8, 16] {
        if flags & bit != 0 {
            let s = w.some(pos, 1024);
            pos += s.iter().position(|b| *b == 0)? as u64 + 1;
        }
    }
    if flags & 2 != 0 {
        pos += 2;
    }
    // Inflate the stream (the output is thrown away) to learn where it ends.
    let mut state = Box::new(DecompressorOxide::new());
    let mut out = vec![0u8; 32_768];
    let mut out_pos = 0usize;
    let mut consumed = 0u64;
    let mut produced = 0u64;
    let mut tar = false;
    loop {
        let chunk = w.some(pos + consumed, 1 << 16);
        if chunk.is_empty() {
            return None;
        }
        let (status, used, wrote) = decompress(
            &mut state,
            chunk,
            &mut out,
            out_pos,
            inflate_flags::TINFL_FLAG_HAS_MORE_INPUT,
        );
        if produced == 0 && wrote >= 512 {
            tar = &out[257..262] == b"ustar";
        }
        consumed += used as u64;
        produced += wrote as u64;
        out_pos = (out_pos + wrote) & (out.len() - 1);
        match status {
            TINFLStatus::Done => break,
            TINFLStatus::NeedsMoreInput | TINFLStatus::HasMoreOutput => {
                if used == 0 && wrote == 0 {
                    return None;
                }
            }
            _ => return None,
        }
        if consumed > MAX {
            return None;
        }
    }
    if produced == 0 {
        return None;
    }
    let end = pos + consumed + 8;
    if end > w.limit() {
        return None;
    }
    hit(end, if tar { "tgz" } else { "gz" })
}

pub fn xz(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 8 << 30;
    let h = w.get(0, 12)?;
    let stream_flags = [h[6], h[7]];
    if crc32(&h[6..8]) != le32(h, 8)? {
        return None;
    }
    // The stream ends with a 12-byte footer: CRC, index size, the same flags, "YZ".
    let mut pos = 12u64;
    loop {
        let chunk = w.some(pos, 1 << 20);
        let n = chunk.len();
        if n < 12 {
            return None;
        }
        for i in 10..n - 1 {
            if chunk[i] == b'Y'
                && chunk[i + 1] == b'Z'
                && (pos + i as u64 + 2).is_multiple_of(4)
                && chunk[i - 2..i] == stream_flags
                && crc32(&chunk[i - 6..i]) == le32(chunk, i - 10)?
            {
                return hit(pos + i as u64 + 2, "xz");
            }
        }
        if n < 1 << 20 {
            return None;
        }
        pos += n as u64 - 12;
        if pos > MAX {
            return None;
        }
    }
}

pub fn cab(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 36)?;
    let size = le32(h, 8)? as u64;
    if le32(h, 4)? != 0 || le32(h, 12)? != 0 || h[24] != 3 || h[25] != 1 || size < 36 {
        return None;
    }
    hit(size, "cab")
}

pub fn tar(w: &mut Win) -> Option<Hit> {
    let mut pos = 0u64;
    let mut entries = 0u32;
    let octal = |b: &[u8]| -> Option<u64> {
        let s: String = b
            .iter()
            .skip_while(|c| **c == b' ')
            .take_while(|c| **c != 0 && **c != b' ')
            .map(|c| *c as char)
            .collect();
        u64::from_str_radix(&s, 8).ok()
    };
    while let Some(h) = w.get(pos, 512) {
        if h.iter().all(|b| *b == 0) {
            // Two blank blocks close the archive.
            pos = (pos + 1024).min(w.limit());
            break;
        }
        let stored = octal(&h[148..156]);
        let sum: u64 = h
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    *b as u64
                }
            })
            .sum();
        let Some(size) = octal(&h[124..136]) else {
            break;
        };
        if stored != Some(sum) || pos + 512 + size > w.limit() {
            break;
        }
        pos += 512 + size.div_ceil(512) * 512;
        entries += 1;
    }
    if entries == 0 {
        return None;
    }
    hit(pos.min(w.limit()), "tar")
}

// ------------------------------------------------------------------------------------
// Programs
// ------------------------------------------------------------------------------------

pub fn pe(w: &mut Win) -> Option<Hit> {
    let lfanew = w.le32(0x3C)? as u64;
    if !(0x40..=0x1000).contains(&lfanew) {
        return None;
    }
    let h = w.get(lfanew, 24)?.to_vec();
    if &h[..4] != b"PE\0\0" {
        return None;
    }
    let machine = le16(&h, 4)?;
    let sections = le16(&h, 6)? as u64;
    let opt_size = le16(&h, 20)? as u64;
    let characteristics = le16(&h, 22)?;
    if ![0x014C, 0x8664, 0xAA64, 0x01C0, 0x01C4, 0x0200].contains(&machine)
        || !(1..=96).contains(&sections)
        || opt_size < 96
    {
        return None;
    }
    let opt = w.get(lfanew + 24, opt_size as usize)?.to_vec();
    let plus = match le16(&opt, 0)? {
        0x10B => false,
        0x20B => true,
        _ => return None,
    };
    let mut end = le32(&opt, 60)? as u64;
    let subsystem = le16(&opt, 68)?;
    // The certificate table is the one directory addressed by file offset.
    let dirs = if plus { 112 } else { 96 };
    if let (Some(off), Some(len)) = (le32(&opt, dirs + 32), le32(&opt, dirs + 36)) {
        if off != 0 && len < 64 << 20 {
            end = end.max(off as u64 + len as u64);
        }
    }
    let table = w.get(lfanew + 24 + opt_size, (sections * 40) as usize)?;
    for s in table.chunks_exact(40) {
        let (size, ptr) = (le32(s, 16)? as u64, le32(s, 20)? as u64);
        if ptr != 0 {
            end = end.max(ptr + size);
        }
    }
    if end < 512 {
        return None;
    }
    let ext = if characteristics & 0x2000 != 0 {
        "dll"
    } else if subsystem == 1 {
        "sys"
    } else {
        "exe"
    };
    hit(end, ext)
}

/// Linux programs and libraries (little-endian ELF): the file reaches as far
/// as its last segment, section or header table.
pub fn elf(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 4 << 30;
    let h = w.get(0, 64)?.to_vec();
    let wide = match h[4] {
        1 => false,
        2 => true,
        _ => return None,
    };
    if h[5] != 1
        || h[6] != 1
        || !(1..=4).contains(&le16(&h, 16)?)
        || le16(&h, 18)? == 0
        || le32(&h, 20)? != 1
    {
        return None;
    }
    let (phoff, shoff, at) = if wide {
        (le64(&h, 32)?, le64(&h, 40)?, 52)
    } else {
        (le32(&h, 28)? as u64, le32(&h, 32)? as u64, 40)
    };
    let ehsize = le16(&h, at)? as u64;
    let phentsize = le16(&h, at + 2)? as usize;
    let phnum = le16(&h, at + 4)? as usize;
    let shentsize = le16(&h, at + 6)? as usize;
    let shnum = le16(&h, at + 8)? as usize;
    let (eh, ph, sh) = if wide { (64, 56, 64) } else { (52, 32, 40) };
    if ehsize != eh
        || (phnum != 0 && phentsize != ph)
        || (shnum != 0 && shentsize != sh)
        || (phnum == 0 && shnum == 0)
    {
        return None;
    }
    let mut end = ehsize;
    if phnum > 0 {
        if phoff < eh {
            return None;
        }
        let table = w.get(phoff, phnum * ph)?.to_vec();
        end = end.max(phoff + (phnum * ph) as u64);
        for p in table.chunks_exact(ph) {
            let (off, size) = if wide {
                (le64(p, 8)?, le64(p, 32)?)
            } else {
                (le32(p, 4)? as u64, le32(p, 16)? as u64)
            };
            if size > 0 {
                end = end.max(off.checked_add(size)?);
            }
        }
    }
    if shnum > 0 {
        if shoff < eh {
            return None;
        }
        let table = w.get(shoff, shnum * sh)?.to_vec();
        end = end.max(shoff + (shnum * sh) as u64);
        for s in table.chunks_exact(sh) {
            let (off, size) = if wide {
                (le64(s, 24)?, le64(s, 32)?)
            } else {
                (le32(s, 16)? as u64, le32(s, 20)? as u64)
            };
            // Unused entries and sections without bytes in the file (.bss) do not count.
            if !matches!(le32(s, 4)?, 0 | 8) && size > 0 {
                end = end.max(off.checked_add(size)?);
            }
        }
    }
    if end > MAX {
        return None;
    }
    hit(end, "elf")
}

// ------------------------------------------------------------------------------------
// Fonts
// ------------------------------------------------------------------------------------

/// TrueType and OpenType: the table directory says where every table lies.
pub fn sfnt(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 12)?;
    let ext = match &h[..4] {
        [0, 1, 0, 0] => "ttf",
        b"OTTO" => "otf",
        _ => return None,
    };
    let tables = be16(h, 4)? as usize;
    if !(4..=64).contains(&tables) {
        return None;
    }
    let first = 12 + tables as u64 * 16;
    let dir = w.get(12, tables * 16)?;
    let mut end = first;
    let mut head = false;
    let mut known = false;
    for t in dir.chunks_exact(16) {
        let tag = &t[..4];
        let (off, len) = (be32(t, 8)? as u64, be32(t, 12)? as u64);
        if !tag.iter().all(|c| (0x20..0x7F).contains(c))
            || off < first
            || !off.is_multiple_of(4)
            || len > 1 << 30
        {
            return None;
        }
        match tag {
            // The font header table has one fixed length.
            b"head" if len == 54 => head = true,
            b"head" => return None,
            b"cmap" | b"glyf" | b"name" | b"maxp" | b"CFF " | b"CFF2" => known = true,
            _ => {}
        }
        end = end.max(off + len);
    }
    if !head || !known {
        return None;
    }
    // Tables are padded with zeros to four bytes; a few fonts leave that out
    // after their last table.
    let padded = end.div_ceil(4) * 4;
    let is_padded = w
        .get(end, (padded - end) as usize)
        .is_some_and(|p| p.iter().all(|b| *b == 0));
    hit(if is_padded { padded } else { end }, ext)
}

/// Web fonts (WOFF and WOFF2) store their total length in the header.
pub fn woff(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 48)?;
    let two = match &h[..4] {
        b"wOFF" => false,
        b"wOF2" => true,
        _ => return None,
    };
    if !matches!(&h[4..8], [0, 1, 0, 0] | b"OTTO" | b"true" | b"ttcf") {
        return None;
    }
    let len = be32(h, 8)? as u64;
    let tables = be16(h, 12)? as u64;
    let directory = if two { 48 + tables } else { 44 + tables * 20 };
    if tables == 0 || tables > 4096 || be16(h, 14)? != 0 || be32(h, 16)? < 12 || len <= directory {
        return None;
    }
    if two {
        let compressed = be32(h, 20)? as u64;
        if compressed == 0 || compressed >= len {
            return None;
        }
        hit(len, "woff2")
    } else {
        // The optional metadata and private blocks lie inside the file.
        for at in [24, 36] {
            let (off, size) = (be32(h, at)? as u64, be32(h, at + 4)? as u64);
            if (off == 0) != (size == 0) || off + size > len {
                return None;
            }
        }
        hit(len, "woff")
    }
}

// ------------------------------------------------------------------------------------
// More pictures: JPEG 2000, DjVu, metafiles, icons
// ------------------------------------------------------------------------------------

/// Offset just past the first occurrence of `needle` in `[from, end)`.
/// ASCII letters match in either case; other bytes must be equal.
fn find_first_ci(w: &mut Win, from: u64, end: u64, needle: &[u8]) -> Option<u64> {
    let mut pos = from;
    let overlap = needle.len() as u64 - 1;
    while pos < end {
        let want = ((end - pos) as usize).min(1 << 16);
        let chunk = w.some(pos, want);
        if chunk.len() < needle.len() {
            break;
        }
        if let Some(i) = chunk
            .windows(needle.len())
            .position(|x| x.eq_ignore_ascii_case(needle))
        {
            return Some(pos + (i + needle.len()) as u64);
        }
        if chunk.len() < want || pos + chunk.len() as u64 >= end {
            break;
        }
        pos += chunk.len() as u64 - overlap;
    }
    None
}

/// Walks a JPEG 2000 codestream that starts at `start` and returns the
/// offset just past its end-of-codestream marker.
fn j2k_end(w: &mut Win, start: u64, max: u64) -> Option<u64> {
    let siz = w.get(start, 42)?;
    let components = be16(siz, 40)? as u32;
    if siz[..4] != [0xFF, 0x4F, 0xFF, 0x51]
        || !(1..=16_384).contains(&components)
        || be16(siz, 4)? as u32 != 38 + 3 * components
        || be32(siz, 8)? == 0
        || be32(siz, 12)? == 0
    {
        return None;
    }
    let cap = w.limit().min(start.saturating_add(max));
    // The main header: marker segments up to the first tile part.
    let mut pos = start + 2;
    for _ in 0..4096 {
        let m = w.get(pos, 4)?;
        if m[0] != 0xFF {
            return None;
        }
        match m[1] {
            0x90 => break,
            0x50..=0x7F => {
                let len = be16(m, 2)? as u64;
                if len < 2 {
                    return None;
                }
                pos += 2 + len;
            }
            _ => return None,
        }
    }
    // Tile parts, each with its own length, then the end marker.
    loop {
        let t = w.get(pos, 12)?;
        if t[..4] != [0xFF, 0x90, 0x00, 0x0A] {
            return None;
        }
        let len = be32(t, 6)? as u64;
        if len == 0 {
            // The last tile part may leave its length open: it runs up to the end marker.
            return find_first_ci(w, pos + 12, cap, &[0xFF, 0xD9]);
        }
        if len < 14 {
            return None;
        }
        pos += len;
        if pos > cap {
            return None;
        }
        let next = w.get(pos, 2)?;
        if next == [0xFF, 0xD9] {
            return Some(pos + 2);
        }
    }
}

pub const JP2_SIGNATURE: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0C, 0x6A, 0x50, 0x20, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
];

/// JPEG 2000 in its box container: the picture ends with the codestream box.
pub fn jp2(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 4 << 30;
    let f = w.get(12, 12)?;
    if &f[4..8] != b"ftyp" || !matches!(&f[8..12], b"jp2 " | b"jpx ") {
        return None;
    }
    let mut pos = 12u64;
    for _ in 0..4096 {
        let h = w.get(pos, 8)?;
        let mut size = be32(h, 0)? as u64;
        let ty = [h[4], h[5], h[6], h[7]];
        if !ty.iter().all(|c| (0x20..0x7F).contains(c)) {
            return None;
        }
        let mut header = 8u64;
        if size == 1 {
            size = w.be64(pos + 8)?;
            header = 16;
        }
        if &ty == b"jp2c" {
            let data = pos + header;
            if size == 0 {
                // "Up to the end of the file": the codestream itself says where that is.
                return hit(j2k_end(w, data, MAX)?, "jp2");
            }
            if size < header + 4 || size > MAX || w.get(data, 2)? != [0xFF, 0x4F] {
                return None;
            }
            return hit(pos + size, "jp2");
        }
        if size < header {
            return None;
        }
        pos = pos.checked_add(size)?;
        if pos > MAX {
            return None;
        }
    }
    None
}

/// A bare JPEG 2000 codestream without the box container.
pub fn j2k(w: &mut Win) -> Option<Hit> {
    hit(j2k_end(w, 0, 4 << 30)?, "j2k")
}

pub fn djvu(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 16)?;
    let len = be32(h, 8)? as u64;
    if &h[..8] != b"AT&TFORM" || !matches!(&h[12..16], b"DJVU" | b"DJVM" | b"DJVI") || len < 16 {
        return None;
    }
    hit(len + 12, "djvu")
}

/// Enhanced metafile: the header record holds the size of the whole file.
pub fn emf(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 88)?;
    let header = le32(h, 4)? as u64;
    let bytes = le32(h, 48)? as u64;
    let records = le32(h, 52)? as u64;
    if le32(h, 0)? != 1
        || &h[40..44] != b" EMF"
        || le32(h, 44)? != 0x1_0000
        || !(88..=4096).contains(&header)
        || !header.is_multiple_of(4)
        || le16(h, 58)? != 0
        || !bytes.is_multiple_of(4)
        || bytes < header + 20
        || records < 2
        || records > (bytes - header) / 8 + 1
    {
        return None;
    }
    // The last record is the end-of-file record, which repeats its own size at its end.
    let last = w.le32(bytes - 4)? as u64;
    if last < 20 || !last.is_multiple_of(4) || last > bytes - header {
        return None;
    }
    let eof = w.get(bytes - last, 8)?;
    if le32(eof, 0)? != 14 || le32(eof, 4)? as u64 != last {
        return None;
    }
    hit(bytes, "emf")
}

/// Icons and cursors: a directory of pictures, each a PNG or a bitmap.
pub fn ico(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 6)?;
    let cursor = match le16(h, 2)? {
        1 => false,
        2 => true,
        _ => return None,
    };
    let count = le16(h, 4)? as usize;
    if le16(h, 0)? != 0 || !(1..=64).contains(&count) {
        return None;
    }
    let dir = w.get(6, count * 16)?.to_vec();
    let first = 6 + count as u64 * 16;
    let mut end = first;
    for e in dir.chunks_exact(16) {
        let (a, b) = (le16(e, 4)?, le16(e, 6)?);
        let (size, off) = (le32(e, 8)? as u64, le32(e, 12)? as u64);
        // Icons: colour planes and bits per pixel. Cursors: the hot spot.
        let fields_ok = if cursor {
            a < 256 && b < 256
        } else {
            a <= 1 && [0, 1, 4, 8, 16, 24, 32].contains(&b)
        };
        if e[3] != 0 || !fields_ok || off < first || !(40..=16 << 20).contains(&size) {
            return None;
        }
        let p = w.get(off, 16)?;
        let png = p.starts_with(b"\x89PNG\r\n\x1a\n");
        let bitmap = le32(p, 0)? == 40 && (1..=1024).contains(&le32(p, 4)?) && le16(p, 12)? == 1;
        if !png && !bitmap {
            return None;
        }
        end = end.max(off + size);
    }
    hit(end, if cursor { "cur" } else { "ico" })
}

// ------------------------------------------------------------------------------------
// More audio and video
// ------------------------------------------------------------------------------------

/// Length of an APE tag that starts (with its header) at `pos`; 0 if there is none.
fn ape_tag(w: &mut Win, pos: u64) -> u64 {
    let limit = w.limit();
    match w.get(pos, 32) {
        Some(t) if &t[..8] == b"APETAGEX" => {
            let size = le32(t, 12).unwrap_or(0) as u64;
            let flags = le32(t, 20).unwrap_or(0);
            // Bit 29 marks this block as the header; the size counts the items and the footer.
            if flags & 0x2000_0000 != 0
                && (32..16 << 20).contains(&size)
                && pos + 32 + size <= limit
            {
                32 + size
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// DSD audio (DSF): the first chunk holds the size of the whole file.
pub fn dsf(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 40)?;
    let total = le64(h, 12)?;
    let metadata = le64(h, 20)?;
    if &h[..4] != b"DSD "
        || le64(h, 4)? != 28
        || &h[28..32] != b"fmt "
        || le64(h, 32)? != 52
        || !(92..=64 << 30).contains(&total)
        || (metadata != 0 && !(92..total).contains(&metadata))
    {
        return None;
    }
    hit(total, "dsf")
}

/// WavPack: a chain of blocks, each with its own length.
pub fn wavpack(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 64 << 30;
    let mut pos = 0u64;
    let mut blocks = 0u64;
    let mut last_index = 0u64;
    while let Some(h) = w.get(pos, 32) {
        let size = le32(h, 4)? as u64;
        if &h[..4] != b"wvpk"
            || !(0x402..=0x410).contains(&le16(h, 8)?)
            || !(24..=4 << 20).contains(&size)
        {
            break;
        }
        // A block that counts its samples from an earlier point belongs to the next file.
        let index = (h[10] as u64) << 32 | le32(h, 16)? as u64;
        if (blocks > 0 && index < last_index) || pos + 8 + size > w.limit() {
            break;
        }
        last_index = index;
        pos += 8 + size;
        blocks += 1;
        if pos > MAX {
            return None;
        }
    }
    if blocks == 0 {
        return None;
    }
    pos += ape_tag(w, pos);
    hit(pos, "wv")
}

/// AMR speech (narrow and wide band): frames whose length follows from
/// their first byte. The file ends at the first byte that cannot start a frame.
pub fn amr(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 1 << 30;
    // Frame lengths (with the header byte) by frame type; 0 = not a valid type.
    const NARROW: [u8; 16] = [13, 14, 16, 18, 20, 21, 27, 32, 6, 0, 0, 0, 0, 0, 0, 1];
    const WIDE: [u8; 16] = [18, 24, 33, 37, 41, 47, 51, 59, 61, 6, 0, 0, 0, 0, 1, 1];
    let h = w.get(0, 9)?;
    let (mut pos, sizes) = if h.starts_with(b"#!AMR\n") {
        (6u64, &NARROW)
    } else if h.starts_with(b"#!AMR-WB\n") {
        (9u64, &WIDE)
    } else {
        return None;
    };
    let mut frames = 0u32;
    while let Some(b) = w.u8(pos) {
        let len = sizes[(b >> 3 & 15) as usize] as u64;
        if b & 0x83 != 0 || len == 0 || pos + len > w.limit() {
            break;
        }
        // Blank space after the file would otherwise pass as a row of frames.
        if b == 0 && w.get(pos, len as usize)?.iter().all(|x| *x == 0) {
            break;
        }
        pos += len;
        frames += 1;
        if pos > MAX {
            return None;
        }
    }
    if frames < 10 {
        return None;
    }
    hit(pos, "amr")
}

/// Sun / NeXT audio: header size and data size are both in the header.
pub fn au(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 24)?;
    let header = be32(h, 4)? as u64;
    let data = be32(h, 8)? as u64;
    if &h[..4] != b".snd"
        || !(24..=1 << 20).contains(&header)
        || data == 0
        // "Unknown length", as written by programs that stream their output.
        || data == 0xFFFF_FFFF
        || !(1..=27).contains(&be32(h, 12)?)
        || !(100..=768_000).contains(&be32(h, 16)?)
        || !(1..=256).contains(&be32(h, 20)?)
    {
        return None;
    }
    hit(header + data, "au")
}

/// Monkey's Audio from version 3.98 on: the descriptor lists the length of every part.
pub fn ape(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 52)?;
    let version = le16(h, 4)?;
    let descriptor = le32(h, 8)? as u64;
    let header = le32(h, 12)? as u64;
    let seek_table = le32(h, 16)? as u64;
    let wav_header = le32(h, 20)? as u64;
    let frames = (le32(h, 28)? as u64) << 32 | le32(h, 24)? as u64;
    let terminating = le32(h, 32)? as u64;
    if &h[..4] != b"MAC "
        || !(3980..=4999).contains(&version)
        || !(52..=4096).contains(&descriptor)
        || !(24..=4096).contains(&header)
        || !seek_table.is_multiple_of(4)
        || frames == 0
        || frames > 64 << 30
    {
        return None;
    }
    let mut end = descriptor + header + seek_table + wav_header + frames + terminating;
    if end > w.limit() {
        return None;
    }
    end += ape_tag(w, end);
    hit(end, "ape")
}

/// MPEG program stream (MPG, VOB): packs and packets, each introduced by a
/// start code. Ends with the program end code, or where the chain breaks.
pub fn mpeg_ps(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 64 << 30;
    let mut pos = 0u64;
    let mut packs = 0u64;
    let mut packets = 0u64;
    while let Some(h) = w.get(pos, 4) {
        if h[..3] != [0, 0, 1] {
            break;
        }
        let next = match h[3] {
            0xB9 => {
                pos += 4;
                break;
            }
            0xBA => {
                let Some(p) = w.get(pos, 14) else {
                    break;
                };
                // The fixed marker bits tell the two generations of pack header apart.
                let mpeg2 = p[4] & 0xC4 == 0x44
                    && p[6] & 4 != 0
                    && p[8] & 4 != 0
                    && p[9] & 1 != 0
                    && p[12] & 3 == 3;
                let mpeg1 = p[4] & 0xF1 == 0x21
                    && p[6] & 1 != 0
                    && p[8] & 1 != 0
                    && p[9] & 0x80 != 0
                    && p[11] & 1 != 0;
                let stuffing = (p[13] & 7) as u64;
                if !mpeg2 && !mpeg1 {
                    break;
                }
                packs += 1;
                if mpeg2 {
                    pos + 14 + stuffing
                } else {
                    pos + 12
                }
            }
            0xBB..=0xFF => {
                let Some(len) = w.be16(pos + 4) else {
                    break;
                };
                packets += 1;
                pos + 6 + len as u64
            }
            _ => break,
        };
        if next > w.limit() {
            break;
        }
        pos = next;
        if pos > MAX {
            return None;
        }
    }
    // A few start codes in a row happen by chance; a recording has many.
    if packs < 4 || packets < 4 {
        return None;
    }
    hit(pos, "mpg")
}

/// Flash (uncompressed): the header holds the length of the file.
pub fn swf(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 30)?;
    let len = le32(h, 4)? as u64;
    if &h[..3] != b"FWS" || !(1..=45).contains(&h[3]) {
        return None;
    }
    // The stage rectangle: a 5-bit field width, then four values of that many bits.
    let width = (h[8] >> 3) as usize;
    let field = |n: usize| -> u32 {
        (5 + n * width..5 + (n + 1) * width).fold(0u32, |v, bit| {
            v << 1 | (h[8 + bit / 8] >> (7 - bit % 8) & 1) as u32
        })
    };
    if width < 2 || field(0) != 0 || field(2) != 0 {
        return None;
    }
    let (x, y) = (field(1), field(3));
    if x == 0 || y == 0 || x >> (width - 1) != 0 || y >> (width - 1) != 0 {
        return None;
    }
    let after = 8 + (5 + 4 * width).div_ceil(8);
    let rate = le16(h, after)?;
    if rate == 0 || rate > 120 << 8 || le16(h, after + 2)? == 0 || len < after as u64 + 6 {
        return None;
    }
    // The last tag of every movie is the two-byte end tag.
    if w.get(len - 2, 2)? != [0, 0] {
        return None;
    }
    hit(len, "swf")
}

// ------------------------------------------------------------------------------------
// More archives: Zstandard, LZ4, ar
// ------------------------------------------------------------------------------------

/// Does a compressed Zstandard block of `size` bytes begin with a literals
/// section that fits inside it? `b` holds the first bytes of the block.
fn zstd_literals_fit(b: &[u8], size: u64) -> bool {
    let Some(&first) = b.first() else {
        return false;
    };
    let byte = |i: usize| b.get(i).copied().unwrap_or(0) as u64;
    let format = first >> 2 & 3;
    let (header, unpacked, stored) = if first & 3 < 2 {
        // Raw or run-length literals: only the unpacked size is given.
        let (header, unpacked) = match format {
            0 | 2 => (1, byte(0) >> 3),
            1 => (2, byte(0) >> 4 | byte(1) << 4),
            _ => (3, byte(0) >> 4 | byte(1) << 4 | byte(2) << 12),
        };
        (header, unpacked, if first & 3 == 0 { unpacked } else { 1 })
    } else {
        // Packed literals: unpacked and stored size, with the same number of bits each.
        let (header, bits) = match format {
            0 | 1 => (3, 10),
            2 => (4, 14),
            _ => (5, 18),
        };
        let v = (0..5).fold(0u64, |v, i| v | byte(i) << (8 * i));
        let mask = (1u64 << bits) - 1;
        (header, v >> 4 & mask, v >> (4 + bits) & mask)
    };
    // At least one byte of sequences follows the literals.
    unpacked <= 128 << 10 && header + stored < size
}

/// The end of one Zstandard frame that starts at `start`.
fn zstd_frame(w: &mut Win, start: u64) -> Option<u64> {
    let d = w.u8(start + 4)?;
    if d & 0x08 != 0 {
        return None;
    }
    let single = d & 0x20 != 0;
    let dict = [0u64, 1, 2, 4][(d & 3) as usize];
    let content_size = match d >> 6 {
        0 => single as u64,
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let mut pos = start + 5 + !single as u64 + dict + content_size;
    loop {
        let b = w.get(pos, 3)?;
        let v = b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32) << 16;
        let size = (v >> 3) as u64;
        let body = match v >> 1 & 3 {
            0 => size,
            // One byte, repeated `size` times when unpacked.
            1 => 1,
            2 if size > 0 => size,
            _ => return None,
        };
        if size > 128 << 10 || (v >> 1 & 3 == 2 && !zstd_literals_fit(w.some(pos + 3, 5), size)) {
            return None;
        }
        pos += 3 + body;
        if pos > w.limit() {
            return None;
        }
        if v & 1 != 0 {
            break;
        }
    }
    if d & 0x04 != 0 {
        pos += 4;
    }
    (pos <= w.limit()).then_some(pos)
}

/// Walks frames that follow one another; `frame` returns the end of the
/// frame at a position. Skippable frames may sit between and after them.
fn framed(
    w: &mut Win,
    magic: u32,
    frame: fn(&mut Win, u64) -> Option<u64>,
    ext: &'static str,
) -> Option<Hit> {
    const MAX: u64 = 64 << 30;
    let mut pos = 0u64;
    let mut frames = 0u32;
    while let Some(m) = w.le32(pos) {
        if m & 0xFFFF_FFF0 == 0x184D_2A50 && frames > 0 {
            let Some(len) = w.le32(pos + 4) else {
                break;
            };
            let next = pos + 8 + len as u64;
            if next > w.limit() {
                break;
            }
            pos = next;
            continue;
        }
        if m != magic {
            break;
        }
        match frame(w, pos) {
            Some(end) => pos = end,
            None if frames == 0 => return None,
            None => break,
        }
        frames += 1;
        if pos > MAX {
            return None;
        }
    }
    if frames == 0 {
        return None;
    }
    hit(pos, ext)
}

pub fn zstd(w: &mut Win) -> Option<Hit> {
    framed(w, 0xFD2F_B528, zstd_frame, "zst")
}

/// The xxHash32 of a short input (under 16 bytes), as used for the LZ4 header check.
pub(crate) fn xxh32_short(data: &[u8]) -> u32 {
    const P1: u32 = 0x9E37_79B1;
    const P2: u32 = 0x85EB_CA77;
    const P3: u32 = 0xC2B2_AE3D;
    const P4: u32 = 0x27D4_EB2F;
    const P5: u32 = 0x1656_67B1;
    debug_assert!(data.len() < 16);
    let mut h = P5.wrapping_add(data.len() as u32);
    let mut words = data.chunks_exact(4);
    for c in &mut words {
        let v = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        h = h
            .wrapping_add(v.wrapping_mul(P3))
            .rotate_left(17)
            .wrapping_mul(P4);
    }
    for b in words.remainder() {
        h = h
            .wrapping_add((*b as u32).wrapping_mul(P5))
            .rotate_left(11)
            .wrapping_mul(P1);
    }
    h ^= h >> 15;
    h = h.wrapping_mul(P2);
    h ^= h >> 13;
    h = h.wrapping_mul(P3);
    h ^ h >> 16
}

/// The end of one LZ4 frame that starts at `start`.
fn lz4_frame(w: &mut Win, start: u64) -> Option<u64> {
    let h = w.some(start + 4, 15).to_vec();
    let (flags, block) = (*h.first()?, *h.get(1)?);
    if flags & 0xC0 != 0x40 || flags & 0x02 != 0 || block & 0x8F != 0 || block >> 4 < 4 {
        return None;
    }
    // Optional content size and dictionary id, then one byte that checks the header.
    let content_size = if flags & 0x08 != 0 { 8 } else { 0 };
    let descriptor = 2 + content_size + (flags & 1) as usize * 4;
    if (xxh32_short(h.get(..descriptor)?) >> 8) as u8 != *h.get(descriptor)? {
        return None;
    }
    let largest = 1u64 << (8 + 2 * (block >> 4));
    let block_check = if flags & 0x10 != 0 { 4 } else { 0 };
    let mut pos = start + 4 + descriptor as u64 + 1;
    loop {
        let v = w.le32(pos)?;
        pos += 4;
        if v == 0 {
            break;
        }
        // The top bit marks a block stored without compression.
        let size = (v & 0x7FFF_FFFF) as u64;
        if size > largest {
            return None;
        }
        pos += size + block_check;
        if pos > w.limit() {
            return None;
        }
    }
    if flags & 0x04 != 0 {
        pos += 4;
    }
    (pos <= w.limit()).then_some(pos)
}

pub fn lz4(w: &mut Win) -> Option<Hit> {
    framed(w, 0x184D_2204, lz4_frame, "lz4")
}

/// `ar` archives: Debian packages and static libraries.
pub fn ar(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 16 << 30;
    let mut pos = 8u64;
    let mut members = 0u32;
    let mut ext = "a";
    while let Some(h) = w.get(pos, 60) {
        if &h[58..60] != b"`\n" || !h[..58].iter().all(|c| (0x20..0x7F).contains(c)) {
            break;
        }
        let digits = h[48..58].trim_ascii_end();
        if digits.is_empty() || !digits.iter().all(|c| c.is_ascii_digit()) {
            break;
        }
        let size = digits.iter().fold(0u64, |v, c| v * 10 + (c - b'0') as u64);
        let debian = h.starts_with(b"debian-binary");
        if pos + 60 + size > w.limit() {
            break;
        }
        if members == 0 && debian {
            ext = "deb";
        }
        // Members start on even offsets.
        pos += 60 + size + (size & 1);
        members += 1;
        if pos > MAX {
            return None;
        }
    }
    if members == 0 {
        return None;
    }
    hit(pos.min(w.limit()), ext)
}

// ------------------------------------------------------------------------------------
// Windows system files, contacts, calendars, captures
// ------------------------------------------------------------------------------------

/// Windows registry hive: a 4096-byte header, then the hive bins.
pub fn regf(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 512)?;
    let mut sum = 0u32;
    for i in 0..127 {
        sum ^= le32(h, i * 4)?;
    }
    // Two values are never stored as a checksum.
    let sum = match sum {
        0 => 1,
        0xFFFF_FFFF => 0xFFFF_FFFE,
        s => s,
    };
    let bins = le32(h, 40)? as u64;
    if sum != le32(h, 508)?
        || le32(h, 20)? != 1
        || le32(h, 24)? > 6
        || bins == 0
        || !bins.is_multiple_of(4096)
    {
        return None;
    }
    if w.get(4096, 4)? != b"hbin" {
        return None;
    }
    hit(4096 + bins, "hve")
}

/// Windows event log: a 4096-byte header, then chunks of 64 KB.
pub fn evtx(w: &mut Win) -> Option<Hit> {
    const CHUNK: u64 = 65_536;
    let h = w.get(0, 128)?;
    let count = le16(h, 42)? as u64;
    if &h[..8] != b"ElfFile\0"
        || le32(h, 32)? != 128
        || le16(h, 38)? != 3
        || le16(h, 40)? != 4096
        || count == 0
        || crc32(&h[..120]) != le32(h, 124)?
    {
        return None;
    }
    let mut good = 0u64;
    while good < count
        && 4096 + (good + 1) * CHUNK <= w.limit()
        && w.get(4096 + good * CHUNK, 8) == Some(b"ElfChnk\0")
    {
        good += 1;
    }
    match good {
        0 => None,
        g if g == count => hit(4096 + g * CHUNK, "evtx"),
        // Fewer chunks than the header counts: the rest was overwritten.
        g => partial(4096 + g * CHUNK, "evtx"),
    }
}

/// Text made of objects between a BEGIN and an END line (vCard, iCalendar).
/// Several objects may follow one another; the file ends after the last one.
fn text_objects(w: &mut Win, begin: &[u8], end: &[u8], ext: &'static str) -> Option<Hit> {
    const MAX: u64 = 64 << 20;
    let extent = text_extent(w, MAX);
    let mut pos = 0u64;
    let mut done = 0u64;
    while pos < extent {
        match w.get(pos, begin.len()) {
            Some(b) if b.eq_ignore_ascii_case(begin) => {}
            _ => break,
        }
        let Some(mut e) = find_first_ci(w, pos + begin.len() as u64, extent, end) else {
            break;
        };
        // Keep the line break that closes the END line.
        let tail = w.some(e, 2);
        if tail.starts_with(b"\r\n") {
            e += 2;
        } else if tail.starts_with(b"\n") {
            e += 1;
        }
        done = e;
        // Blank lines may separate one object from the next.
        pos = e;
        while pos < extent && matches!(w.u8(pos), Some(b'\r' | b'\n')) {
            pos += 1;
        }
    }
    if done == 0 {
        return None;
    }
    hit(done, ext)
}

pub fn vcard(w: &mut Win) -> Option<Hit> {
    text_objects(w, b"BEGIN:VCARD", b"\nEND:VCARD", "vcf")
}

pub fn icalendar(w: &mut Win) -> Option<Hit> {
    text_objects(w, b"BEGIN:VCALENDAR", b"\nEND:VCALENDAR", "ics")
}

/// Network capture (pcap): a header, then packet records with their lengths.
pub fn pcap(w: &mut Win) -> Option<Hit> {
    const MAX: u64 = 64 << 30;
    const LARGEST: u32 = 262_144;
    let h = w.get(0, 24)?;
    // Timestamps count micro- or nanoseconds, depending on the signature.
    let second = match le32(h, 0)? {
        0xA1B2_C3D4 => 1_000_000,
        0xA1B2_3C4D => 1_000_000_000,
        _ => return None,
    };
    let link = le32(h, 20)?;
    if le16(h, 4)? != 2 || le16(h, 6)? != 4 || link & 0x03FF_0000 != 0 || link & 0xFFFF > 1024 {
        return None;
    }
    let snap = match le32(h, 16)? {
        0 => LARGEST,
        s => s.min(LARGEST),
    };
    let mut pos = 24u64;
    let mut records = 0u64;
    while let Some(r) = w.get(pos, 16) {
        let (fraction, stored, original) = (le32(r, 4)?, le32(r, 8)?, le32(r, 12)?);
        if fraction >= second
            || stored == 0
            || stored > snap
            || original < stored
            || pos + 16 + stored as u64 > w.limit()
        {
            break;
        }
        pos += 16 + stored as u64;
        records += 1;
        if pos > MAX {
            return None;
        }
    }
    if records == 0 {
        return None;
    }
    hit(pos, "pcap")
}

/// The class id that every Windows shortcut carries after its header size.
pub const LNK_HEADER: [u8; 20] = [
    0x4C, 0x00, 0x00, 0x00, 0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x46,
];

/// Windows shortcut: a fixed header, then optional parts announced by flag bits.
pub fn lnk(w: &mut Win) -> Option<Hit> {
    let h = w.get(0, 76)?;
    let flags = le32(h, 20)?;
    if h[..20] != LNK_HEADER || h[66..76].iter().any(|b| *b != 0) {
        return None;
    }
    let mut pos = 76u64;
    if flags & 0x01 != 0 {
        // The target as a list of shell item ids.
        pos += 2 + w.le16(pos)? as u64;
    }
    if flags & 0x02 != 0 {
        // Where the target was: volume and path.
        let len = w.le32(pos)? as u64;
        if !(28..=1 << 20).contains(&len) {
            return None;
        }
        pos += len;
    }
    // Name, relative path, working folder, arguments, icon: counted strings.
    let wide = if flags & 0x80 != 0 { 2 } else { 1 };
    for bit in [0x04, 0x08, 0x10, 0x20, 0x40] {
        if flags & bit != 0 {
            pos += 2 + w.le16(pos)? as u64 * wide;
        }
    }
    // Extra blocks, closed by a size below 4.
    for _ in 0..64 {
        let len = w.le32(pos)? as u64;
        if len < 4 {
            return hit(pos + 4, "lnk");
        }
        if len > 1 << 20 || w.le32(pos + 4)? & 0xFFFF_FFF0 != 0xA000_0000 {
            return None;
        }
        pos += len;
    }
    None
}
