//! Tests for the format readers: every supported type is embedded in random
//! data and must be found with exactly its own length; damaged and random
//! input must never panic.

use super::*;
use crate::testimg::samples as s;
use crate::util::Rng;

const LEAD: usize = 8192;

/// Random bytes, the sample at a sector boundary, then more random bytes
/// (or zeros when `blank_after`).
fn embed(sample: &[u8], blank_after: bool) -> Source {
    let mut rng = Rng(0xC0FFEE ^ sample.len() as u64);
    let mut img = rng.bytes(LEAD);
    img.extend_from_slice(sample);
    let pad = img.len().div_ceil(512) * 512 - img.len();
    let tail = if blank_after {
        vec![0u8; pad + 65_536]
    } else {
        rng.bytes(pad + 65_536)
    };
    img.extend_from_slice(&tail);
    Source::from_bytes(img)
}

fn carve_with(carver: &Carver, src: &Source) -> Option<Hit> {
    let head = src.read_vec(LEAD as u64, 512);
    carver.identify(src, LEAD as u64, &head, false)
}

fn carve(sample: &[u8]) -> Option<Hit> {
    carve_with(&Carver::new(&[]), &embed(sample, false))
}

fn all_samples() -> Vec<(&'static str, Vec<u8>, &'static str)> {
    let small_text = s::txt(1, 30);
    let mut all = vec![
        ("jpeg", s::jpeg(1, 320, 240), "jpg"),
        ("png", s::png(2, 200, 150), "png"),
        ("gif", s::gif(3, 80, 60), "gif"),
        ("bmp", s::bmp(4, 100, 70), "bmp"),
        ("tiff", s::tiff(5, 120, 90), "tif"),
        ("webp", s::webp(6, 120, 90), "webp"),
        (
            "nef",
            s::tiff_with_make(7, "NIKON CORPORATION", false, 5000),
            "nef",
        ),
        ("arw", s::tiff_with_make(8, "SONY", false, 5000), "arw"),
        (
            "dng",
            s::tiff_with_make(9, "Invented Camera Co", true, 5000),
            "dng",
        ),
        ("psd", s::psd(10, 60, 40), "psd"),
        ("raf", s::raf(11), "raf"),
        ("heic", s::heic(12, 9000), "heic"),
        ("mp4", s::mp4(13, 30_000), "mp4"),
        ("mov", s::mov(14, 30_000), "mov"),
        ("m4a", s::m4a(15, 12_000), "m4a"),
        ("3gp", s::three_gp(16, 12_000), "3gp"),
        ("avi", s::avi(17, 20_000), "avi"),
        ("mkv", s::mkv(18, 20_000, false), "mkv"),
        ("webm", s::mkv(19, 20_000, true), "webm"),
        ("wmv", s::wmv(20, 20_000, true), "wmv"),
        ("wma", s::wmv(21, 20_000, false), "wma"),
        ("flv", s::flv(22, 30), "flv"),
        ("ts", s::mpeg_ts(23, 3000), "ts"),
        ("mp3 with tag", s::mp3(24, 60, true), "mp3"),
        ("mp3 bare", s::mp3(25, 60, false), "mp3"),
        ("wav", s::wav(26, 20_000), "wav"),
        ("ogg", s::ogg(27, 6), "ogg"),
        ("midi", s::midi(28), "mid"),
        ("aiff", s::aiff(29, 9000), "aif"),
        ("pdf", s::pdf(30), "pdf"),
        ("pdf saved twice", s::pdf_updated(31), "pdf"),
        ("docx", s::docx(32), "docx"),
        ("xlsx", s::xlsx(33), "xlsx"),
        ("pptx", s::pptx(34), "pptx"),
        ("odt", s::odt(35), "odt"),
        ("doc", s::ole(36, "WordDocument", 20_000), "doc"),
        ("xls", s::ole(37, "Workbook", 20_000), "xls"),
        ("ppt", s::ole(38, "PowerPoint Document", 20_000), "ppt"),
        ("rtf", s::rtf(39), "rtf"),
        ("xml", s::xml(40), "xml"),
        ("svg", s::svg(41), "svg"),
        ("html", s::html(42), "html"),
        ("sqlite", s::sqlite(43, 6), "sqlite"),
        ("pst", s::pst(44, 20_000), "pst"),
        (
            "zip",
            s::zip(
                &[
                    ("a.txt", small_text.clone()),
                    ("dir/b.bin", vec![7u8; 3000]),
                ],
                false,
            ),
            "zip",
        ),
        (
            "zip written as a stream",
            s::zip(
                &[("a.txt", s::txt(2, 400)), ("b.txt", s::txt(3, 900))],
                true,
            ),
            "zip",
        ),
        ("rar 5", s::rar5(45, 9000), "rar"),
        ("rar 4", s::rar4(46, 9000), "rar"),
        ("7z", s::sevenz(47, 9000), "7z"),
        ("gzip", s::gzip(&s::txt(48, 4000), Some("notes.txt")), "gz"),
        (
            "tgz",
            s::gzip(&s::tar(&[("a.txt", s::txt(49, 300))]), None),
            "tgz",
        ),
        ("xz", s::xz(50, 9000), "xz"),
        (
            "tar",
            s::tar(&[("a.txt", s::txt(51, 300)), ("b.txt", s::txt(52, 2000))]),
            "tar",
        ),
        ("cab", s::cab(53, 9000), "cab"),
        ("exe", s::exe(54, 12_000, false), "exe"),
        ("dll", s::exe(55, 12_000, true), "dll"),
    ];
    all.extend(more_samples());
    all
}

#[test]
fn every_format_is_found_with_its_exact_length() {
    for (label, bytes, ext) in all_samples() {
        let hit = carve(&bytes).unwrap_or_else(|| panic!("{label}: not recognised"));
        assert_eq!(hit.ext, ext, "{label}");
        assert_eq!(hit.size, bytes.len() as u64, "{label}: length");
        assert!(!hit.partial && !hit.estimated, "{label}");
    }
}

