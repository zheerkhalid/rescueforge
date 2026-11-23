//! The data model shared by the scanners, the session file and the window.

use serde::{Deserialize, Serialize};

/// The broad groups used by the type filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    Pictures,
    Video,
    Audio,
    Documents,
    Archives,
    Other,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Pictures,
        Category::Video,
        Category::Audio,
        Category::Documents,
        Category::Archives,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Pictures => "Pictures",
            Category::Video => "Video",
            Category::Audio => "Audio",
            Category::Documents => "Documents",
            Category::Archives => "Archives",
            Category::Other => "Other",
        }
    }

    pub fn index(self) -> usize {
        Category::ALL.iter().position(|c| *c == self).unwrap_or(5)
    }

    /// The group a lower-case extension belongs to.
    pub fn from_ext(ext: &str) -> Category {
        match ext {
            "jpg" | "jpeg" | "jpe" | "png" | "gif" | "bmp" | "tif" | "tiff" | "webp" | "heic"
            | "heif" | "avif" | "psd" | "psb" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "orf"
            | "rw2" | "pef" | "srw" | "raf" | "raw" | "ico" | "svg" | "ai" | "eps" | "jfif"
            | "djvu" | "xcf" | "jp2" | "j2k" | "emf" | "cur" => Category::Pictures,
            "mp4" | "mov" | "m4v" | "3gp" | "3g2" | "avi" | "mkv" | "webm" | "wmv" | "asf"
            | "flv" | "mpg" | "mpeg" | "ts" | "m2ts" | "mts" | "ogv" | "vob" => Category::Video,
            "mp3" | "wav" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "wma" | "aac" | "aif"
            | "aiff" | "mid" | "midi" | "amr" | "ape" | "dsf" | "wv" | "au" => Category::Audio,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "rtf" | "txt" | "xml"
            | "html" | "htm" | "odt" | "ods" | "odp" | "csv" | "md" | "epub" | "msg" | "vsd"
            | "vsdx" | "pst" | "ost" | "json" | "log" | "ole" | "pub" | "one" | "mdb" | "accdb"
            | "eml" | "plist" | "kml" | "gpx" | "vcf" | "ics" => Category::Documents,
            "zip" | "rar" | "7z" | "gz" | "tgz" | "tar" | "xz" | "bz2" | "cab" | "jar" | "apk"
            | "iso" | "img" | "vhd" | "vhdx" | "zst" | "lz4" | "deb" => Category::Archives,
            _ => Category::Other,
        }
    }
}

/// How likely it is that the recovered bytes are the original file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Chance {
    /// None of the file's clusters have been reused.
    Good,
    /// Some of the clusters now belong to another file, or the end is uncertain.
    Partial,
    /// The start of the file (or all of it) has been reused, or nothing points to its data.
    Poor,
    /// The file system gave no way to tell.
    Unknown,
}

impl Chance {
    pub fn label(self) -> &'static str {
        match self {
            Chance::Good => "Good",
            Chance::Partial => "Partly overwritten",
            Chance::Poor => "Overwritten",
            Chance::Unknown => "Unknown",
        }
    }

    pub fn explain(self) -> &'static str {
        match self {
            Chance::Good => "The space this file used has not been given to another file. It should come back intact.",
            Chance::Partial => "Part of the space this file used now belongs to something else, or its end could not be confirmed. It may open with damage.",
            Chance::Poor => "The space this file used has been reused. The recovered file will probably not open.",
            Chance::Unknown => "The file system gives no information about whether this space has been reused.",
        }
    }
}

/// Where the entry came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Origin {
    /// A record of the NTFS master file table.
    Ntfs,
    /// A FAT12/16/32 directory entry.
    Fat,
    /// An exFAT directory entry set.
    ExFat,
    /// Found by its content signature; the name and folder are not known.
    Carved,
    /// A file record left over from an earlier NTFS (for example before a quick format).
    OrphanNtfs,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Ntfs => "NTFS file record",
            Origin::Fat => "FAT directory entry",
            Origin::ExFat => "exFAT directory entry",
            Origin::Carved => "Content signature (deep scan)",
            Origin::OrphanNtfs => "File record of an earlier NTFS",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Status {
    /// Deleted: the file system no longer lists it.
    Deleted,
    /// Still listed by a working file system.
    Existing,
    /// Belongs to a lost partition or an earlier file system.
    Lost,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Deleted => "Deleted",
            Status::Existing => "Existing",
            Status::Lost => "Lost",
        }
    }
}

/// A run of bytes on the scanned source. Offsets are absolute (from the start
/// of the drive, disk or image that was opened).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extent {
    #[serde(rename = "o")]
    pub offset: u64,
    #[serde(rename = "l")]
    pub len: u64,
}

