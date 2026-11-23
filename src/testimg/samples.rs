//! Generators for small, structurally valid files of many types. Pictures
//! are produced by the `image` crate's encoders; everything else is built by
//! hand from the format's documented layout.

use crate::util::{crc32, Rng};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use std::io::Cursor;

fn noise(seed: u64, len: usize) -> Vec<u8> {
    Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1).bytes(len)
}

/// Noise without zero bytes (so that it never looks like the end of anything).
fn dense(seed: u64, len: usize) -> Vec<u8> {
    noise(seed, len).into_iter().map(|b| b | 1).collect()
}

/// A small invented landscape: sky gradient, sun, two ranges of hills.
pub fn picture(seed: u64, w: u32, h: u32) -> RgbImage {
    let mut r = Rng(seed.wrapping_mul(7919) | 1);
    let palettes: [([u8; 3], [u8; 3], [u8; 3]); 6] = [
        ([255, 190, 120], [90, 120, 200], [40, 70, 60]),
        ([120, 190, 255], [220, 240, 255], [60, 110, 70]),
        ([250, 140, 110], [70, 60, 130], [30, 40, 70]),
        ([180, 220, 250], [250, 250, 240], [110, 130, 90]),
        ([255, 210, 150], [200, 110, 140], [80, 50, 80]),
        ([150, 210, 200], [240, 250, 230], [50, 90, 100]),
    ];
    let (top, bottom, hill) = palettes[r.below(6) as usize];
    let sun = (
        (0.2 + r.below(60) as f32 / 100.0) * w as f32,
        (0.2 + r.below(30) as f32 / 100.0) * h as f32,
        (0.06 + r.below(8) as f32 / 100.0) * w.min(h) as f32,
    );
    let (f1, p1, f2, p2) = (
        0.6 + r.below(90) as f32 / 100.0,
        r.below(628) as f32 / 100.0,
        0.9 + r.below(120) as f32 / 100.0,
        r.below(628) as f32 / 100.0,
    );
    // The ridge lines only depend on the column.
    let ridges: Vec<(f32, f32)> = (0..w)
        .map(|x| {
            let fx = x as f32 / w as f32;
            (
                0.60 + 0.07 * (fx * f1 * std::f32::consts::TAU + p1).sin()
                    + 0.02 * (fx * f1 * 3.7 * std::f32::consts::TAU + p2).sin(),
                0.79 + 0.05 * (fx * f2 * std::f32::consts::TAU + p2).sin()
                    + 0.015 * (fx * f2 * 4.3 * std::f32::consts::TAU + p1).sin(),
            )
        })
        .collect();
    let grain_seed = seed.wrapping_mul(0x9E37_79B9) as u32;
    RgbImage::from_fn(w, h, |x, y| {
        let fy = y as f32 / h as f32;
        let mix = |a: u8, b: u8, t: f32| (a as f32 + (b as f32 - a as f32) * t) as u8;
        let mut c = [
            mix(top[0], bottom[0], fy),
            mix(top[1], bottom[1], fy),
            mix(top[2], bottom[2], fy),
        ];
        let d = ((x as f32 - sun.0).powi(2) + (y as f32 - sun.1).powi(2)).sqrt();
        if d < sun.2 {
            c = [255, 244, 214];
        } else if d < sun.2 * 2.2 {
            let t = 1.0 - (d - sun.2) / (sun.2 * 1.2);
            c = [
                mix(c[0], 255, t * 0.5),
                mix(c[1], 240, t * 0.5),
                mix(c[2], 200, t * 0.5),
            ];
        }
        let (far, near) = ridges[x as usize];
        if fy > near {
            c = hill;
        } else if fy > far {
            c = [
                mix(hill[0], c[0], 0.45),
                mix(hill[1], c[1], 0.45),
                mix(hill[2], c[2], 0.45),
            ];
        }
        // A little film grain, so that the files have a realistic size.
        let mut n = x.wrapping_mul(0x27D4_EB2D) ^ y.wrapping_mul(0x1656_67B1) ^ grain_seed;
        n ^= n >> 15;
        n = n.wrapping_mul(0x2C1B_3C6D);
        n ^= n >> 12;
        let g = (n & 15) as i16 - 8;
        Rgb(c.map(|v| (v as i16 + g).clamp(0, 255) as u8))
    })
}