#[test]
fn formats_without_a_stored_length_are_estimated() {
    let flac = s::flac(1, 40);
    let hit = carve_with(&Carver::new(&[]), &embed(&flac, true)).expect("flac");
    assert_eq!(
        (hit.ext, hit.size, hit.estimated),
        ("flac", flac.len() as u64, true)
    );
    // The next file's signature also ends it.
    let mut two = flac.clone();
    two.extend_from_slice(&s::png(2, 50, 50));
    let hit = carve(&two).expect("flac before png");
    assert_eq!(hit.size, flac.len() as u64);

    let text = s::txt(3, 900);
    let hit = carve_with(&Carver::new(&[]), &embed(&text, true)).expect("txt");
    assert_eq!(
        (hit.ext, hit.size, hit.estimated),
        ("txt", text.len() as u64, true)
    );
    // Plain text is off unless switched on, and never starts mid-text.
    assert!(carve_with(&Carver::new(&default_off()), &embed(&text, true)).is_none());
    let src = embed(&text, true);
    let head = src.read_vec(LEAD as u64, 512);
    assert!(Carver::new(&[])
        .identify(&src, LEAD as u64, &head, true)
        .is_none());
}

#[test]
fn broken_off_files_are_marked_partial() {
    // A JPEG whose second half was overwritten by something else.
    let jpeg = s::jpeg(1, 640, 480);
    let cut = &jpeg[..jpeg.len() * 2 / 3];
    let hit = carve(cut).expect("truncated jpeg");
    assert!(hit.partial);
    assert!(hit.size >= 4096 && hit.size < jpeg.len() as u64 + 70_000);
    // A ZIP that lost its directory.
    let zip = s::zip(
        &[("a.txt", s::txt(2, 800)), ("b.txt", s::txt(3, 800))],
        false,
    );
    let central = zip.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    let hit = carve(&zip[..central]).expect("zip without directory");
    assert!(hit.partial);
    assert_eq!(hit.size, central as u64);
    // Files that are cut off right at the start are not worth listing.
    assert!(carve(&jpeg[..600]).is_none());
    assert!(carve(&s::png(4, 100, 100)[..300]).is_none());
    assert!(carve(&s::mp4(5, 9000)[..40]).is_none());
}

#[test]
fn switched_off_types_are_ignored() {
    let jpeg = s::jpeg(1, 100, 80);
    let docx = s::docx(2);
    let off = Carver::new(&["jpg".to_string(), "docx".to_string()]);
    assert!(carve_with(&off, &embed(&jpeg, false)).is_none());
    assert!(carve_with(&off, &embed(&docx, false)).is_none());
    // A plain ZIP is still found when only the Office types are off.
    let zip = s::zip(&[("a.txt", s::txt(3, 100))], false);
    assert!(carve_with(&off, &embed(&zip, false)).is_some());
    let only_zip_off = Carver::new(&["zip".to_string()]);
    assert!(carve_with(&only_zip_off, &embed(&zip, false)).is_none());
    assert!(carve_with(&only_zip_off, &embed(&docx, false)).is_some());
}

#[test]
fn random_data_contains_no_files() {
    let mut rng = Rng(0xBAD_5EED);
    let data = rng.bytes(6 << 20);
    let src = Source::from_bytes(data.clone());
    let carver = Carver::new(&default_off());
    let mut hits = Vec::new();
    for off in (0..data.len()).step_by(512) {
        let head = &data[off..(off + 512).min(data.len())];
        if let Some(h) = carver.identify(&src, off as u64, head, false) {
            hits.push((off, h));
        }
    }
    assert!(hits.is_empty(), "false finds in noise: {hits:?}");
}

#[test]
fn damaged_files_never_panic() {
    let carver = Carver::new(&[]);
    let mut rng = Rng(0xF022);
    for (label, bytes, _) in all_samples() {
        for round in 0..120 {
            let mut b = bytes.clone();
            match round % 4 {
                // Cut off at a random point.
                0 => b.truncate(64 + rng.below(b.len() as u64 - 64) as usize),
                // A few random bytes changed, mostly near the start where the structure is.
                1 | 2 => {
                    for _ in 0..(1 + rng.below(6)) {
                        let span = if round % 4 == 1 {
                            b.len().min(400)
                        } else {
                            b.len()
                        };
                        let at = rng.below(span as u64) as usize;
                        b[at] = rng.next_u64() as u8;
                    }
                }
                // A whole field replaced by 0xFF: sizes and offsets become huge.
                _ => {
                    let at = 4 + rng.below((b.len().min(256) - 12) as u64) as usize;
                    b[at..at + 8].fill(0xFF);
                }
            }
            let src = embed(&b, round % 2 == 0);
            let head = src.read_vec(LEAD as u64, 512);
            if let Some(h) = carver.identify(&src, LEAD as u64, &head, false) {
                assert!(
                    h.size >= 64 && h.size <= src.len() - LEAD as u64,
                    "{label}: size {} out of bounds",
                    h.size
                );
            }
        }
    }
}

