//! Builders for small disk images with known contents.
//!
//! RescueForge is never tested against a real disk. The unit tests, the
//! `--selftest` run and the `--demo` mode all work on images made here: a
//! tree of invented files is written into a FAT, exFAT or NTFS image, some of
//! the files are "deleted" the way the real file system would do it, and the
//! scanners are expected to bring them back byte for byte.

pub mod disk;
pub mod exfat;
pub mod fat;
pub mod ntfs;
pub mod samples;

/// What happened to a deleted file's space afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reuse {
    /// Nothing: the file is fully recoverable.
    None,
    /// The second half was given to another file.
    Tail,
    /// All of it was given to another file.
    All,
}

#[derive(Clone, Debug)]
pub struct TFile {
    /// Path with `/` separators.
    pub path: String,
    pub data: Vec<u8>,
    pub deleted: bool,
    /// Unix seconds (UTC).
    pub mtime: i64,
    /// Store in two pieces with a gap (NTFS and exFAT images).
    pub fragmented: bool,
    pub reuse: Reuse,
}

impl TFile {
    pub fn new(path: &str, data: Vec<u8>) -> TFile {
        TFile {
            path: path.to_string(),
            data,
            deleted: false,
            mtime: 1_750_000_000,
            fragmented: false,
            reuse: Reuse::None,
        }
    }
    pub fn deleted(mut self) -> TFile {
        self.deleted = true;
        self
    }
    pub fn fragmented(mut self) -> TFile {
        self.fragmented = true;
        self
    }
    pub fn reuse(mut self, r: Reuse) -> TFile {
        self.deleted = true;
        self.reuse = r;
        self
    }
    pub fn at(mut self, mtime: i64) -> TFile {
        self.mtime = mtime;
        self
    }
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
    pub fn folder(&self) -> &str {
        match self.path.rfind('/') {
            Some(i) => &self.path[..i],
            None => "",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub files: Vec<TFile>,
    /// Folders that were deleted together with everything in them.
    pub deleted_dirs: Vec<String>,
}

impl Tree {
    /// Every folder that appears in a path, parents before children.
    pub fn dirs(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for f in &self.files {
            let mut acc = String::new();
            for part in f.folder().split('/').filter(|p| !p.is_empty()) {
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(part);
                if !out.contains(&acc) {
                    out.push(acc.clone());
                }
            }
        }
        out.sort_by_key(|d| d.matches('/').count());
        out
    }

    /// Is this folder (or one above it) deleted?
    pub fn dir_deleted(&self, dir: &str) -> bool {
        self.deleted_dirs
            .iter()
            .any(|d| dir == d || dir.starts_with(&format!("{d}/")))
    }

    /// Is the file gone, either on its own or with its folder?
    pub fn is_deleted(&self, f: &TFile) -> bool {
        f.deleted || self.dir_deleted(f.folder())
    }
}

/// The tree used by most tests: a mix of small and larger files, deleted and
/// kept, with long and non-ASCII names, a deleted folder and reused space.
pub fn sample_tree() -> Tree {
    use samples as s;
    let t0 = 1_750_000_000i64;
    Tree {
        files: vec![
            TFile::new("readme.txt", b"Kept file.\r\n".to_vec()),
            TFile::new("Photos/keep.png", s::png(1, 64, 48)),
            TFile::new("Photos/Summer trip/beach.jpg", s::jpeg(2, 320, 240))
                .deleted()
                .at(t0 + 86_400),
            TFile::new(
                "Photos/Summer trip/sunset over the bay.png",
                s::png(3, 200, 150),
            )
            .deleted(),
            TFile::new("Documents/Budget 2025.xlsx", s::xlsx(4)).deleted(),
            TFile::new(
                "Documents/Letters/A rather long file name for a letter.rtf",
                s::rtf(5),
            )
            .deleted(),
            TFile::new("Documents/notes.txt", b"Milk, eggs, flour.\n".to_vec()).deleted(),
            TFile::new("Music/song.wav", s::wav(6, 30_000))
                .deleted()
                .fragmented(),
            TFile::new("Old/gone.pdf", s::pdf(7)),
            TFile::new("Old/Deeper/inside.gif", s::gif(8, 40, 30)),
            TFile::new("Videos/clip.mp4", s::mp4(9, 40_000)).reuse(Reuse::Tail),
            TFile::new("Videos/lost.avi", s::avi(10, 20_000)).reuse(Reuse::All),
            TFile::new(
                "Fotos/\u{dc}bergr\u{f6}\u{df}e \u{5199}\u{771f}.jpg",
                s::jpeg(11, 160, 120),
            )
            .deleted(),
            TFile::new("Videos/kept.mp4", s::mp4(12, 9_000)),
        ],
        deleted_dirs: vec!["Old".to_string()],
    }
}