fn encode(seed: u64, w: u32, h: u32, format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(picture(seed, w, h))
        .write_to(&mut out, format)
        .expect("encode sample picture");
    out.into_inner()
}

pub fn jpeg(seed: u64, w: u32, h: u32) -> Vec<u8> {
    encode(seed, w, h, ImageFormat::Jpeg)
}
pub fn png(seed: u64, w: u32, h: u32) -> Vec<u8> {
    encode(seed, w, h, ImageFormat::Png)
}
pub fn gif(seed: u64, w: u32, h: u32) -> Vec<u8> {
    encode(seed, w, h, ImageFormat::Gif)
}
pub fn bmp(seed: u64, w: u32, h: u32) -> Vec<u8> {
    encode(seed, w, h, ImageFormat::Bmp)
}
pub fn tiff(seed: u64, w: u32, h: u32) -> Vec<u8> {
    encode(seed, w, h, ImageFormat::Tiff)
}
pub fn webp(seed: u64, w: u32, h: u32) -> Vec<u8> {
    encode(seed, w, h, ImageFormat::WebP)
}

/// A little-endian TIFF with a camera maker tag (stands in for NEF, ARW, ...).
pub fn tiff_with_make(seed: u64, make: &str, dng: bool, payload: usize) -> Vec<u8> {
    let mut make_bytes = make.as_bytes().to_vec();
    make_bytes.push(0);
    let count: u16 = if dng { 6 } else { 5 };
    let ifd_end = 8 + 2 + count as usize * 12 + 4;
    let make_off = ifd_end;
    let strip_off = (make_off + make_bytes.len()).div_ceil(4) * 4;
    let mut out = b"II*\0".to_vec();
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    let mut entry = |tag: u16, ty: u16, n: u32, value: u32| {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    };
    entry(0x0100, 4, 1, 64);
    entry(0x0101, 4, 1, 48);
    entry(0x010F, 2, make_bytes.len() as u32, make_off as u32);
    entry(0x0111, 4, 1, strip_off as u32);
    entry(0x0117, 4, 1, payload as u32);
    if dng {
        entry(0xC612, 1, 4, u32::from_le_bytes([1, 4, 0, 0]));
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&make_bytes);
    out.resize(strip_off, 0);
    out.extend_from_slice(&dense(seed, payload));
    out
}

pub fn psd(seed: u64, w: u32, h: u32) -> Vec<u8> {
    let mut out = b"8BPS".to_vec();
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&h.to_be_bytes());
    out.extend_from_slice(&w.to_be_bytes());
    out.extend_from_slice(&8u16.to_be_bytes());
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&[0; 12]); // colour mode, resources, layers: all empty
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&dense(seed, (3 * w * h) as usize));
    out
}

pub fn raf(seed: u64) -> Vec<u8> {
    let mut out = b"FUJIFILMCCD-RAW ".to_vec();
    out.resize(160, 0);
    let (a, b, c) = (3000u32, 500u32, 6000u32);
    out[84..88].copy_from_slice(&160u32.to_be_bytes());
    out[88..92].copy_from_slice(&a.to_be_bytes());
    out[92..96].copy_from_slice(&(160 + a).to_be_bytes());
    out[96..100].copy_from_slice(&b.to_be_bytes());
    out[100..104].copy_from_slice(&(160 + a + b).to_be_bytes());
    out[104..108].copy_from_slice(&c.to_be_bytes());
    out.extend_from_slice(&dense(seed, (a + b + c) as usize));
    out
}

// ------------------------------------------------------------------------------------
// Audio and video
// ------------------------------------------------------------------------------------

pub fn wav(seed: u64, samples: usize) -> Vec<u8> {
    let data: Vec<u8> = (0..samples)
        .flat_map(|i| {
            let t = i as f32 / 22_050.0;
            let v = (t * (220.0 + (seed % 7) as f32 * 40.0) * std::f32::consts::TAU).sin() * 9000.0
                + (t * 3.0 * std::f32::consts::TAU).sin() * 3000.0;
            (v as i16).to_le_bytes()
        })
        .collect();
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&22_050u32.to_le_bytes());
    out.extend_from_slice(&44_100u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = ((8 + body.len()) as u32).to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    b.extend_from_slice(body);
    b
}