#[test]
fn signatures_followed_by_garbage_never_panic() {
    let magics: [&[u8]; 30] = [
        b"\xFF\xD8\xFF\xE0",
        b"\x89PNG\r\n\x1a\n",
        b"GIF89a",
        b"BM",
        b"II*\0",
        b"MM\0*",
        b"8BPS\0\x01",
        b"RIFF",
        b"\0\0\0\x18ftyp",
        b"\x1A\x45\xDF\xA3",
        b"\x30\x26\xB2\x75\x8E\x66\xCF\x11\xA6\xD9\x00\xAA\x00\x62\xCE\x6C",
        b"ID3\x03\0\0",
        b"\xFF\xFB\x90\x64",
        b"fLaC\x80\0\0\x22",
        b"OggS\0\x02",
        b"%PDF-1.7\n",
        b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1",
        b"PK\x03\x04",
        b"Rar!\x1a\x07\x00",
        b"Rar!\x1a\x07\x01\x00",
        b"7z\xBC\xAF\x27\x1C",
        b"\x1F\x8B\x08\0",
        b"\xFD7zXZ\0",
        b"MZ",
        b"SQLite format 3\0",
        b"!BDN",
        b"{\\rtf1",
        b"<?xml version=\"1.0\"?><a>",
        b"FLV\x01\x05\0\0\0\x09",
        b"MThd\0\0\0\x06",
    ];
    let carver = Carver::new(&[]);
    let mut rng = Rng(77);
    for magic in magics {
        for _ in 0..150 {
            let mut body = rng.bytes(3000);
            body[..magic.len()].copy_from_slice(magic);
            // Sprinkle zeros and 0xFF runs: common in real disks, extreme as field values.
            for _ in 0..rng.below(4) {
                let at = rng.below(2900) as usize;
                let v = if rng.below(2) == 0 { 0 } else { 0xFF };
                body[at..at + 16].fill(v);
            }
            let src = Source::from_bytes(body.clone());
            let _ = carver.identify(&src, 0, &body[..512], false);
            // The same candidate at the very end of the source.
            let short = Source::from_bytes(body[..600].to_vec());
            let _ = carver.identify(&short, 0, &body[..512], false);
        }
    }
}

// ------------------------------------------------------------------------------------
// Fonts, JPEG 2000, DjVu, metafiles, icons, more audio and video, Zstandard,
// LZ4, ar, registry hives, event logs, contacts, calendars, ELF, captures,
// shortcuts. Their samples are built here, byte by byte.
// ------------------------------------------------------------------------------------

fn noise(seed: u64, len: usize) -> Vec<u8> {
    Rng(seed).bytes(len)
}

fn be(v: usize) -> [u8; 4] {
    (v as u32).to_be_bytes()
}

fn le(v: usize) -> [u8; 4] {
    (v as u32).to_le_bytes()
}

fn le64(v: usize) -> [u8; 8] {
    (v as u64).to_le_bytes()
}

/// A TrueType or OpenType font: a table directory, then the padded tables.
fn font(seed: u64, magic: &[u8; 4]) -> Vec<u8> {
    let tables: [(&[u8; 4], usize); 5] = [
        (b"cmap", 100),
        (b"glyf", 333),
        (b"head", 54),
        (b"maxp", 32),
        (b"name", 77),
    ];
    let first = 12 + 16 * tables.len();
    let mut out = magic.to_vec();
    out.extend(5u16.to_be_bytes());
    out.extend([0u8; 6]);
    let mut body = Vec::new();
    for (i, (tag, len)) in tables.iter().enumerate() {
        out.extend(*tag);
        out.extend(be(0));
        out.extend(be(first + body.len()));
        out.extend(be(*len));
        body.extend(noise(seed + i as u64, *len));
        body.resize(body.len().div_ceil(4) * 4, 0);
    }
    out.extend(body);
    out
}

fn web_font(seed: u64, two: bool) -> Vec<u8> {
    let body = noise(seed, 700);
    let header = if two { 48 } else { 44 + 3 * 20 };
    let mut out = if two { b"wOF2" } else { b"wOFF" }.to_vec();
    out.extend(be(0x0001_0000));
    out.extend(be(header + body.len()));
    out.extend(3u16.to_be_bytes());
    out.extend([0u8; 2]);
    out.extend(be(4000));
    if two {
        out.extend(be(600));
    }
    out.resize(header, 0);
    out.extend(body);
    out
}

/// A JPEG 2000 codestream with one tile part. `open` leaves the length of
/// the tile part at zero ("up to the end marker").
fn j2k(seed: u64, len: usize, open: bool) -> Vec<u8> {
    let mut out = vec![0xFF, 0x4F, 0xFF, 0x51];
    out.extend(41u16.to_be_bytes());
    out.extend([0u8; 2]);
    out.extend(be(640));
    out.extend(be(480));
    out.extend([0u8; 8]);
    out.extend(be(640));
    out.extend(be(480));
    out.extend([0u8; 8]);
    out.extend(1u16.to_be_bytes());
    out.extend([7, 1, 1]);
    out.extend([0xFF, 0x52, 0, 12, 0, 0, 0, 1, 0, 5, 4, 4, 0, 0]);
    out.extend([0xFF, 0x5C, 0, 4, 0x40, 0x40]);
    out.extend([0xFF, 0x90, 0, 10, 0, 0]);
    out.extend(be(if open { 0 } else { 14 + len }));
    out.extend([0, 1, 0xFF, 0x93]);
    // Packet data never contains a marker.
    out.extend(
        noise(seed, len)
            .into_iter()
            .map(|b| if b == 0xFF { 0x7F } else { b }),
    );
    out.extend([0xFF, 0xD9]);
    out
}

fn jp2(seed: u64, len: usize, open_box: bool, open_tile: bool) -> Vec<u8> {
    let mut out = formats::JP2_SIGNATURE.to_vec();
    out.extend(be(20));
    out.extend(b"ftypjp2 \0\0\0\0jp2 ");
    out.extend(be(30));
    out.extend(b"jp2h");
    out.extend(be(22));
    out.extend(b"ihdr");
    out.extend(be(480));
    out.extend(be(640));
    out.extend([0, 1, 7, 7, 0, 0]);
    let code = j2k(seed, len, open_tile);
    out.extend(be(if open_box { 0 } else { 8 + code.len() }));
    out.extend(b"jp2c");
    out.extend(code);
    out
}

fn djvu(seed: u64, len: usize) -> Vec<u8> {
    let mut out = b"AT&TFORM".to_vec();
    out.extend(be(4 + len));
    out.extend(b"DJVU");
    out.extend(noise(seed, len));
    out
}