impl Extent {
    /// A hole: reads as zeros.
    pub const SPARSE: u64 = u64::MAX;

    pub fn is_sparse(&self) -> bool {
        self.offset == Self::SPARSE
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileData {
    /// The content was stored inside the file record itself (small NTFS files).
    #[serde(rename = "r", with = "hex_bytes")]
    Resident(Vec<u8>),
    #[serde(rename = "x")]
    Extents(Vec<Extent>),
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::util::to_hex(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        crate::util::from_hex(&s).ok_or_else(|| serde::de::Error::custom("bad hex"))
    }
}

/// One recoverable file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundFile {
    #[serde(rename = "n")]
    pub name: String,
    /// Folder, with `/` between the parts and no leading or trailing slash.
    #[serde(rename = "p")]
    pub path: String,
    #[serde(rename = "s")]
    pub size: u64,
    /// Unix seconds (UTC), 0 when unknown.
    #[serde(rename = "m", default)]
    pub modified: i64,
    #[serde(rename = "c", default)]
    pub created: i64,
    #[serde(rename = "st")]
    pub status: Status,
    #[serde(rename = "or")]
    pub origin: Origin,
    #[serde(rename = "ch")]
    pub chance: Chance,
    #[serde(rename = "d")]
    pub data: FileData,
    /// A short remark shown in the details panel ("" when there is none).
    #[serde(rename = "no", default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl FoundFile {
    pub fn ext(&self) -> String {
        crate::util::ext_of(&self.name)
    }

    pub fn category(&self) -> Category {
        Category::from_ext(&self.ext())
    }

    /// The absolute offset of the first byte of content, if it lies on the source.
    pub fn first_offset(&self) -> Option<u64> {
        match &self.data {
            FileData::Extents(e) => e.first().filter(|x| !x.is_sparse()).map(|x| x.offset),
            FileData::Resident(_) => None,
        }
    }
}

/// What was scanned. Stored in session files so that the source can be reopened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceSpec {
    /// A mounted volume, for example `D:`.
    Volume { letter: String, guid: String },
    /// A whole physical disk.
    Disk { number: u32 },
    /// A disk image file.
    Image { path: String },
    /// The invented in-memory disk of the demo mode.
    Demo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories() {
        assert_eq!(Category::from_ext("jpg"), Category::Pictures);
        assert_eq!(Category::from_ext("nef"), Category::Pictures);
        assert_eq!(Category::from_ext("mkv"), Category::Video);
        assert_eq!(Category::from_ext("flac"), Category::Audio);
        assert_eq!(Category::from_ext("xlsx"), Category::Documents);
        assert_eq!(Category::from_ext("7z"), Category::Archives);
        assert_eq!(Category::from_ext("exe"), Category::Other);
        assert_eq!(Category::from_ext("jp2"), Category::Pictures);
        assert_eq!(Category::from_ext("cur"), Category::Pictures);
        assert_eq!(Category::from_ext("wv"), Category::Audio);
        assert_eq!(Category::from_ext("mpg"), Category::Video);
        assert_eq!(Category::from_ext("vcf"), Category::Documents);
        assert_eq!(Category::from_ext("ics"), Category::Documents);
        assert_eq!(Category::from_ext("zst"), Category::Archives);
        assert_eq!(Category::from_ext("deb"), Category::Archives);
        for other in [
            "ttf", "woff2", "swf", "a", "hve", "evtx", "elf", "pcap", "lnk",
        ] {
            assert_eq!(Category::from_ext(other), Category::Other, "{other}");
        }
        assert_eq!(Category::from_ext(""), Category::Other);
        for (i, c) in Category::ALL.iter().enumerate() {
            assert_eq!(c.index(), i);
        }
    }

    #[test]
    fn found_file_round_trips_through_json() {
        let f = FoundFile {
            name: "note.txt".into(),
            path: "Docs/Old".into(),
            size: 5,
            modified: 1_700_000_000,
            created: 0,
            status: Status::Deleted,
            origin: Origin::Ntfs,
            chance: Chance::Good,
            data: FileData::Resident(b"hello".to_vec()),
            note: String::new(),
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("68656c6c6f"), "{json}");
        assert_eq!(serde_json::from_str::<FoundFile>(&json).unwrap(), f);
        let g = FoundFile {
            data: FileData::Extents(vec![
                Extent {
                    offset: 4096,
                    len: 8192,
                },
                Extent {
                    offset: Extent::SPARSE,
                    len: 4096,
                },
            ]),
            ..f
        };
        let json = serde_json::to_string(&g).unwrap();
        let back: FoundFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
        assert_eq!(back.first_offset(), Some(4096));
        assert_eq!(back.category(), Category::Documents);
    }
}