fn mp4_branded(seed: u64, brand: &[u8; 4], payload: usize) -> Vec<u8> {
    let mut ftyp = brand.to_vec();
    ftyp.extend_from_slice(&0x200u32.to_be_bytes());
    ftyp.extend_from_slice(brand);
    ftyp.extend_from_slice(b"mp42");
    let mut out = mp4_box(b"ftyp", &ftyp);
    out.extend_from_slice(&mp4_box(b"moov", &mp4_box(b"mvhd", &[0u8; 100])));
    out.extend_from_slice(&mp4_box(b"mdat", &dense(seed, payload)));
    out
}

pub fn mp4(seed: u64, payload: usize) -> Vec<u8> {
    mp4_branded(seed, b"isom", payload)
}
pub fn m4a(seed: u64, payload: usize) -> Vec<u8> {
    mp4_branded(seed, b"M4A ", payload)
}
pub fn three_gp(seed: u64, payload: usize) -> Vec<u8> {
    mp4_branded(seed, b"3gp5", payload)
}

/// QuickTime the old way: no type box, the movie header comes first.
pub fn mov(seed: u64, payload: usize) -> Vec<u8> {
    let mut out = mp4_box(b"moov", &mp4_box(b"mvhd", &[0u8; 100]));
    out.extend_from_slice(&mp4_box(b"mdat", &dense(seed, payload)));
    out
}

pub fn heic(seed: u64, payload: usize) -> Vec<u8> {
    let mut ftyp = b"heic".to_vec();
    ftyp.extend_from_slice(&0u32.to_be_bytes());
    ftyp.extend_from_slice(b"mif1heic");
    let mut out = mp4_box(b"ftyp", &ftyp);
    out.extend_from_slice(&mp4_box(b"meta", &[0u8; 300]));
    out.extend_from_slice(&mp4_box(b"mdat", &dense(seed, payload)));
    out
}

