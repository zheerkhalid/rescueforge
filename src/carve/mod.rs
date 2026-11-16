//! Signature carving: recognising files by their content when no file
//! system record points at them any more.
//!
//! [`Carver::identify`] looks at the first sector of a candidate and, when a
//! known signature matches, hands over to a format reader in [`formats`]
//! that walks the real structure of the file to find where it ends.

pub mod formats;

use crate::model::Category;
use crate::source::Source;
use std::collections::HashSet;

/// A file recognised at some offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub size: u64,
    pub ext: &'static str,
    /// The structure broke off before its proper end (fragmented or overwritten).
    pub partial: bool,
    /// The end was estimated, not read from the format.
    pub estimated: bool,
}

pub(crate) fn hit(size: u64, ext: &'static str) -> Option<Hit> {
    Some(Hit {
        size,
        ext,
        partial: false,
        estimated: false,
    })
}

pub(crate) fn partial(size: u64, ext: &'static str) -> Option<Hit> {
    Some(Hit {
        size,
        ext,
        partial: true,
        estimated: false,
    })
}

/// One line of the "file types to look for" list in Settings.
pub struct TypeInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub cat: Category,
    pub exts: &'static [&'static str],
    pub default_on: bool,
}

const fn ty(
    id: &'static str,
    label: &'static str,
    cat: Category,
    exts: &'static [&'static str],
) -> TypeInfo {
    TypeInfo {
        id,
        label,
        cat,
        exts,
        default_on: true,
    }
}

pub const TYPES: &[TypeInfo] = &[
    ty("jpg", "JPEG", Category::Pictures, &["jpg"]),
    ty("png", "PNG", Category::Pictures, &["png"]),
    ty("gif", "GIF", Category::Pictures, &["gif"]),
    ty("bmp", "BMP", Category::Pictures, &["bmp"]),
    ty("tif", "TIFF", Category::Pictures, &["tif"]),
    ty("webp", "WebP", Category::Pictures, &["webp"]),
    ty("heic", "HEIC / AVIF", Category::Pictures, &["heic", "avif"]),
    ty(
        "psd",
        "Photoshop (PSD, PSB)",
        Category::Pictures,
        &["psd", "psb"],
    ),
    ty(
        "raw",
        "Camera RAW (CR2, CR3, NEF, ARW, DNG, ORF, RW2, PEF, SRW, RAF)",
        Category::Pictures,
        &[
            "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "pef", "srw", "raf",
        ],
    ),
    ty(
        "mp4",
        "MP4 / MOV / M4V / 3GP",
        Category::Video,
        &["mp4", "mov", "m4v", "3gp"],
    ),
    ty("avi", "AVI", Category::Video, &["avi"]),
    ty("mkv", "MKV / WebM", Category::Video, &["mkv", "webm"]),
    ty("wmv", "WMV / ASF", Category::Video, &["wmv"]),
    ty("flv", "FLV", Category::Video, &["flv"]),
    ty(
        "ts",
        "MPEG transport stream (TS, M2TS)",
        Category::Video,
        &["ts", "m2ts"],
    ),
    ty("ogv", "Ogg video", Category::Video, &["ogv"]),
    ty("mp3", "MP3", Category::Audio, &["mp3"]),
    ty("wav", "WAV", Category::Audio, &["wav"]),
    ty("flac", "FLAC", Category::Audio, &["flac"]),
    ty(
        "ogg",
        "OGG / Opus",
        Category::Audio,
        &["ogg", "opus", "oga"],
    ),
    ty("m4a", "M4A", Category::Audio, &["m4a"]),
    ty("wma", "WMA", Category::Audio, &["wma"]),
    ty("mid", "MIDI", Category::Audio, &["mid"]),
    ty("aif", "AIFF", Category::Audio, &["aif"]),
    ty("pdf", "PDF", Category::Documents, &["pdf"]),
    ty(
        "docx",
        "Word, Excel, PowerPoint (DOCX, XLSX, PPTX)",
        Category::Documents,
        &["docx", "xlsx", "pptx", "vsdx"],
    ),
    ty(
        "doc",
        "Word, Excel, PowerPoint 97-2003 (DOC, XLS, PPT, MSG)",
        Category::Documents,
        &["doc", "xls", "ppt", "msg", "vsd", "pub", "ole"],
    ),
    ty(
        "odf",
        "OpenDocument and EPUB",
        Category::Documents,
        &["odt", "ods", "odp", "epub"],
    ),
    ty("rtf", "RTF", Category::Documents, &["rtf"]),
    ty(
        "pst",
        "Outlook data (PST, OST)",
        Category::Documents,
        &["pst", "ost"],
    ),
    ty(
        "xml",
        "XML (also KML, GPX)",
        Category::Documents,
        &["xml", "plist", "kml", "gpx"],
    ),
    ty("svg", "SVG", Category::Pictures, &["svg"]),
    ty("html", "HTML", Category::Documents, &["html"]),
    TypeInfo {
        id: "txt",
        label: "Plain text (many false finds)",
        cat: Category::Documents,
        exts: &["txt"],
        default_on: false,
    },
    ty(
        "zip",
        "ZIP (also JAR, APK)",
        Category::Archives,
        &["zip", "jar", "apk"],
    ),
    ty("rar", "RAR", Category::Archives, &["rar"]),
    ty("7z", "7z", Category::Archives, &["7z"]),
    ty("gz", "GZIP (GZ, TGZ)", Category::Archives, &["gz", "tgz"]),
    ty("xz", "XZ", Category::Archives, &["xz"]),
    ty("tar", "TAR", Category::Archives, &["tar"]),
    ty("cab", "Cabinet (CAB)", Category::Archives, &["cab"]),
    ty(
        "exe",
        "Programs (EXE, DLL, SYS, MSI)",
        Category::Other,
        &["exe", "dll", "sys", "msi"],
    ),
    ty("sqlite", "SQLite databases", Category::Other, &["sqlite"]),
];

