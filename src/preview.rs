//! Previews of found files: decoded pictures, text, or a hex dump.

use crate::model::FoundFile;
use crate::recover::read_file_range;
use crate::source::Source;

/// Pictures larger than this are not decoded for a preview.
pub const MAX_PICTURE_BYTES: u64 = 48 << 20;

/// Can the built-in decoders show this extension?
pub fn is_picture(ext: &str) -> bool {
    matches!(
        ext,
        "jpg" | "jpeg" | "jpe" | "jfif" | "png" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico"
    )
}

/// The first `max` bytes of a file.
pub fn head(src: &Source, f: &FoundFile, max: usize) -> Vec<u8> {
    let mut buf = vec![0u8; (f.size.min(max as u64)) as usize];
    let n = read_file_range(src, f, 0, &mut buf);
    buf.truncate(n);
    buf
}

pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Size of the original picture.
    pub full: (u32, u32),
}

/// Decodes a picture and scales it down to fit `max_side`. Returns `None`
/// for anything the decoders reject; a decoder bug cannot take the program down.
pub fn decode_picture(bytes: &[u8], max_side: u32) -> Option<Decoded> {
    let bytes = bytes.to_vec();
    std::panic::catch_unwind(move || {
        let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .ok()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(20_000);
        limits.max_image_height = Some(20_000);
        limits.max_alloc = Some(512 << 20);
        reader.limits(limits);
        let img = reader.decode().ok()?;
        let full = (img.width(), img.height());
        let small = if full.0 > max_side || full.1 > max_side {
            img.thumbnail(max_side, max_side)
        } else {
            img
        };
        let rgba = small.to_rgba8();
        Some(Decoded {
            width: rgba.width(),
            height: rgba.height(),
            rgba: rgba.into_raw(),
            full,
        })
    })
    .ok()
    .flatten()
}

/// The content as text, when it looks like text (UTF-8, or UTF-16 with a byte-order mark).
pub fn as_text(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let s = crate::util::utf16le(&bytes[2..]);
        return (!s.contains('\0')).then_some(s);
    }
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let text = match std::str::from_utf8(body) {
        Ok(s) => s,
        // The preview may have cut a character in half at the end.
        Err(e) if e.valid_up_to() + 4 >= body.len() => {
            std::str::from_utf8(&body[..e.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    let odd = text
        .chars()
        .filter(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{c}'))
        .count();
    (odd == 0 && !text.is_empty()).then(|| text.to_string())
}

/// The classic hex view: offset, sixteen bytes, their characters.
pub fn hex_dump(bytes: &[u8], max_rows: usize) -> String {
    let mut out = String::with_capacity(max_rows.min(bytes.len() / 16 + 1) * 78);
    for (row, chunk) in bytes.chunks(16).take(max_rows).enumerate() {
        out.push_str(&format!("{:08X}  ", row * 16));
        for i in 0..16 {
            match chunk.get(i) {
                Some(b) => out.push_str(&format!("{b:02X} ")),
                None => out.push_str("   "),
            }
            if i == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        for b in chunk {
            out.push(if (0x20..0x7F).contains(b) {
                *b as char
            } else {
                '.'
            });
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimg::samples;
    use crate::util::Rng;

    #[test]
    fn decodes_and_scales_pictures() {
        for bytes in [
            samples::jpeg(1, 400, 300),
            samples::png(2, 400, 300),
            samples::gif(3, 80, 60),
            samples::bmp(4, 400, 300),
            samples::webp(5, 400, 300),
            samples::tiff(6, 400, 300),
        ] {
            let d = decode_picture(&bytes, 128).expect("decodes");
            assert!(d.width <= 128 && d.height <= 128 && d.width.max(d.height) >= 60);
            assert_eq!(d.rgba.len(), (d.width * d.height * 4) as usize);
        }
        let d = decode_picture(&samples::png(2, 40, 30), 128).unwrap();
        assert_eq!((d.width, d.height, d.full), (40, 30, (40, 30)));
    }

    #[test]
    fn damaged_pictures_are_refused_quietly() {
        assert!(decode_picture(&[], 100).is_none());
        assert!(decode_picture(b"not a picture at all", 100).is_none());
        let mut rng = Rng(42);
        for (i, base) in [
            samples::jpeg(1, 120, 90),
            samples::png(2, 120, 90),
            samples::gif(3, 60, 40),
            samples::bmp(4, 60, 40),
            samples::webp(5, 60, 40),
            samples::tiff(6, 60, 40),
        ]
        .iter()
        .enumerate()
        {
            for round in 0..60 {
                let mut b = base.clone();
                if round % 3 == 0 {
                    b.truncate(rng.below(b.len() as u64) as usize);
                } else {
                    for _ in 0..(1 + rng.below(12)) {
                        let at = rng.below(b.len() as u64) as usize;
                        b[at] = rng.next_u64() as u8;
                    }
                }
                // Any answer is fine; what matters is that it returns.
                let _ = decode_picture(&b, 64);
                let _ = i;
            }
        }
    }

    #[test]
    fn text_and_hex() {
        assert_eq!(as_text(b"plain\r\ntext").as_deref(), Some("plain\r\ntext"));
        assert_eq!(as_text(b"\xEF\xBB\xBFbom").as_deref(), Some("bom"));
        assert_eq!(
            as_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]).as_deref(),
            Some("hi")
        );
        assert_eq!(
            as_text("caf\u{e9}".as_bytes()).as_deref(),
            Some("caf\u{e9}")
        );
        // A multi-byte character cut by the end of the preview is dropped.
        assert_eq!(
            as_text(&"caf\u{e9}".as_bytes()[..4]).as_deref(),
            Some("caf")
        );
        assert_eq!(as_text(b"bin\0ary"), None);
        assert_eq!(as_text(&[0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86]), None);
        assert_eq!(as_text(b""), None);
        let dump = hex_dump(b"RescueForge hex view!", 10);
        let mut lines = dump.lines();
        assert_eq!(
            lines.next().unwrap(),
            "00000000  52 65 73 63 75 65 46 6F  72 67 65 20 68 65 78 20  RescueForge hex "
        );
        assert!(lines
            .next()
            .unwrap()
            .starts_with("00000010  76 69 65 77 21 "));
        assert_eq!(hex_dump(&[0u8; 1000], 3).lines().count(), 3);
        assert_eq!(hex_dump(&[], 3), "");
    }
}