pub fn avi(seed: u64, payload: usize) -> Vec<u8> {
    let payload = payload & !1;
    let mut body = b"AVI LIST".to_vec();
    body.extend_from_slice(&68u32.to_le_bytes());
    body.extend_from_slice(b"hdrlavih");
    body.extend_from_slice(&56u32.to_le_bytes());
    body.extend_from_slice(&[0u8; 56]);
    body.extend_from_slice(b"LIST");
    body.extend_from_slice(&(4 + payload as u32).to_le_bytes());
    body.extend_from_slice(b"movi");
    body.extend_from_slice(&dense(seed, payload));
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

pub fn mkv(seed: u64, payload: usize, webm: bool) -> Vec<u8> {
    let doc: &[u8] = if webm { b"webm" } else { b"matroska" };
    let mut head = vec![0x42, 0x86, 0x81, 0x01, 0x42, 0x82, 0x80 | doc.len() as u8];
    head.extend_from_slice(doc);
    let mut out = vec![0x1A, 0x45, 0xDF, 0xA3, 0x80 | head.len() as u8];
    out.extend_from_slice(&head);
    let long = |n: usize| {
        let mut v = (n as u64).to_be_bytes();
        v[0] = 0x01;
        v
    };
    let mut cluster = vec![0x1F, 0x43, 0xB6, 0x75];
    cluster.extend_from_slice(&long(payload));
    cluster.extend_from_slice(&dense(seed, payload));
    out.extend_from_slice(&[0x18, 0x53, 0x80, 0x67]);
    out.extend_from_slice(&long(cluster.len()));
    out.extend_from_slice(&cluster);
    out
}

pub fn wmv(seed: u64, payload: usize, video: bool) -> Vec<u8> {
    use crate::carve::formats::ASF_HEADER;
    let file_props: [u8; 16] = [
        0xA1, 0xDC, 0xAB, 0x8C, 0x47, 0xA9, 0xCF, 0x11, 0x8E, 0xE4, 0x00, 0xC0, 0x0C, 0x20, 0x53,
        0x65,
    ];
    let stream_props: [u8; 16] = [
        0x91, 0x07, 0xDC, 0xB7, 0xB7, 0xA9, 0xCF, 0x11, 0x8E, 0xE6, 0x00, 0xC0, 0x0C, 0x20, 0x53,
        0x65,
    ];
    let video_guid: [u8; 16] = [
        0xC0, 0xEF, 0x19, 0xBC, 0x4D, 0x5B, 0xCF, 0x11, 0xA8, 0xFD, 0x00, 0x80, 0x5F, 0x5C, 0x44,
        0x2B,
    ];
    let header_size = 30 + 104 + 78;
    let total = header_size + 50 + payload;
    let mut out = ASF_HEADER.to_vec();
    out.extend_from_slice(&(header_size as u64).to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&[1, 2]);
    out.extend_from_slice(&file_props);
    out.extend_from_slice(&104u64.to_le_bytes());
    out.extend_from_slice(&[0x11; 16]);
    out.extend_from_slice(&(total as u64).to_le_bytes());
    out.extend_from_slice(&[0u8; 56]);
    out.extend_from_slice(&stream_props);
    out.extend_from_slice(&78u64.to_le_bytes());
    out.extend_from_slice(&if video { video_guid } else { [0x22; 16] });
    out.extend_from_slice(&[0u8; 38]);
    let mut data = vec![
        0x36, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE,
        0x6C,
    ];
    data.extend_from_slice(&(50 + payload as u64).to_le_bytes());
    data.extend_from_slice(&[0x11; 16]);
    data.extend_from_slice(&[0u8; 10]);
    data.extend_from_slice(&dense(seed, payload));
    out.extend_from_slice(&data);
    out
}

pub fn flv(seed: u64, tags: usize) -> Vec<u8> {
    let mut out = b"FLV\x01\x05".to_vec();
    out.extend_from_slice(&9u32.to_be_bytes());
    let mut prev = 0u32;
    for i in 0..tags {
        out.extend_from_slice(&prev.to_be_bytes());
        let size = 200 + (i as u32 % 5) * 40;
        out.push(if i % 3 == 0 { 8 } else { 9 });
        out.extend_from_slice(&size.to_be_bytes()[1..]);
        out.extend_from_slice(&[0, 0, i as u8, 0, 0, 0, 0]);
        out.extend_from_slice(&dense(seed + i as u64, size as usize));
        prev = 11 + size;
    }
    out.extend_from_slice(&prev.to_be_bytes());
    out
}

pub fn mpeg_ts(seed: u64, packets: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(packets * 188);
    for i in 0..packets {
        out.extend_from_slice(&[0x47, 0x41, 0x00, 0x10 | (i as u8 & 15)]);
        out.extend_from_slice(&dense(seed + i as u64, 184));
    }
    out
}

pub fn mp3(seed: u64, frames: usize, id3: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if id3 {
        let body = b"TIT2\0\0\0\x0b\0\0\0Invented\0\0";
        out.extend_from_slice(b"ID3\x03\0\0");
        out.extend_from_slice(&[0, 0, 0, body.len() as u8]);
        out.extend_from_slice(body);
    }
    for i in 0..frames {
        // MPEG-1 Layer III, 128 kbit/s, 44.1 kHz: 417 bytes without padding.
        out.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
        let mut body = dense(seed + i as u64, 413);
        // Keep the filler from looking like a frame header.
        for b in body.iter_mut() {
            if *b == 0xFF {
                *b = 0x7F;
            }
        }
        out.extend_from_slice(&body);
    }
    out
}

/// FLAC with a length that is a whole number of sectors (the carver has to
/// estimate where FLAC ends).
pub fn flac(seed: u64, sectors: usize) -> Vec<u8> {
    let mut out = b"fLaC".to_vec();
    out.extend_from_slice(&[0x80, 0, 0, 34]);
    let mut info = [0u8; 34];
    info[..2].copy_from_slice(&4096u16.to_be_bytes());
    info[2..4].copy_from_slice(&4096u16.to_be_bytes());
    info[10] = 0x0A;
    info[11] = 0xC4;
    info[12] = 0x42;
    info[13] = 0xF0;
    info[14..18].copy_from_slice(&441_000u32.to_be_bytes());
    out.extend_from_slice(&info);
    out.extend_from_slice(&[0xFF, 0xF8]);
    let rest = sectors * 512 - out.len();
    out.extend_from_slice(&dense(seed, rest));
    out
}

pub fn ogg(seed: u64, pages: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..pages {
        let payload = if i == 0 {
            let mut p = b"\x01vorbis".to_vec();
            p.extend_from_slice(&[0u8; 23]);
            p
        } else {
            dense(seed + i as u64, 900 + i * 37)
        };
        out.extend_from_slice(b"OggS\0");
        out.push(match i {
            0 => 2,
            i if i + 1 == pages => 4,
            _ => 0,
        });
        out.extend_from_slice(&(i as u64 * 4096).to_le_bytes());
        out.extend_from_slice(&0x1234_5678u32.to_le_bytes());
        out.extend_from_slice(&(i as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        let mut lacing = vec![255u8; payload.len() / 255];
        lacing.push((payload.len() % 255) as u8);
        out.push(lacing.len() as u8);
        out.extend_from_slice(&lacing);
        out.extend_from_slice(&payload);
    }
    out
}

pub fn midi(seed: u64) -> Vec<u8> {
    let mut out = b"MThd".to_vec();
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&[0, 1, 0, 2, 0, 96]);
    for t in 0..2u64 {
        let body = dense(seed + t, 300);
        out.extend_from_slice(b"MTrk");
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
    }
    out
}

pub fn aiff(seed: u64, payload: usize) -> Vec<u8> {
    let payload = payload & !1;
    let mut out = b"FORM".to_vec();
    out.extend_from_slice(&(4 + payload as u32).to_be_bytes());
    out.extend_from_slice(b"AIFF");
    out.extend_from_slice(&dense(seed, payload));
    out
}

// ------------------------------------------------------------------------------------
// Documents
// ------------------------------------------------------------------------------------

/// Invented filler text.
pub fn prose(seed: u64, words: usize) -> String {
    const WORDS: [&str; 24] = [
        "harbour", "lantern", "meadow", "quiet", "window", "paper", "river", "garden", "morning",
        "copper", "ticket", "orchard", "letter", "bridge", "silver", "market", "candle", "winter",
        "story", "island", "kettle", "ribbon", "summer", "pocket",
    ];
    let mut r = Rng(seed | 1);
    let mut s = String::new();
    for i in 0..words {
        if i > 0 {
            s.push(if i % 13 == 0 { '\n' } else { ' ' });
        }
        s.push_str(WORDS[r.below(24) as usize]);
    }
    s
}

pub fn txt(seed: u64, words: usize) -> Vec<u8> {
    prose(seed, words).into_bytes()
}

pub fn rtf(seed: u64) -> Vec<u8> {
    format!(
        "{{\\rtf1\\ansi\\deff0 {{\\fonttbl{{\\f0 Segoe UI;}}}}\\f0\\fs22 {}\\par A brace \\{{ in the text \\}} stays text.\\par }}",
        prose(seed, 120).replace('\n', "\\par ")
    )
    .into_bytes()
}

pub fn xml(seed: u64) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- invented -->\n<catalog>\n  <item id=\"1\">{}</item>\n  <item id=\"2\"><catalog>nested</catalog></item>\n</catalog>\n",
        prose(seed, 40)
    )
    .into_bytes()
}