fn emf(records: usize) -> Vec<u8> {
    let total = 88 + 16 * records + 20;
    let mut out = Vec::new();
    out.extend(le(1));
    out.extend(le(88));
    out.extend([0u8; 32]);
    out.extend(b" EMF");
    out.extend(le(0x1_0000));
    out.extend(le(total));
    out.extend(le(records + 2));
    out.extend([1, 0, 0, 0]);
    out.resize(88, 0);
    for i in 0..records {
        // "Move to" records.
        for v in [27, 16, i * 10, i * 7] {
            out.extend(le(v));
        }
    }
    for v in [14, 20, 0, 0, 20] {
        out.extend(le(v));
    }
    out
}

fn icon(seed: u64, cursor: bool) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(noise(seed, 192));
    let mut bitmap = Vec::new();
    bitmap.extend(le(40));
    bitmap.extend(le(16));
    bitmap.extend(le(32));
    bitmap.extend([1, 0, 32, 0]);
    bitmap.extend(noise(seed + 1, 284));
    let mut out = vec![0, 0, if cursor { 2 } else { 1 }, 0, 2, 0];
    let mut at = 6 + 2 * 16;
    for image in [&png, &bitmap] {
        out.extend([16, 16, 0, 0, 1, 0, 32, 0]);
        out.extend(le(image.len()));
        out.extend(le(at));
        at += image.len();
    }
    out.extend(png);
    out.extend(bitmap);
    out
}

fn dsf(seed: u64, len: usize) -> Vec<u8> {
    let mut out = b"DSD ".to_vec();
    out.extend(le64(28));
    out.extend(le64(28 + 52 + 12 + len));
    out.extend(le64(0));
    out.extend(b"fmt ");
    out.extend(le64(52));
    out.extend([0u8; 40]);
    out.extend(b"data");
    out.extend(le64(12 + len));
    out.extend(noise(seed, len));
    out
}

/// An APE tag with header and footer, as audio players append it.
fn ape_tag(seed: u64) -> Vec<u8> {
    let items = noise(seed, 90);
    let mut out = Vec::new();
    for flags in [0xA000_0000usize, 0x8000_0000] {
        out.extend(b"APETAGEX");
        out.extend(le(2000));
        out.extend(le(items.len() + 32));
        out.extend(le(1));
        out.extend(le(flags));
        out.extend([0u8; 8]);
        if flags == 0xA000_0000 {
            out.extend(&items);
        }
    }
    out
}

fn wavpack(seed: u64, blocks: usize, tag: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..blocks {
        let body = noise(seed + i as u64, 200 + i * 37);
        out.extend(b"wvpk");
        out.extend(le(24 + body.len()));
        out.extend(0x410u16.to_le_bytes());
        out.extend([0u8; 2]);
        out.extend(le(blocks * 1000));
        out.extend(le(i * 1000));
        out.extend(le(1000));
        out.extend(le(0x1800_0125));
        out.extend(le(0x1234_5678));
        out.extend(body);
    }
    if tag {
        out.extend(ape_tag(seed));
    }
    out
}

fn amr(seed: u64, frames: usize, wide: bool) -> Vec<u8> {
    let (mut out, sizes): (Vec<u8>, [usize; 8]) = if wide {
        (b"#!AMR-WB\n".to_vec(), [18, 24, 33, 37, 41, 47, 51, 59])
    } else {
        (b"#!AMR\n".to_vec(), [13, 14, 16, 18, 20, 21, 27, 32])
    };
    for i in 0..frames {
        let mode = i % 8;
        out.push((mode as u8) << 3 | 4);
        out.extend(noise(seed + i as u64, sizes[mode] - 1));
    }
    out
}

fn au(seed: u64, len: usize) -> Vec<u8> {
    let mut out = b".snd".to_vec();
    for v in [28, len, 3, 44_100, 2, 0] {
        out.extend(be(v));
    }
    out.extend(noise(seed, len));
    out
}

fn ape(seed: u64, len: usize, tag: bool) -> Vec<u8> {
    let mut out = b"MAC ".to_vec();
    out.extend(3990u16.to_le_bytes());
    out.extend([0u8; 2]);
    for v in [52, 24, 8, 0, len, 0, 0] {
        out.extend(le(v));
    }
    out.extend([0u8; 16]);
    out.extend(noise(seed, 24 + 8 + len));
    if tag {
        out.extend(ape_tag(seed));
    }
    out
}

/// An MPEG program stream: packs with one video packet each.
fn mpeg_ps(seed: u64, packs: usize, mpeg2: bool, end_code: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..packs {
        out.extend([0, 0, 1, 0xBA]);
        if mpeg2 {
            out.extend([0x44, 0x00, 0x04, 0x00, 0x04, 0x01, 0x01, 0x89, 0xC3, 0xF8]);
        } else {
            out.extend([0x21, 0x00, 0x01, 0x00, 0x01, 0x80, 0x00, 0x01]);
        }
        if i == 0 {
            // The system header.
            out.extend([0, 0, 1, 0xBB, 0, 12]);
            out.extend(noise(seed, 12));
        }
        let body = noise(seed + i as u64, 1500 + i * 11);
        out.extend([0, 0, 1, 0xE0]);
        out.extend((body.len() as u16).to_be_bytes());
        out.extend(body);
    }
    if end_code {
        out.extend([0, 0, 1, 0xB9]);
    }
    out
}

fn swf(seed: u64, len: usize) -> Vec<u8> {
    // The stage: 5 bits "15", then 0, 11000, 0, 8000 in 15 bits each.
    let mut bits = Vec::new();
    for (value, width) in [(15u32, 5), (0, 15), (11_000, 15), (0, 15), (8000, 15)] {
        bits.extend((0..width).rev().map(|b| value >> b & 1 == 1));
    }
    let rect: Vec<u8> = bits
        .chunks(8)
        .map(|c| {
            c.iter()
                .enumerate()
                .fold(0u8, |v, (i, bit)| v | (*bit as u8) << (7 - i))
        })
        .collect();
    let mut out = b"FWS\x0A".to_vec();
    out.extend(le(8 + rect.len() + 4 + len + 2));
    out.extend(rect);
    out.extend([0, 24, 1, 0]);
    out.extend(noise(seed, len));
    out.extend([0, 0]);
    out
}

