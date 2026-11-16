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
    vec![
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
    ]
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