pub fn svg(seed: u64) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\"><title>{}</title><circle cx=\"32\" cy=\"32\" r=\"20\"/></svg>\n",
        prose(seed, 6)
    )
    .into_bytes()
}

pub fn html(seed: u64) -> Vec<u8> {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head><title>Invented page</title></head>\n<body>\n<p>{}</p>\n</body>\n</html>\n",
        prose(seed, 80)
    )
    .into_bytes()
}

pub fn pdf(seed: u64) -> Vec<u8> {
    pdf_with(seed, 0)
}

/// A PDF with an embedded binary stream of `extra` bytes (a scanned page, say).
pub fn pdf_with(seed: u64, extra: usize) -> Vec<u8> {
    let stream = format!(
        "BT /F1 12 Tf 72 720 Td ({}) Tj ET",
        prose(seed, 60).replace('\n', " ")
    );
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
        format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        )
        .into_bytes(),
    ];
    if extra > 0 {
        let mut o =
            format!("<< /Type /XObject /Subtype /Image /Length {extra} >>\nstream\n").into_bytes();
        // Letters only: binary noise could happen to spell an end-of-file mark.
        o.extend(dense(seed, extra).into_iter().map(|b| b'A' + b % 26));
        o.extend_from_slice(b"\nendstream");
        objects.push(o);
    }
    let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A PDF that was saved again later: a second section follows the first end mark.
pub fn pdf_updated(seed: u64) -> Vec<u8> {
    let mut out = pdf(seed);
    let at = out.len();
    out.extend_from_slice(
        format!(
            "5 0 obj\n<< /Note ({}) >>\nendobj\nxref\n5 1\n{at:010} 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R /Prev 9 >>\nstartxref\n{}\n%%EOF\n",
            prose(seed + 1, 12).replace('\n', " "),
            at + 40
        )
        .as_bytes(),
    );
    out
}