fn zstd_block(out: &mut Vec<u8>, kind: u32, size: usize, last: bool, body: &[u8]) {
    let v = (size as u32) << 3 | kind << 1 | last as u32;
    out.extend(&v.to_le_bytes()[..3]);
    out.extend(body);
}

/// Two Zstandard frames with a skippable frame between them.
fn zstd(seed: u64) -> Vec<u8> {
    // Checksum at the end, a window byte, no content size.
    let mut out = vec![0x28, 0xB5, 0x2F, 0xFD, 0x04, 0x58];
    zstd_block(&mut out, 0, 300, false, &noise(seed, 300));
    zstd_block(&mut out, 1, 1000, false, &[0x55]);
    // A packed block: it opens with (here: no) literals.
    let mut packed = noise(seed + 1, 200);
    packed[0] = 0;
    zstd_block(&mut out, 2, 200, true, &packed);
    out.extend(noise(seed + 2, 4));
    out.extend([0x53, 0x2A, 0x4D, 0x18]);
    out.extend(le(16));
    out.extend(noise(seed + 3, 16));
    // One segment with a two-byte content size.
    out.extend([0x28, 0xB5, 0x2F, 0xFD, 0x60, 0x90, 0x00]);
    zstd_block(&mut out, 0, 400, true, &noise(seed + 4, 400));
    out
}

fn lz4(seed: u64) -> Vec<u8> {
    // Block checksums, content size, content checksum; blocks of up to 64 KB.
    let mut out = vec![0x04, 0x22, 0x4D, 0x18, 0x7C, 0x40];
    out.extend(le64(1100));
    let check = (formats::xxh32_short(&out[4..]) >> 8) as u8;
    out.push(check);
    for (i, len) in [500usize, 600].into_iter().enumerate() {
        out.extend(le(len | 0x8000_0000));
        out.extend(noise(seed + i as u64, len + 4));
    }
    out.extend(le(0));
    out.extend(noise(seed + 9, 4));
    out
}

fn ar(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"!<arch>\n".to_vec();
    for (name, data) in members {
        let header = format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            name,
            1_700_000_000,
            0,
            0,
            100_644,
            data.len()
        );
        assert_eq!(header.len(), 60);
        out.extend(header.as_bytes());
        out.extend(data);
        if data.len() % 2 == 1 {
            out.push(b'\n');
        }
    }
    out
}

fn deb(seed: u64) -> Vec<u8> {
    ar(&[
        ("debian-binary", b"2.0\n".to_vec()),
        ("control.tar.gz", noise(seed, 301)),
        ("data.tar.xz", noise(seed + 1, 2000)),
    ])
}

fn regf(seed: u64, bins: usize) -> Vec<u8> {
    let mut out = vec![0u8; 4096];
    out[..4].copy_from_slice(b"regf");
    out[4..12].copy_from_slice(&[9, 0, 0, 0, 9, 0, 0, 0]);
    out[20..24].copy_from_slice(&le(1));
    out[24..28].copy_from_slice(&le(5));
    out[36..40].copy_from_slice(&le(32));
    out[40..44].copy_from_slice(&le(bins * 4096));
    let sum = out[..508].chunks_exact(4).fold(0u32, |v, c| {
        v ^ u32::from_le_bytes([c[0], c[1], c[2], c[3]])
    });
    out[508..512].copy_from_slice(&sum.to_le_bytes());
    for i in 0..bins {
        out.extend(b"hbin");
        out.extend(noise(seed + i as u64, 4092));
    }
    out
}

fn evtx(seed: u64, chunks: usize) -> Vec<u8> {
    let mut out = vec![0u8; 4096];
    out[..8].copy_from_slice(b"ElfFile\0");
    out[16..24].copy_from_slice(&le64(chunks - 1));
    out[32..36].copy_from_slice(&le(128));
    out[36..40].copy_from_slice(&[1, 0, 3, 0]);
    out[40..42].copy_from_slice(&4096u16.to_le_bytes());
    out[42..44].copy_from_slice(&(chunks as u16).to_le_bytes());
    let sum = crate::util::crc32(&out[..120]);
    out[124..128].copy_from_slice(&sum.to_le_bytes());
    for i in 0..chunks {
        out.extend(b"ElfChnk\0");
        out.extend(noise(seed + i as u64, 65_528));
    }
    out
}

fn vcard(cards: usize) -> Vec<u8> {
    let mut out = String::new();
    for i in 0..cards {
        out += &format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Sample;Person {i};;;\r\nFN:Person {i} Sample\r\n\
             TEL;TYPE=CELL:+00 000 000 {i:04}\r\nNOTE:An invented contact for the tests.\r\n\
             END:VCARD\r\n"
        );
    }
    out.into_bytes()
}

fn icalendar(events: usize) -> Vec<u8> {
    let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Tests//EN\r\n");
    for i in 0..events {
        out += &format!(
            "BEGIN:VEVENT\r\nUID:{i}@tests.invalid\r\nDTSTART:20240102T10{i:02}00Z\r\n\
             SUMMARY:Invented meeting {i}\r\nEND:VEVENT\r\n"
        );
    }
    out += "END:VCALENDAR\r\n";
    out.into_bytes()
}

/// A 64-bit ELF program: one segment, three sections, the section table at the end.
fn elf64(seed: u64) -> Vec<u8> {
    let mut out = b"\x7FELF\x02\x01\x01".to_vec();
    out.resize(16, 0);
    out.extend([2, 0, 0x3E, 0, 1, 0, 0, 0]);
    out.extend(le64(0x40_1000));
    out.extend(le64(64));
    out.extend(le64(400));
    out.extend(le(0));
    for v in [64u16, 56, 1, 64, 3, 2] {
        out.extend(v.to_le_bytes());
    }
    // The segment: the first 400 bytes of the file.
    out.extend(le(1));
    out.extend(le(5));
    for v in [0, 0x40_0000, 0x40_0000, 400, 400, 4096] {
        out.extend(le64(v));
    }
    out.extend(noise(seed, 400 - 120));
    // An unused entry, a code section and one that takes no room in the file.
    out.extend([0u8; 64]);
    for (kind, offset, size) in [(1, 120, 280), (8, 400, 1 << 40)] {
        out.extend(le(1));
        out.extend(le(kind));
        out.extend(le64(2));
        out.extend(le64(0x40_0000));
        out.extend(le64(offset));
        out.extend(le64(size));
        out.extend([0u8; 24]);
    }
    out
}

