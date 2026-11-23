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

/// Is a thumbnail worth asking for? Pictures, and the files that carry a
/// preview picture inside (camera RAW photos, Photoshop files).
pub fn has_thumbnail(ext: &str, size: u64) -> bool {
    (is_picture(ext) && size <= MAX_PICTURE_BYTES)
        || is_raw_photo(ext)
        || matches!(ext, "psd" | "psb")
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

/// Text saved in the system's older one-byte encoding (for example Arabic
/// or Western European "ANSI" text files). Only accepted when every byte
/// could be text, so binary data still gets the hex view.
pub fn as_legacy_text(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 8 || std::str::from_utf8(bytes).is_ok() {
        return None;
    }
    let texty = bytes
        .iter()
        .all(|b| matches!(b, 0x09 | 0x0A | 0x0C | 0x0D | 0x20..=0x7E | 0x80..=0xFF));
    // Mostly high bytes is far more likely compressed or encrypted data.
    let high = bytes.iter().filter(|b| **b >= 0x80).count();
    let spaces = bytes.iter().filter(|b| **b == b' ' || **b == b'\n').count();
    if !texty || (high * 2 > bytes.len() && spaces * 12 < bytes.len()) {
        return None;
    }
    let s = crate::sys::ansi_to_string(bytes);
    (!s.is_empty()).then_some(s)
}

/// Files that are not pictures themselves but usually carry one inside: the
/// preview that cameras store in RAW photos, the thumbnail of a Photoshop
/// file, the cover of a song.
pub fn has_embedded_picture(ext: &str) -> bool {
    is_raw_photo(ext) || matches!(ext, "psd" | "psb" | "mp3" | "flac" | "m4a" | "wma")
}

pub fn is_raw_photo(ext: &str) -> bool {
    matches!(
        ext,
        "cr2" | "cr3" | "nef" | "arw" | "dng" | "orf" | "rw2" | "pef" | "srw" | "raf" | "raw"
    )
}

/// How much of a file is read to look for the picture inside it.
pub fn embedded_reach(ext: &str, thumbnail: bool) -> usize {
    match (is_raw_photo(ext), thumbnail) {
        (true, false) => 32 << 20,
        (true, true) => 8 << 20,
        (false, _) => 4 << 20,
    }
}

/// Finds the best picture stored inside other data: every JPEG or PNG start
/// is tried and the largest one that really decodes wins.
pub fn embedded_picture(bytes: &[u8], max_side: u32) -> Option<Decoded> {
    let mut best: Option<Decoded> = None;
    let mut tried = 0;
    let mut at = 1usize;
    while tried < 10 {
        let Some(start) = picture_start(bytes, at) else {
            break;
        };
        tried += 1;
        if let Some(d) = decode_picture(&bytes[start..], max_side) {
            let area = |d: &Decoded| d.full.0 as u64 * d.full.1 as u64;
            if best.as_ref().map(|b| area(&d) > area(b)).unwrap_or(true) {
                best = Some(d);
            }
            // Large enough to look at: no need to decode the others.
            if best
                .as_ref()
                .map(|b| b.full.0.max(b.full.1) >= max_side.min(1024))
                .unwrap_or(false)
            {
                break;
            }
        }
        at = start + 3;
    }
    best
}

/// Where the next JPEG or PNG seems to start at or after `from`.
fn picture_start(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 12 <= bytes.len() {
        match bytes[i] {
            // A real JPEG continues with a table, an APPn or a comment segment.
            0xFF if bytes[i + 1] == 0xD8
                && bytes[i + 2] == 0xFF
                && matches!(bytes[i + 3], 0xDB | 0xC4 | 0xE0..=0xEF | 0xFE) =>
            {
                return Some(i)
            }
            0x89 if bytes[i..].starts_with(b"\x89PNG\r\n\x1a\n") => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// ZIP-based files: archives, and the documents that are ZIP files inside.
pub fn is_zip_based(ext: &str) -> bool {
    matches!(
        ext,
        "zip" | "jar" | "apk" | "docx" | "xlsx" | "pptx" | "vsdx" | "odt" | "ods" | "odp" | "epub"
    )
}

/// How much of a ZIP-based file is read for its preview.
pub const ZIP_REACH: usize = 6 << 20;
/// The most text taken out of one document part.
const MAX_PART: usize = 2 << 20;

struct ZipEntry<'a> {
    name: String,
    size: u64,
    method: u16,
    /// The stored bytes (possibly cut short by the end of what was read).
    data: &'a [u8],
}

/// Walks the entries at the start of a ZIP file. Stops quietly at the first
/// thing that does not look right.
fn zip_entries(bytes: &[u8]) -> Vec<ZipEntry<'_>> {
    use crate::util::{le16, le32};
    let mut out = Vec::new();
    let mut pos = 0usize;
    while out.len() < 2000 && bytes.get(pos..pos + 4) == Some(b"PK\x03\x04") {
        let (Some(flags), Some(method), Some(packed), Some(size), Some(nlen), Some(xlen)) = (
            le16(bytes, pos + 6),
            le16(bytes, pos + 8),
            le32(bytes, pos + 18),
            le32(bytes, pos + 22),
            le16(bytes, pos + 26),
            le16(bytes, pos + 28),
        ) else {
            break;
        };
        let name_at = pos + 30;
        let data_at = name_at + nlen as usize + xlen as usize;
        let Some(name) = bytes.get(name_at..name_at + nlen as usize) else {
            break;
        };
        let name = String::from_utf8_lossy(name).into_owned();
        let streamed = flags & 8 != 0 && packed == 0;
        let end = if streamed {
            bytes.len()
        } else {
            (data_at + packed as usize).min(bytes.len())
        };
        out.push(ZipEntry {
            name,
            size: size as u64,
            method,
            data: bytes.get(data_at..end).unwrap_or(&[]),
        });
        if streamed {
            // The sizes follow the data: find the next entry by its signature.
            let Some(next) = bytes
                .get(data_at..)
                .and_then(|rest| rest.windows(4).position(|w| w == b"PK\x03\x04"))
            else {
                break;
            };
            if let Some(last) = out.last_mut() {
                last.data = &bytes[data_at..data_at + next];
            }
            pos = data_at + next;
        } else {
            pos = data_at + packed as usize;
        }
    }
    out
}

fn unpack(e: &ZipEntry) -> Option<Vec<u8>> {
    match e.method {
        0 => Some(e.data[..e.data.len().min(MAX_PART)].to_vec()),
        8 => {
            let whole = miniz_oxide::inflate::decompress_to_vec_with_limit(e.data, MAX_PART);
            match whole {
                Ok(v) => Some(v),
                // Cut short, damaged or longer than the limit: keep what came out.
                Err(err) => (!err.output.is_empty()).then_some(err.output),
            }
        }
        _ => None,
    }
}

/// Turns the XML of a document part into plain text: paragraphs become
/// lines, tags are dropped, entities are decoded.
fn xml_to_text(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len() / 4);
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        push_decoded(&mut out, &rest[..open]);
        let Some(close) = rest[open..].find('>') else {
            rest = "";
            break;
        };
        let tag = &rest[open + 1..open + close];
        let name = tag
            .trim_start_matches('/')
            .split([' ', '/'])
            .next()
            .unwrap_or("");
        let closing = tag.starts_with('/');
        match name {
            "w:p" | "a:p" | "text:p" | "text:h" | "si" | "row" | "table:table-row" if closing => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            "w:br" | "a:br" | "text:line-break" => out.push('\n'),
            "w:tab" | "text:tab" | "table:table-cell" | "c"
                if !closing && !out.is_empty() && !out.ends_with(['\n', '\t']) =>
            {
                out.push('\t');
            }
            _ => {}
        }
        rest = &rest[open + close + 1..];
    }
    push_decoded(&mut out, rest);
    out.trim().to_string()
}