/// A minimal OLE compound file with one stream of the given name.
pub fn ole(seed: u64, stream: &str, payload: usize) -> Vec<u8> {
    let sectors = payload.div_ceil(512).clamp(8, 120);
    let mut out = vec![0u8; 512];
    out[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    out[24..26].copy_from_slice(&0x3Eu16.to_le_bytes());
    out[26..28].copy_from_slice(&3u16.to_le_bytes());
    out[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
    out[30..32].copy_from_slice(&9u16.to_le_bytes());
    out[32..34].copy_from_slice(&6u16.to_le_bytes());
    out[44..48].copy_from_slice(&1u32.to_le_bytes());
    out[48..52].copy_from_slice(&1u32.to_le_bytes());
    out[56..60].copy_from_slice(&4096u32.to_le_bytes());
    out[60..64].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    out[68..72].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    out[76..80].copy_from_slice(&0u32.to_le_bytes());
    for i in 1..109 {
        out[76 + i * 4..80 + i * 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    }
    // Sector 0: the allocation table.
    let mut fat = vec![0xFFu8; 512];
    fat[..4].copy_from_slice(&0xFFFF_FFFDu32.to_le_bytes());
    fat[4..8].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    for i in 0..sectors {
        let next = if i + 1 == sectors {
            0xFFFF_FFFE
        } else {
            (3 + i) as u32
        };
        fat[(2 + i) * 4..(3 + i) * 4].copy_from_slice(&next.to_le_bytes());
    }
    out.extend_from_slice(&fat);
    // Sector 1: the directory.
    let mut dir = vec![0u8; 512];
    let mut entry = |slot: usize, name: &str, kind: u8, child: u32, start: u32, size: u64| {
        let e = &mut dir[slot * 128..(slot + 1) * 128];
        let units: Vec<u16> = name.encode_utf16().collect();
        for (i, u) in units.iter().enumerate() {
            e[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        e[64..66].copy_from_slice(&((units.len() as u16 + 1) * 2).to_le_bytes());
        e[66] = kind;
        e[67] = 1;
        e[68..72].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        e[72..76].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        e[76..80].copy_from_slice(&child.to_le_bytes());
        e[116..120].copy_from_slice(&start.to_le_bytes());
        e[120..128].copy_from_slice(&size.to_le_bytes());
    };
    entry(0, "Root Entry", 5, 1, 0xFFFF_FFFE, 0);
    entry(1, stream, 2, 0xFFFF_FFFF, 2, (sectors * 512) as u64);
    out.extend_from_slice(&dir);
    out.extend_from_slice(&dense(seed, sectors * 512));
    out
}

pub fn sqlite(seed: u64, pages: u32) -> Vec<u8> {
    let mut out = dense(seed, pages as usize * 4096);
    out[..16].copy_from_slice(b"SQLite format 3\0");
    out[16..18].copy_from_slice(&4096u16.to_be_bytes());
    out[18..24].copy_from_slice(&[1, 1, 0, 64, 32, 32]);
    out[24..28].copy_from_slice(&7u32.to_be_bytes());
    out[28..32].copy_from_slice(&pages.to_be_bytes());
    out[92..96].copy_from_slice(&7u32.to_be_bytes());
    out
}

pub fn pst(seed: u64, size: usize) -> Vec<u8> {
    let mut out = dense(seed, size);
    out[..4].copy_from_slice(b"!BDN");
    out[8..10].copy_from_slice(b"SM");
    out[10..12].copy_from_slice(&23u16.to_le_bytes());
    out[0xB8..0xC0].copy_from_slice(&(size as u64).to_le_bytes());
    out
}

// ------------------------------------------------------------------------------------
// Archives
// ------------------------------------------------------------------------------------

/// A ZIP archive. `streamed` writes the sizes after the data, as tools do
/// that cannot seek back.
pub fn zip(entries: &[(&str, Vec<u8>)], streamed: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let (method, packed): (u16, Vec<u8>) = if streamed {
            (8, miniz_oxide::deflate::compress_to_vec(data, 6))
        } else {
            (0, data.clone())
        };
        let flags: u16 = if streamed { 8 } else { 0 };
        let sizes = |out: &mut Vec<u8>, real: bool| {
            let (c, p, u) = if real {
                (crc, packed.len() as u32, data.len() as u32)
            } else {
                (0, 0, 0)
            };
            out.extend_from_slice(&c.to_le_bytes());
            out.extend_from_slice(&p.to_le_bytes());
            out.extend_from_slice(&u.to_le_bytes());
        };
        out.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0x21, 0]);
        sizes(&mut out, !streamed);
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&packed);
        if streamed {
            out.extend_from_slice(&0x0807_4B50u32.to_le_bytes());
            sizes(&mut out, true);
        }
        central.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&flags.to_le_bytes());
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&[0, 0, 0x21, 0]);
        sizes(&mut central, true);
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0u8; 12]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn office(seed: u64, part: &str) -> Vec<u8> {
    office_with(seed, part, 0)
}