/// A 32-bit ELF program without a section table.
fn elf32(seed: u64) -> Vec<u8> {
    let mut out = b"\x7FELF\x01\x01\x01".to_vec();
    out.resize(16, 0);
    out.extend([2, 0, 3, 0, 1, 0, 0, 0]);
    for v in [0x0804_8000, 52, 0, 0] {
        out.extend(le(v));
    }
    for v in [52u16, 32, 1, 0, 0, 0] {
        out.extend(v.to_le_bytes());
    }
    for v in [1, 0, 0x0804_8000, 0x0804_8000, 500, 500, 5, 4096] {
        out.extend(le(v));
    }
    out.extend(noise(seed, 500 - 84));
    out
}

fn pcap(seed: u64, packets: usize, nanoseconds: bool) -> Vec<u8> {
    let mut out = if nanoseconds {
        vec![0x4D, 0x3C, 0xB2, 0xA1]
    } else {
        vec![0xD4, 0xC3, 0xB2, 0xA1]
    };
    out.extend([2, 0, 4, 0]);
    for v in [0, 0, 65_535, 1] {
        out.extend(le(v));
    }
    for i in 0..packets {
        let len = 60 + i * 97 % 1400;
        for v in [1_700_000_000 + i, i * 1000, len, len] {
            out.extend(le(v));
        }
        out.extend(noise(seed + i as u64, len));
    }
    out
}

/// A Windows shortcut with every kind of part: id list, link info, a name
/// and one extra block.
fn lnk(seed: u64) -> Vec<u8> {
    let mut out = formats::LNK_HEADER.to_vec();
    out.extend(le(0x87));
    out.extend(le(0x20));
    out.extend(noise(seed, 24));
    out.extend(le(12_345));
    out.extend(le(0));
    out.extend(le(1));
    out.extend([0u8; 12]);
    assert_eq!(out.len(), 76);
    out.extend(22u16.to_le_bytes());
    out.extend(noise(seed + 1, 22));
    out.extend(le(60));
    out.extend(noise(seed + 2, 56));
    out.extend(6u16.to_le_bytes());
    out.extend(noise(seed + 3, 12));
    out.extend(le(20));
    out.extend(le(0xA000_0003));
    out.extend(noise(seed + 4, 12));
    out.extend(le(0));
    out
}

fn more_samples() -> Vec<(&'static str, Vec<u8>, &'static str)> {
    vec![
        ("ttf", font(60, &[0, 1, 0, 0]), "ttf"),
        ("otf", font(61, b"OTTO"), "otf"),
        ("woff", web_font(62, false), "woff"),
        ("woff2", web_font(63, true), "woff2"),
        ("jp2", jp2(64, 5000, false, false), "jp2"),
        (
            "jp2 with an open codestream box",
            jp2(65, 5000, true, false),
            "jp2",
        ),
        (
            "jp2 with an open box and tile",
            jp2(66, 70_000, true, true),
            "jp2",
        ),
        ("j2k", j2k(67, 5000, false), "j2k"),
        ("j2k with an open tile", j2k(68, 5000, true), "j2k"),
        ("djvu", djvu(69, 5000), "djvu"),
        ("emf", emf(40), "emf"),
        ("ico", icon(70, false), "ico"),
        ("cur", icon(71, true), "cur"),
        ("dsf", dsf(72, 9000), "dsf"),
        ("wavpack", wavpack(73, 6, false), "wv"),
        ("wavpack with a tag", wavpack(74, 6, true), "wv"),
        ("amr", amr(75, 200, false), "amr"),
        ("amr wide band", amr(76, 200, true), "amr"),
        ("au", au(77, 9000), "au"),
        ("ape", ape(78, 9000, false), "ape"),
        ("ape with a tag", ape(79, 9000, true), "ape"),
        ("mpeg-2 program stream", mpeg_ps(80, 40, true, true), "mpg"),
        ("mpeg-1 program stream", mpeg_ps(81, 40, false, true), "mpg"),
        ("swf", swf(82, 5000), "swf"),
        ("zstd", zstd(83), "zst"),
        ("lz4", lz4(84), "lz4"),
        ("deb", deb(85), "deb"),
        (
            "static library",
            ar(&[
                ("/", noise(86, 40)),
                ("one.o/", noise(87, 777)),
                ("two.o/", noise(88, 1200)),
            ]),
            "a",
        ),
        ("registry hive", regf(89, 3), "hve"),
        ("evtx", evtx(90, 2), "evtx"),
        ("vcard", vcard(4), "vcf"),
        ("icalendar", icalendar(5), "ics"),
        ("elf 64-bit", elf64(91), "elf"),
        ("elf 32-bit", elf32(92), "elf"),
        ("pcap", pcap(93, 25, false), "pcap"),
        ("pcap in nanoseconds", pcap(94, 25, true), "pcap"),
        ("lnk", lnk(95), "lnk"),
    ]
}

#[test]
fn lz4_header_check_matches_known_headers() {
    // The two headers that the lz4 tool writes by default.
    assert_eq!((formats::xxh32_short(&[0x64, 0x40]) >> 8) as u8, 0xA7);
    assert_eq!((formats::xxh32_short(&[0x64, 0x70]) >> 8) as u8, 0xB9);
    assert_eq!(formats::xxh32_short(&[]), 0x02CC_5D05);
}

