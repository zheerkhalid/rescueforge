//! Saving a scan to a file and opening it again later, so that a long scan
//! does not have to be repeated.
//!
//! File layout: the line `RESCUEFORGE-SESSION 1`, then the session as
//! zlib-compressed JSON.

use crate::model::{FoundFile, SourceSpec};
use crate::scan::{ResumePoint, ScanOptions};
use serde::{Deserialize, Serialize};
use std::path::Path;

const MAGIC: &[u8] = b"RESCUEFORGE-SESSION 1\n";
/// Largest session that will be unpacked (guards against a hostile file).
const MAX_JSON: usize = 3 << 30;

pub const EXTENSION: &str = "rfsession";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// Unix seconds.
    pub created: i64,
    pub source: SourceSpec,
    /// How the source was shown in the window ("Media (D:)").
    pub source_label: String,
    pub source_len: u64,
    pub options: ScanOptions,
    /// False when the scan was stopped early or still running when saved.
    pub finished: bool,
    pub files: Vec<FoundFile>,
    pub log: Vec<String>,
    /// Where a deep scan that was stopped early can be continued.
    #[serde(default)]
    pub resume: Option<ResumePoint>,
}

impl Session {
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json =
            serde_json::to_vec(self).map_err(|e| format!("Could not pack the session: {e}"))?;
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(&json, 6));
        // Write to a temporary name first so that a failed save never leaves
        // a half-written session behind.
        let tmp = path.with_extension("rfsession-part");
        std::fs::write(&tmp, &out)
            .and_then(|_| std::fs::rename(&tmp, path))
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("Could not save \"{}\": {e}", path.display())
            })
    }

    pub fn load(path: &Path) -> Result<Session, String> {
        let raw = std::fs::read(path)
            .map_err(|e| format!("Could not open \"{}\": {e}", path.display()))?;
        let Some(packed) = raw.strip_prefix(MAGIC) else {
            return Err(format!(
                "\"{}\" is not a RescueForge session file (or it was made by a newer version).",
                path.display()
            ));
        };
        let json = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(packed, MAX_JSON)
            .map_err(|_| "The session file is damaged and cannot be unpacked.".to_string())?;
        serde_json::from_slice(&json).map_err(|e| format!("The session file is damaged: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Chance, Extent, FileData, Origin, Status};
    use crate::util::Rng;

    fn sample() -> Session {
        Session {
            created: 1_760_000_000,
            source: SourceSpec::Image {
                path: "E:\\images\\card.img".into(),
            },
            source_label: "card.img".into(),
            source_len: 1 << 30,
            options: ScanOptions {
                deep: true,
                ..Default::default()
            },
            finished: true,
            files: (0..500)
                .map(|i| FoundFile {
                    name: format!("file-{i}.jpg"),
                    path: format!("Photos/Set {}", i % 7),
                    size: 1000 + i,
                    modified: 1_700_000_000 + i as i64,
                    created: 0,
                    status: Status::Deleted,
                    origin: Origin::Ntfs,
                    chance: Chance::Good,
                    data: if i % 5 == 0 {
                        FileData::Resident(vec![i as u8; 20])
                    } else {
                        FileData::Extents(vec![Extent {
                            offset: i * 4096,
                            len: 1000 + i,
                        }])
                    },
                    note: String::new(),
                })
                .collect(),
            log: vec!["This volume: NTFS file system, 500 entries listed.".into()],
            resume: Some(ResumePoint {
                pos: 256 << 20,
                skip_until: 300 << 20,
                lost: vec![1 << 20],
                listed: 500,
            }),
        }
    }

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("rescueforge-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scan.rfsession");
        let s = sample();
        s.save(&path).unwrap();
        assert!(!dir.join("scan.rfsession-part").exists());
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(size < 40_000, "compressed: {size} bytes");
        assert_eq!(Session::load(&path).unwrap(), s);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sessions_of_the_first_version_still_open() {
        // What 1.0 wrote: no "resume" at all, and scan options without it.
        let json = br#"{"created":1,"source":"Demo","source_label":"Old","source_len":512,
            "options":{"deep":true,"free_only":true,"find_partitions":true,
            "include_existing":false,"carve_off":["txt"],"tz":0},
            "finished":false,"files":[],"log":["line"]}"#;
        let mut raw = MAGIC.to_vec();
        raw.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(json, 6));
        let dir =
            std::env::temp_dir().join(format!("rescueforge-session-old-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.rfsession");
        std::fs::write(&path, raw).unwrap();
        let s = Session::load(&path).unwrap();
        assert_eq!(s.resume, None);
        assert_eq!(s.options.resume, None);
        assert!(s.options.deep && !s.finished);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn damaged_files_give_errors_not_panics() {
        let dir =
            std::env::temp_dir().join(format!("rescueforge-session-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.rfsession");
        sample().save(&good).unwrap();
        let bytes = std::fs::read(&good).unwrap();
        let path = dir.join("bad.rfsession");
        assert!(Session::load(&dir.join("missing.rfsession")).is_err());
        std::fs::write(&path, b"").unwrap();
        assert!(Session::load(&path)
            .unwrap_err()
            .contains("not a RescueForge session"));
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(Session::load(&path).is_err());
        let mut junk = MAGIC.to_vec();
        junk.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(
            b"{\"created\":[]}",
            6,
        ));
        std::fs::write(&path, &junk).unwrap();
        assert!(Session::load(&path).unwrap_err().contains("damaged"));
        let mut rng = Rng(3);
        for _ in 0..300 {
            let mut b = bytes.clone();
            for _ in 0..4 {
                let i = MAGIC.len() + rng.below((b.len() - MAGIC.len()) as u64) as usize;
                b[i] ^= 1 << rng.below(8);
            }
            std::fs::write(&path, &b).unwrap();
            let _ = Session::load(&path);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