/// The ids that are switched off unless the user enables them.
pub fn default_off() -> Vec<String> {
    TYPES
        .iter()
        .filter(|t| !t.default_on)
        .map(|t| t.id.to_string())
        .collect()
}

/// A bounds-checked, buffered view of the source starting at a candidate file.
pub struct Win<'a> {
    src: &'a Source,
    base: u64,
    limit: u64,
    buf: Vec<u8>,
    pos: u64,
}

const BLOCK: usize = 64 * 1024;

impl<'a> Win<'a> {
    pub fn new(src: &'a Source, base: u64) -> Win<'a> {
        Win {
            src,
            base,
            limit: src.len().saturating_sub(base),
            buf: Vec::new(),
            pos: 0,
        }
    }

    /// How many bytes exist from the start of the candidate to the end of the source.
    pub fn limit(&self) -> u64 {
        self.limit
    }

    fn fill(&mut self, off: u64, want: usize) {
        let want = (want.max(BLOCK) as u64).min(self.limit - off) as usize;
        self.buf.resize(want, 0);
        self.src.read_at(self.base + off, &mut self.buf);
        self.pos = off;
    }

    /// Exactly `len` bytes at `off`, or `None` past the end of the source.
    pub fn get(&mut self, off: u64, len: usize) -> Option<&[u8]> {
        let end = off.checked_add(len as u64)?;
        if end > self.limit || len > 16 << 20 {
            return None;
        }
        if off < self.pos || end > self.pos + self.buf.len() as u64 {
            self.fill(off, len);
        }
        let s = (off - self.pos) as usize;
        Some(&self.buf[s..s + len])
    }

    /// Up to `max` bytes at `off` (fewer at the end of the source).
    pub fn some(&mut self, off: u64, max: usize) -> &[u8] {
        if off >= self.limit {
            return &[];
        }
        let len = (max as u64).min(self.limit - off) as usize;
        self.get(off, len).unwrap_or(&[])
    }

    pub fn u8(&mut self, off: u64) -> Option<u8> {
        self.get(off, 1).map(|b| b[0])
    }
    pub fn le16(&mut self, off: u64) -> Option<u16> {
        self.get(off, 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    pub fn le32(&mut self, off: u64) -> Option<u32> {
        self.get(off, 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn le64(&mut self, off: u64) -> Option<u64> {
        self.get(off, 8).and_then(|b| crate::util::le64(b, 0))
    }
    pub fn be16(&mut self, off: u64) -> Option<u16> {
        self.get(off, 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }
    pub fn be32(&mut self, off: u64) -> Option<u32> {
        self.get(off, 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn be64(&mut self, off: u64) -> Option<u64> {
        self.get(off, 8).and_then(|b| crate::util::be64(b, 0))
    }
}

/// Bytes that may appear in a text file.
pub(crate) fn is_text(b: u8) -> bool {
    matches!(b, 0x09 | 0x0A | 0x0C | 0x0D | 0x20..=0x7E | 0x80..=0xFF)
}

/// Does this sector start with one of the unmistakable signatures? Used to
/// find the end of formats that do not record their own length.
pub fn strong_magic(h: &[u8]) -> bool {
    if h.len() < 16 {
        return false;
    }
    h.starts_with(&[0xFF, 0xD8, 0xFF])
        || h.starts_with(b"\x89PNG\r\n\x1a\n")
        || h.starts_with(b"GIF8")
        || h.starts_with(b"%PDF-")
        || h.starts_with(b"PK\x03\x04")
        || h.starts_with(b"Rar!\x1a\x07")
        || h.starts_with(b"7z\xBC\xAF\x27\x1C")
        || h.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1])
        || h.starts_with(b"RIFF")
        || h.starts_with(b"fLaC")
        || h.starts_with(b"OggS")
        || h.starts_with(b"ID3")
        || h.starts_with(&[0x1A, 0x45, 0xDF, 0xA3])
        || h.starts_with(b"SQLite format 3\0")
        || h.starts_with(b"FILE0")
        || &h[4..8] == b"ftyp"
        || (h.starts_with(b"MZ") && h.len() >= 64)
}

/// Recognises files by content.
pub struct Carver {
    on: HashSet<&'static str>,
}

impl Carver {
    /// `disabled`: type ids (see [`TYPES`]) that should not be looked for.
    pub fn new(disabled: &[String]) -> Carver {
        let mut on = HashSet::new();
        for t in TYPES {
            if !disabled.iter().any(|d| d == t.id) {
                on.extend(t.exts.iter().copied());
            }
        }
        Carver { on }
    }

    fn any(&self, exts: &[&str]) -> bool {
        exts.iter().any(|e| self.on.contains(e))
    }

    pub fn wants_text(&self) -> bool {
        self.on.contains("txt")
    }

    /// Is a file of a known type starting at `offset`? `head` holds the first
    /// sector (at least 512 bytes unless the source ends earlier);
    /// `after_text` says whether the previous sector ended in text.
    pub fn identify(
        &self,
        src: &Source,
        offset: u64,
        head: &[u8],
        after_text: bool,
    ) -> Option<Hit> {
        if head.len() < 32 {
            return None;
        }
        let mut w = Win::new(src, offset);
        let w = &mut w;
        use formats as f;
        let found = match head[0] {
            0xFF if head[1] == 0xD8 && head[2] == 0xFF && self.any(&["jpg"]) => f::jpeg(w),
            0xFF if head[1] & 0xE0 == 0xE0 && self.any(&["mp3"]) => f::mp3(w),
            0x89 if head.starts_with(b"\x89PNG\r\n\x1a\n") && self.any(&["png"]) => f::png(w),
            b'G' if head.starts_with(b"GIF8") && self.any(&["gif"]) => f::gif(w),
            b'B' if head[1] == b'M' && self.any(&["bmp"]) => f::bmp(w),
            b'I' if head.starts_with(b"ID3") && self.any(&["mp3"]) => f::mp3(w),
            b'I' | b'M'
                if (head.starts_with(b"II*\0")
                    || head.starts_with(b"MM\0*")
                    || head.starts_with(b"IIRO")
                    || head.starts_with(b"IIRS")
                    || head.starts_with(b"IIU\0"))
                    && self.any(&[
                        "tif", "cr2", "nef", "arw", "dng", "orf", "rw2", "pef", "srw",
                    ]) =>
            {
                f::tiff(w)
            }
            b'M' if head[1] == b'Z' && self.any(&["exe", "dll", "sys"]) => f::pe(w),
            b'M' if head.starts_with(b"MThd") && self.any(&["mid"]) => f::midi(w),
            b'M' if head.starts_with(b"MSCF") && self.any(&["cab"]) => f::cab(w),
            b'R' if head.starts_with(b"RIFF") && self.any(&["avi", "wav", "webp"]) => f::riff(w),
            b'R' if head.starts_with(b"Rar!\x1a\x07") && self.any(&["rar"]) => f::rar(w),
            b'8' if head.starts_with(b"8BPS") && self.any(&["psd", "psb"]) => f::psd(w),
            0x1A if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) && self.any(&["mkv", "webm"]) => {
                f::mkv(w)
            }
            0x30 if head.starts_with(&f::ASF_HEADER) && self.any(&["wmv", "wma"]) => f::asf(w),
            b'f' if head.starts_with(b"fLaC") && self.any(&["flac"]) => f::flac(w),
            b'O' if head.starts_with(b"OggS") && self.any(&["ogg", "opus", "oga", "ogv"]) => {
                f::ogg(w)
            }
            b'%' if head.starts_with(b"%PDF-") && self.any(&["pdf"]) => f::pdf(w),
            0xD0 if head.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1])
                && self.any(&["doc", "xls", "ppt", "msg", "vsd", "pub", "ole", "msi"]) =>
            {
                f::ole(w)
            }
            b'P' if head.starts_with(b"PK\x03\x04") => f::zip(w),
            b'7' if head.starts_with(b"7z\xBC\xAF\x27\x1C") && self.any(&["7z"]) => f::sevenz(w),
            0x1F if head[1] == 0x8B && head[2] == 8 && self.any(&["gz", "tgz"]) => f::gzip(w),
            0xFD if head.starts_with(b"\xFD7zXZ\0") && self.any(&["xz"]) => f::xz(w),
            b'S' if head.starts_with(b"SQLite format 3\0") && self.any(&["sqlite"]) => f::sqlite(w),
            b'!' if head.starts_with(b"!BDN") && self.any(&["pst", "ost"]) => f::pst(w),
            b'{' if head.starts_with(b"{\\rtf1") && self.any(&["rtf"]) => f::rtf(w),
            b'F' if head.starts_with(b"FORM") && self.any(&["aif"]) => f::aiff(w),
            b'F' if head.starts_with(b"FUJIFILMCCD-RAW") && self.any(&["raf"]) => f::raf(w),
            b'F' if head.starts_with(b"FLV\x01") && self.any(&["flv"]) => f::flv(w),
            b'<' if self.any(&["xml", "svg", "plist", "kml", "gpx", "html"]) => f::markup(w),
            _ => None,
        };
        let found = found
            .or_else(|| {
                let t = &head[4..8];
                let boxed = t == b"ftyp"
                    || t == b"moov"
                    || t == b"mdat"
                    || (t == b"wide" && head[..4] == [0, 0, 0, 8]);
                if boxed && self.any(&["mp4", "mov", "m4v", "3gp", "m4a", "heic", "avif", "cr3"]) {
                    f::mp4(w)
                } else {
                    None
                }
            })
            .or_else(|| {
                if head.len() >= 512 && &head[257..262] == b"ustar" && self.any(&["tar"]) {
                    f::tar(w)
                } else if head.len() >= 512
                    && self.any(&["ts", "m2ts"])
                    && ((head[0] == 0x47 && head[188] == 0x47 && head[376] == 0x47)
                        || (head[4] == 0x47 && head[196] == 0x47 && head[388] == 0x47))
                {
                    f::mpeg_ts(w)
                } else {
                    None
                }
            })
            .or_else(|| {
                if self.wants_text() && !after_text && head.len() >= 512 {
                    f::text(w)
                } else {
                    None
                }
            })?;
        if found.size < 64 || found.size > w.limit() || !self.on.contains(found.ext) {
            return None;
        }
        Some(found)
    }
}

#[cfg(test)]
mod format_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_is_bounds_checked() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
        let src = Source::from_bytes(data.clone());
        let mut w = Win::new(&src, 1000);
        assert_eq!(w.limit(), 199_000);
        assert_eq!(w.get(0, 4).unwrap(), &data[1000..1004]);
        assert_eq!(w.get(150_000, 10).unwrap(), &data[151_000..151_010]);
        assert_eq!(w.get(5, 3).unwrap(), &data[1005..1008], "backward seek");
        assert!(w.get(198_999, 2).is_none());
        assert!(w.get(u64::MAX, 2).is_none());
        assert_eq!(w.some(198_990, 100).len(), 10);
        assert!(w.some(199_000, 100).is_empty());
        assert_eq!(
            w.le32(0),
            Some(u32::from_le_bytes([
                data[1000], data[1001], data[1002], data[1003]
            ]))
        );
        assert_eq!(
            w.be16(198_998),
            Some(u16::from_be_bytes([data[199_998], data[199_999]]))
        );
        assert_eq!(w.be64(198_995), None);
        let mut past = Win::new(&src, 500_000);
        assert_eq!(past.limit(), 0);
        assert!(past.get(0, 1).is_none());
    }

    #[test]
    fn type_table_is_consistent() {
        let mut ids = HashSet::new();
        let mut exts = HashSet::new();
        for t in TYPES {
            assert!(ids.insert(t.id), "duplicate id {}", t.id);
            for e in t.exts {
                assert!(exts.insert(*e), "extension {e} listed twice");
                assert_eq!(
                    Category::from_ext(e),
                    t.cat,
                    "category of .{e} matches its group"
                );
            }
        }
        assert_eq!(default_off(), vec!["txt".to_string()]);
        let c = Carver::new(&["jpg".to_string()]);
        assert!(!c.any(&["jpg"]));
        assert!(c.any(&["png"]));
        assert!(Carver::new(&[]).wants_text());
        assert!(!Carver::new(&default_off()).wants_text());
    }
}