#[test]
fn zstandard_as_written_by_the_reference_encoder() {
    // "beta epsilon alpha gamma ..." (2386 bytes of words) packed at level 3:
    // one frame with a single packed block.
    let frame = crate::util::from_hex(concat!(
        "28b52ffd608608050e0094036265746120657073696c6f6e20616c7068612067616d6d6164656c74",
        "7a65627a62627a7a7a7a627a626262626262626262627a6267616d6d6180dfa8819f61ff0e4042a3",
        "945979115008856044618c2a480a52a9d101b608fa88983263ace5a88e964d752e5ffac2fdabff30",
        "4a190e9f19caf1b03eb6f6eea7c70eccf88d2f26546ac5f2aa16f283926e765f0040eb1d342c7d4f",
        "7d9fb243b8863cda8df41636a92a99e8a606336e4fef50fd858e2d2f6776d10f299e744a31266804",
        "850256faf46f54b7256991e3c49fc8512e24812fea6d8e9807809bf498526a8652224d0d6e83aafc",
        "16efd8dc3e09a24fdab3430805621e6f07e5edad7083ad073a690d134d779f92a0a6221aeaa55162",
        "fef604c533ba75bfcd75f84b0717ef5598acdac36a609385b022aff6976baf9531a491377d000abb",
        "04a58fe79fab2166267c93551bb3064d4274f2119d0cbafd462f1aa0391cd8bd351e04564758dab8",
        "7448d8c0a08b886a2b7131f60b25288b7eb919f80ef2023d263f48e8de2f5b15882621020e05b276",
        "2609df3e615b5235ecfe7822472a62dd36d971b61b806550be7d60cc66555bce261d968ba1158dd1",
        "e39b5de2f64ead1e98564f787f3852f9c0ab",
    ))
    .expect("hex");
    assert_eq!(frame.len(), 458);
    let hit = carve(&frame).expect("zstd");
    assert_eq!((hit.ext, hit.size), ("zst", 458));
    // Two of them in a row are one file.
    let mut twice = frame.clone();
    twice.extend_from_slice(&frame);
    assert_eq!(carve(&twice).expect("two frames").size, 916);
    // A packed block whose literals would not fit in it is not Zstandard.
    let mut broken = frame.clone();
    broken[10] = 0xFF;
    broken[11] = 0xFF;
    assert!(carve(&broken).is_none());
}

#[test]
fn cut_off_files_are_never_longer_than_what_is_left() {
    let carver = Carver::new(&[]);
    for (label, bytes, _) in more_samples() {
        for cut in [65, 100, bytes.len() / 3, bytes.len() / 2, bytes.len() - 1] {
            // The source ends where the file was cut.
            let mut img = noise(5, LEAD);
            img.extend_from_slice(&bytes[..cut]);
            let src = Source::from_bytes(img);
            if let Some(h) = carve_with(&carver, &src) {
                assert!(h.size <= cut as u64, "{label} cut at {cut}: {}", h.size);
                assert!(h.size < bytes.len() as u64, "{label} cut at {cut}");
            }
        }
    }
}

#[test]
fn chains_end_where_the_next_file_begins() {
    let all = Carver::new(&[]);
    // Two WavPack files in a row: the second one counts its samples from zero again.
    let one = wavpack(1, 5, false);
    let mut two = one.clone();
    two.extend(wavpack(2, 5, false));
    assert_eq!(carve(&two).expect("wavpack").size, one.len() as u64);

    // A program stream that was cut before its end code still gives what is there.
    for blank in [false, true] {
        let stream = mpeg_ps(3, 30, true, false);
        let hit = carve_with(&all, &embed(&stream, blank)).expect("mpeg without end code");
        assert_eq!(
            (hit.ext, hit.size, hit.partial),
            ("mpg", stream.len() as u64, false)
        );
    }
    // Three packs are too few to be sure.
    assert!(carve(&mpeg_ps(4, 3, true, true)).is_none());

    // Speech followed by blank space: zeros are not frames.
    let speech = amr(5, 300, false);
    let hit = carve_with(&all, &embed(&speech, true)).expect("amr");
    assert_eq!(hit.size, speech.len() as u64);
    assert!(carve(&amr(6, 5, false)).is_none(), "too short to be sure");

    // Contacts end with the last card, not with the text behind them.
    let cards = vcard(3);
    let mut text = cards.clone();
    text.extend_from_slice(b"\r\nSome other text that happens to follow.\r\nEND:VCARD\r\n");
    assert_eq!(carve(&text).expect("vcard").size, cards.len() as u64);
    // A card without its END line is not listed.
    let card = vcard(1);
    assert!(carve(&card[..card.len() - 12]).is_none());

    // An event log that lost its last chunk is marked as partial.
    let log = evtx(7, 3);
    let mut damaged = log.clone();
    damaged[4096 + 2 * 65_536..4096 + 2 * 65_536 + 8].fill(0);
    let hit = carve(&damaged).expect("damaged evtx");
    assert_eq!(
        (hit.size, hit.partial),
        (4096 + 2 * 65_536, true),
        "two good chunks"
    );

    // A registry hive with a wrong header checksum is not trusted.
    let mut hive = regf(8, 2);
    hive[100] ^= 1;
    assert!(carve(&hive).is_none());

    // An LZ4 header that fails its check byte is not a frame.
    let mut packed = lz4(9);
    packed[14] ^= 0x10;
    assert!(carve(&packed).is_none());

    // Audio of unknown length ("streamed") cannot be sized.
    let mut sound = au(10, 4000);
    sound[8..12].fill(0xFF);
    assert!(carve(&sound).is_none());

    // Monkey's Audio before version 3.98 has another layout.
    let mut old = ape(11, 4000, false);
    old[4..6].copy_from_slice(&3970u16.to_le_bytes());
    assert!(carve(&old).is_none());
}