/// The main part of a Word document: a heading and a few paragraphs.
fn word_body(seed: u64) -> Vec<u8> {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>",
    );
    for i in 0..6 {
        let words = if i == 0 { 5 } else { 45 };
        out.push_str(&format!(
            "<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
            prose(seed + 2 * i, words)
        ));
    }
    out.push_str("</w:body></w:document>");
    out.into_bytes()
}

fn office_with(seed: u64, part: &str, media: usize) -> Vec<u8> {
    let main = if part == "word/document.xml" {
        word_body(seed + 2)
    } else {
        xml(seed + 2)
    };
    let mut entries = vec![
        ("[Content_Types].xml", xml(seed)),
        ("_rels/.rels", xml(seed + 1)),
        (part, main),
        ("docProps/core.xml", xml(seed + 3)),
    ];
    if media > 0 {
        entries.push(("docProps/thumbnail.bin", dense(seed + 4, media)));
    }
    zip(&entries, false)
}

/// Office documents with `media` bytes of embedded pictures.
pub fn docx_with(seed: u64, media: usize) -> Vec<u8> {
    office_with(seed, "word/document.xml", media)
}
pub fn xlsx_with(seed: u64, media: usize) -> Vec<u8> {
    office_with(seed, "xl/workbook.xml", media)
}
pub fn pptx_with(seed: u64, media: usize) -> Vec<u8> {
    office_with(seed, "ppt/presentation.xml", media)
}

pub fn docx(seed: u64) -> Vec<u8> {
    office(seed, "word/document.xml")
}
pub fn xlsx(seed: u64) -> Vec<u8> {
    office(seed, "xl/workbook.xml")
}
pub fn pptx(seed: u64) -> Vec<u8> {
    office(seed, "ppt/presentation.xml")
}
pub fn odt(seed: u64) -> Vec<u8> {
    zip(
        &[
            (
                "mimetype",
                b"application/vnd.oasis.opendocument.text".to_vec(),
            ),
            ("content.xml", xml(seed)),
        ],
        false,
    )
}

fn rar_vint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return out;
        }
        out.push(b | 0x80);
    }
}

pub fn rar5(seed: u64, payload: usize) -> Vec<u8> {
    let mut out = b"Rar!\x1a\x07\x01\x00".to_vec();
    let mut block = |header: Vec<u8>, data: &[u8]| {
        let mut b = rar_vint(header.len() as u64);
        b.extend_from_slice(&header);
        out.extend_from_slice(&crc32(&b).to_le_bytes());
        out.extend_from_slice(&b);
        out.extend_from_slice(data);
    };
    block(vec![1, 0, 0], &[]);
    let mut file = vec![2, 2];
    file.extend_from_slice(&rar_vint(payload as u64));
    file.extend_from_slice(&[0]);
    file.extend_from_slice(&rar_vint(payload as u64));
    file.extend_from_slice(&[0x20, 0, 0, 8]);
    file.extend_from_slice(b"note.txt");
    block(file, &dense(seed, payload));
    block(vec![5, 0, 0], &[]);
    out
}

pub fn rar4(seed: u64, payload: usize) -> Vec<u8> {
    let mut out = b"Rar!\x1a\x07\x00".to_vec();
    out.extend_from_slice(&[0x90, 0xCF, 0x73, 0, 0, 13, 0, 0, 0, 0, 0, 0, 0]);
    let name = b"note.txt";
    let mut head = vec![0, 0, 0x74, 0x00, 0x80];
    head.extend_from_slice(&(32 + name.len() as u16).to_le_bytes());
    head.extend_from_slice(&(payload as u32).to_le_bytes());
    head.extend_from_slice(&(payload as u32).to_le_bytes());
    head.push(2);
    head.extend_from_slice(&[0u8; 8]);
    head.extend_from_slice(&[29, 0x30]);
    head.extend_from_slice(&(name.len() as u16).to_le_bytes());
    head.extend_from_slice(&0x20u32.to_le_bytes());
    head.extend_from_slice(name);
    out.extend_from_slice(&head);
    out.extend_from_slice(&dense(seed, payload));
    out.extend_from_slice(&[0xC4, 0x3D, 0x7B, 0x00, 0x40, 7, 0]);
    out
}