fn push_decoded(out: &mut String, text: &str) {
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';').filter(|i| *i <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=semi]),
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
}

/// What a ZIP-based file holds, as text for the preview: the words of a
/// Word, PowerPoint, Excel or OpenDocument file when they can be read,
/// otherwise the list of files in the archive. The second value says which.
pub fn zip_preview(bytes: &[u8]) -> Option<(String, &'static str)> {
    let entries = zip_entries(bytes);
    if entries.is_empty() {
        return None;
    }
    let mut parts: Vec<&ZipEntry> = entries
        .iter()
        .filter(|e| {
            let n = e.name.as_str();
            n == "word/document.xml"
                || n == "content.xml"
                || n == "xl/sharedStrings.xml"
                || (n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
        })
        .collect();
    // Slides in their order: slide2 before slide10.
    parts.sort_by_key(|e| (e.name.len(), e.name.clone()));
    let mut text = String::new();
    for e in parts {
        if let Some(raw) = unpack(e) {
            let t = xml_to_text(&String::from_utf8_lossy(&raw));
            if !t.is_empty() {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(&t);
            }
        }
        if text.len() > 200_000 {
            break;
        }
    }
    if !text.is_empty() {
        return Some((text, "The text of this document"));
    }
    let mut list = String::new();
    for e in entries.iter().filter(|e| !e.name.ends_with('/')) {
        if e.size > 0 {
            list.push_str(&format!("{}\t{}\n", e.name, crate::util::fmt_size(e.size)));
        } else {
            list.push_str(&format!("{}\n", e.name));
        }
    }
    (!list.is_empty()).then_some((
        list,
        "The files inside this archive (as far as they could be read)",
    ))
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
    fn pictures_inside_other_files_are_found() {
        let mut rng = Rng(7);
        // The way a camera RAW file or a song carries a picture: some
        // structure, a small thumbnail, the real preview, then the data.
        let small = samples::jpeg(1, 160, 120);
        let large = samples::jpeg(2, 640, 480);
        let mut raw = b"II*\0".to_vec();
        raw.extend(rng.bytes(3000));
        // A stray marker that is not a picture must not stop the search.
        raw.extend([0xFF, 0xD8, 0xFF, 0x00]);
        raw.extend(rng.bytes(500));
        raw.extend(&small);
        raw.extend(rng.bytes(2000));
        raw.extend(&large);
        raw.extend(rng.bytes(50_000));
        let d = embedded_picture(&raw, 1400).expect("found");
        assert_eq!(d.full, (640, 480), "the largest picture wins");
        let t = embedded_picture(&raw, 180).expect("thumbnail");
        assert!(t.width <= 180 && t.height <= 180);
        // A PNG cover works as well.
        let mut song = b"ID3\x03\0\0\0\0\x1f\x76APIC".to_vec();
        song.extend(samples::png(3, 300, 300));
        song.extend(rng.bytes(9000));
        assert_eq!(embedded_picture(&song, 1400).unwrap().full, (300, 300));
        // Nothing inside, or only the start of a picture: no preview, no panic.
        assert!(embedded_picture(&rng.bytes(100_000), 1400).is_none());
        let mut cut = rng.bytes(100);
        cut.extend(&large[..200]);
        assert!(embedded_picture(&cut, 1400).is_none());
        assert!(embedded_picture(&[], 1400).is_none());
        // A picture at the very start is the file itself, not something inside it.
        assert!(embedded_picture(&small, 1400).is_none());

        assert!(has_embedded_picture("nef") && has_embedded_picture("mp3"));
        assert!(!has_embedded_picture("jpg") && !has_embedded_picture("pdf"));
        assert!(has_thumbnail("cr2", 60 << 20) && has_thumbnail("psd", 1));
        assert!(has_thumbnail("png", 1000) && !has_thumbnail("png", 1 << 30));
        assert!(!has_thumbnail("mp3", 1000) && !has_thumbnail("docx", 1000));
        assert!(embedded_reach("arw", false) > embedded_reach("arw", true));
    }

    fn word_xml() -> Vec<u8> {
        "<?xml version=\"1.0\"?><w:document><w:body>\
         <w:p><w:r><w:t>Annual report</w:t></w:r></w:p>\
         <w:p><w:r><w:t>Sales &amp; costs &lt;2025&gt;</w:t><w:tab/><w:t>caf&#233; &#x6A9;</w:t></w:r></w:p>\
         <w:p><w:r><w:t>ڕاپۆرتی ساڵانە</w:t><w:br/><w:t>second line</w:t></w:r></w:p>\
         </w:body></w:document>"
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn documents_show_their_text_and_archives_their_contents() {
        let expected = "Annual report\nSales & costs <2025>\tcafé ک\nڕاپۆرتی ساڵانە\nsecond line";
        for streamed in [false, true] {
            let docx = samples::zip(
                &[
                    ("[Content_Types].xml", samples::xml(1)),
                    ("word/document.xml", word_xml()),
                    ("docProps/core.xml", samples::xml(2)),
                ],
                streamed,
            );
            let (text, what) = zip_preview(&docx).expect("text");
            assert_eq!(text, expected, "streamed: {streamed}");
            assert_eq!(what, "The text of this document");
            // Only the first part of the file could be read: still some text.
            let part = &docx[..docx.len() - 200];
            assert!(zip_preview(part).unwrap().0.starts_with("Annual report"));
        }
        // The sample documents of the demo and the self-test read as text too.
        let (text, _) = zip_preview(&samples::docx(7)).expect("sample document");
        assert!(text.lines().count() >= 6 && !text.contains('<'), "{text}");
        // Slides come in their order, spreadsheets show their strings.
        let slide =
            |t: &str| format!("<p:sld><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:sld>").into_bytes();
        let pptx = samples::zip(
            &[
                ("ppt/slides/slide10.xml", slide("ten")),
                ("ppt/slides/slide2.xml", slide("two")),
                ("ppt/slides/slide1.xml", slide("one")),
            ],
            true,
        );
        assert_eq!(zip_preview(&pptx).unwrap().0, "one\n\ntwo\n\nten");
        let xlsx = samples::zip(
            &[(
                "xl/sharedStrings.xml",
                b"<sst><si><t>Rent</t></si><si><t>Food</t></si></sst>".to_vec(),
            )],
            false,
        );
        assert_eq!(zip_preview(&xlsx).unwrap().0, "Rent\nFood");

        // An ordinary archive lists what is in it.
        let zip = samples::zip(
            &[
                ("photos/", Vec::new()),
                ("photos/a.jpg", vec![7u8; 2048]),
                ("notes.txt", b"hello".to_vec()),
            ],
            false,
        );
        let (list, what) = zip_preview(&zip).unwrap();
        assert_eq!(list, "photos/a.jpg\t2.00 KB\nnotes.txt\t5 bytes\n");
        assert!(what.starts_with("The files inside this archive"));
        assert!(is_zip_based("docx") && is_zip_based("apk") && !is_zip_based("rar"));

        // Not a ZIP, or a damaged one: nothing, and never a panic.
        assert!(zip_preview(b"").is_none());
        assert!(zip_preview(b"PK\x03\x04").is_none());
        assert!(zip_preview(&samples::jpeg(1, 60, 40)).is_none());
        let base = samples::zip(&[("word/document.xml", word_xml())], true);
        let mut rng = Rng(11);
        for round in 0..300 {
            let mut b = base.clone();
            if round % 3 == 0 {
                b.truncate(rng.below(b.len() as u64) as usize);
            } else {
                for _ in 0..(1 + rng.below(6)) {
                    let at = rng.below(b.len() as u64) as usize;
                    b[at] = rng.next_u64() as u8;
                }
            }
            let _ = zip_preview(&b);
        }
        assert_eq!(
            xml_to_text("a &unknown; b & c &#xZZ; <b>d</b"),
            "a &unknown; b & c &#xZZ; d"
        );
    }

    #[test]
    fn text_in_the_old_windows_encoding_is_still_text() {
        // "Señor Müller" followed by plain words, as Notepad used to save it.
        let mut ansi = b"Se\xF1or M\xFCller wrote this letter in 1998.\r\n".to_vec();
        ansi.extend_from_slice(b"It has several plain lines of text.\r\n");
        assert_eq!(as_text(&ansi), None, "not valid UTF-8");
        let text = as_legacy_text(&ansi).expect("text");
        assert!(text.starts_with("Se") && text.ends_with("plain lines of text.\r\n"));
        assert_eq!(text.chars().count(), ansi.len(), "one letter per byte");
        // UTF-8 goes the normal way, binary data stays binary.
        assert_eq!(as_legacy_text(b"plain ASCII text"), None);
        assert_eq!(as_legacy_text(b"bin\0ary data \xF1"), None);
        assert_eq!(
            as_legacy_text(&[0xF1, 0x02, 0x80, 0x90, 0x41, 0x42, 0x43, 0x44]),
            None
        );
        assert_eq!(as_legacy_text(&Rng(5).bytes(4096)), None);
        let dense: Vec<u8> = (0..4096u32).map(|i| 0x80 + (i * 7 % 120) as u8).collect();
        assert_eq!(as_legacy_text(&dense), None, "looks like packed data");
        assert_eq!(as_legacy_text(b"\xF1"), None);
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