#[test]
fn fonts_without_padding_after_the_last_table() {
    // The sample ends with three bytes of padding.
    let padded = font(1, b"OTTO");
    let bare = &padded[..padded.len() - 3];
    let mut followed = bare.to_vec();
    followed.extend_from_slice(b"xyz");
    assert_eq!(carve(&followed).expect("otf").size, bare.len() as u64);
    // Followed by blank space the padding cannot be told from it; it is kept.
    let hit = carve_with(&Carver::new(&[]), &embed(bare, true)).expect("otf");
    assert_eq!(hit.size, padded.len() as u64);
    // Without the table that every font has, it is not a font.
    let mut headless = padded.clone();
    let at = headless.windows(4).position(|w| w == b"head").unwrap();
    headless[at..at + 4].copy_from_slice(b"hxad");
    assert!(carve(&headless).is_none());
}

#[test]
fn new_types_can_be_switched_off() {
    let ttf = font(1, &[0, 1, 0, 0]);
    let package = deb(2);
    let library = ar(&[("one.o/", noise(3, 500))]);
    let off = Carver::new(&["font".to_string(), "deb".to_string()]);
    assert!(carve_with(&off, &embed(&ttf, false)).is_none());
    assert!(carve_with(&off, &embed(&web_font(4, true), false)).is_none());
    assert!(carve_with(&off, &embed(&package, false)).is_none());
    // Static libraries share the container but are their own type.
    assert_eq!(
        carve_with(&off, &embed(&library, false)).map(|h| h.ext),
        Some("a")
    );
    let off = Carver::new(&["ar".to_string(), "jp2".to_string()]);
    assert!(carve_with(&off, &embed(&library, false)).is_none());
    assert!(carve_with(&off, &embed(&package, false)).is_some());
    assert!(carve_with(&off, &embed(&jp2(5, 3000, false, false), false)).is_none());
    assert!(carve_with(&off, &embed(&j2k(6, 3000, false), false)).is_none());
    assert!(carve_with(&off, &embed(&ttf, false)).is_some());
}

#[test]
fn new_signatures_followed_by_garbage_are_not_files() {
    let mut emf_start = vec![1, 0, 0, 0, 88, 0, 0, 0];
    emf_start.resize(40, 0);
    emf_start.extend(b" EMF\0\0\x01\0");
    let magics: Vec<Vec<u8>> = vec![
        vec![0, 1, 0, 0, 0, 9],
        b"OTTO\0\x09".to_vec(),
        b"wOFF\0\x01\0\0".to_vec(),
        b"wOF2OTTO".to_vec(),
        formats::JP2_SIGNATURE.to_vec(),
        vec![0xFF, 0x4F, 0xFF, 0x51, 0x00, 0x29],
        b"AT&TFORM".to_vec(),
        emf_start,
        vec![0, 0, 1, 0, 3, 0],
        vec![0, 0, 2, 0, 1, 0],
        b"DSD \x1C\0\0\0\0\0\0\0".to_vec(),
        b"wvpk".to_vec(),
        b"#!AMR\n".to_vec(),
        b"#!AMR-WB\n".to_vec(),
        b".snd\0\0\0\x18".to_vec(),
        b"MAC \x96\x0F".to_vec(),
        vec![0, 0, 1, 0xBA, 0x44, 0x00, 0x04, 0x00, 0x04, 0x01],
        b"FWS\x0A".to_vec(),
        vec![0x28, 0xB5, 0x2F, 0xFD],
        vec![0x04, 0x22, 0x4D, 0x18, 0x64, 0x40, 0xA7],
        b"!<arch>\n".to_vec(),
        b"regf".to_vec(),
        b"ElfFile\0".to_vec(),
        b"BEGIN:VCARD\r\nVERSION:3.0\r\n".to_vec(),
        b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\n".to_vec(),
        b"\x7FELF\x02\x01\x01".to_vec(),
        b"\x7FELF\x01\x01\x01".to_vec(),
        vec![0xD4, 0xC3, 0xB2, 0xA1, 2, 0, 4, 0],
        vec![0x4D, 0x3C, 0xB2, 0xA1, 2, 0, 4, 0],
        formats::LNK_HEADER.to_vec(),
    ];
    let carver = Carver::new(&[]);
    let mut rng = Rng(78);
    for magic in &magics {
        for round in 0..300 {
            let mut body = rng.bytes(3000);
            body[..magic.len()].copy_from_slice(magic);
            // Every other round: runs of zeros and 0xFF, common on real disks
            // and extreme as field values.
            let sprinkled = round % 2 == 1;
            if sprinkled {
                for _ in 0..(1 + rng.below(4)) {
                    let at = magic.len() + rng.below(2900 - magic.len() as u64) as usize;
                    let v = if rng.below(2) == 0 { 0 } else { 0xFF };
                    body[at..at + 16].fill(v);
                }
            }
            let src = Source::from_bytes(body.clone());
            let found = carver.identify(&src, 0, &body[..512], false);
            if let Some(h) = found {
                assert!(h.size <= 3000, "{magic:02X?}: size {}", h.size);
            }
            assert!(
                found.is_none() || sprinkled,
                "{magic:02X?} and noise taken for a file: {found:?}"
            );
            // The same candidate at the very end of the source.
            let short = Source::from_bytes(body[..600].to_vec());
            let _ = carver.identify(&short, 0, &body[..512], false);
        }
    }
}

#[test]
fn every_new_type_has_a_sample() {
    // Each extension of the types added with these readers is produced by a sample.
    let made: HashSet<&str> = more_samples().iter().map(|(_, _, ext)| *ext).collect();
    for id in [
        "jp2", "djvu", "emf", "ico", "mpg", "dsf", "wv", "ape", "amr", "au", "vcf", "ics", "zst",
        "lz4", "deb", "elf", "ar", "font", "swf", "hve", "evtx", "lnk", "pcap",
    ] {
        let t = TYPES.iter().find(|t| t.id == id).expect(id);
        assert!(t.default_on, "{id}");
        for ext in t.exts {
            assert!(made.contains(ext), "no sample for .{ext}");
        }
    }
}