pub fn sevenz(seed: u64, payload: usize) -> Vec<u8> {
    let next = dense(seed + 1, 60);
    let mut start = Vec::new();
    start.extend_from_slice(&(payload as u64).to_le_bytes());
    start.extend_from_slice(&(next.len() as u64).to_le_bytes());
    start.extend_from_slice(&crc32(&next).to_le_bytes());
    let mut out = b"7z\xBC\xAF\x27\x1C\x00\x04".to_vec();
    out.extend_from_slice(&crc32(&start).to_le_bytes());
    out.extend_from_slice(&start);
    out.extend_from_slice(&dense(seed, payload));
    out.extend_from_slice(&next);
    out
}

pub fn gzip(data: &[u8], name: Option<&str>) -> Vec<u8> {
    let mut out = vec![
        0x1F,
        0x8B,
        8,
        if name.is_some() { 8 } else { 0 },
        0,
        0,
        0,
        0,
        0,
        3,
    ];
    if let Some(n) = name {
        out.extend_from_slice(n.as_bytes());
        out.push(0);
    }
    out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(data, 6));
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

pub fn xz(seed: u64, payload: usize) -> Vec<u8> {
    let payload = payload.div_ceil(4) * 4;
    let flags = [0u8, 4];
    let mut out = b"\xFD7zXZ\0".to_vec();
    out.extend_from_slice(&flags);
    out.extend_from_slice(&crc32(&flags).to_le_bytes());
    out.extend_from_slice(&dense(seed, payload));
    let mut tail = 3u32.to_le_bytes().to_vec();
    tail.extend_from_slice(&flags);
    out.extend_from_slice(&crc32(&tail).to_le_bytes());
    out.extend_from_slice(&tail);
    out.extend_from_slice(b"YZ");
    out
}

pub fn tar(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, data) in entries {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(b"0000644\0");
        h[108..116].copy_from_slice(b"0001750\0");
        h[116..124].copy_from_slice(b"0001750\0");
        h[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        h[136..148].copy_from_slice(b"14700000000\0");
        h[148..156].fill(b' ');
        h[156] = b'0';
        h[257..265].copy_from_slice(b"ustar\x0000");
        let sum: u32 = h.iter().map(|b| *b as u32).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.resize(out.len() + 1024, 0);
    out
}

pub fn cab(seed: u64, size: usize) -> Vec<u8> {
    let mut out = dense(seed, size);
    out[..4].copy_from_slice(b"MSCF");
    out[4..8].fill(0);
    out[8..12].copy_from_slice(&(size as u32).to_le_bytes());
    out[12..16].fill(0);
    out[16..20].copy_from_slice(&44u32.to_le_bytes());
    out[20..24].fill(0);
    out[24] = 3;
    out[25] = 1;
    out
}

/// A 64-bit Windows program image with one section.
pub fn exe(seed: u64, code: usize, dll: bool) -> Vec<u8> {
    let mut out = vec![0u8; 0x200];
    out[..2].copy_from_slice(b"MZ");
    out[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    out[0x80..0x84].copy_from_slice(b"PE\0\0");
    out[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
    out[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
    out[0x94..0x96].copy_from_slice(&240u16.to_le_bytes());
    out[0x96..0x98].copy_from_slice(&(if dll { 0x2022u16 } else { 0x0022 }).to_le_bytes());
    let opt = 0x98;
    out[opt..opt + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
    out[opt + 60..opt + 64].copy_from_slice(&0x200u32.to_le_bytes());
    out[opt + 68..opt + 70].copy_from_slice(&3u16.to_le_bytes());
    let sec = opt + 240;
    out[sec..sec + 8].copy_from_slice(b".text\0\0\0");
    out[sec + 16..sec + 20].copy_from_slice(&(code as u32).to_le_bytes());
    out[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes());
    out.extend_from_slice(&dense(seed, code));
    out
}
