//! Copying found files out to a folder, and the rule that keeps the output
//! off the drive that is being recovered.

use crate::control::{Control, Progress};
use crate::model::{FileData, FoundFile, SourceSpec};
use crate::source::Source;
use crate::sys::DestInfo;
use crate::util::{sanitize_name, Crc32};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

/// Reads part of a found file. Holes and anything past the located content
/// read as zeros. Returns how many bytes lie inside the file.
pub fn read_file_range(src: &Source, f: &FoundFile, offset: u64, buf: &mut [u8]) -> usize {
    let n = f.size.saturating_sub(offset).min(buf.len() as u64) as usize;
    let buf = &mut buf[..n];
    match &f.data {
        FileData::Resident(bytes) => {
            for (i, b) in buf.iter_mut().enumerate() {
                *b = bytes.get(offset as usize + i).copied().unwrap_or(0);
            }
        }
        FileData::Extents(extents) => {
            buf.fill(0);
            let mut logical = 0u64;
            let end = offset + n as u64;
            for e in extents {
                let e_end = logical.saturating_add(e.len);
                if e_end > offset && logical < end && !e.is_sparse() {
                    let from = offset.max(logical);
                    let to = end.min(e_end);
                    let dst = &mut buf[(from - offset) as usize..(to - offset) as usize];
                    src.read_at(e.offset.saturating_add(from - logical), dst);
                }
                logical = e_end;
                if logical >= end {
                    break;
                }
            }
        }
    }
    n
}

/// How much of the file's size the located content covers.
pub fn located_bytes(f: &FoundFile) -> u64 {
    match &f.data {
        FileData::Resident(b) => (b.len() as u64).min(f.size),
        FileData::Extents(e) => e
            .iter()
            .fold(0u64, |a, x| a.saturating_add(x.len))
            .min(f.size),
    }
}

/// May recovered files (or an image, or a session file) be written here?
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// Allowed after the user confirms.
    Warn(String),
    /// Never allowed.
    Blocked(String),
}

/// Decides whether `dest` is a safe place to write to while `source` is
/// being recovered. `source_disks`: the physical disks a scanned volume lives on.
pub fn judge(source: &SourceSpec, source_disks: &[u32], dest: &DestInfo) -> Verdict {
    match source {
        SourceSpec::Image { .. } => Verdict::Ok,
        SourceSpec::Volume { letter, guid } => {
            let same_letter = dest
                .root
                .to_ascii_uppercase()
                .starts_with(&format!("{}:", letter.to_ascii_uppercase()));
            let same_guid = !guid.is_empty() && *guid == dest.guid;
            if same_letter || same_guid {
                Verdict::Blocked(format!(
                    "This folder is on {letter}:, the drive you are recovering from. Every file written to that drive can land on top of the data you are trying to get back and destroy it for good. Choose a folder on another drive, a USB stick or a network share."
                ))
            } else if dest.root.is_empty() {
                Verdict::Warn(
                    "RescueForge could not work out which drive this folder is on. Make sure it is not on the drive you are recovering from.".to_string(),
                )
            } else if dest.disks.iter().any(|d| source_disks.contains(d)) {
                Verdict::Warn(format!(
                    "This folder is on a different volume, but on the same physical disk as {letter}:. The lost files will not be overwritten. If the disk itself is failing, though, writing to it adds wear: another disk is the safer choice."
                ))
            } else {
                Verdict::Ok
            }
        }
        SourceSpec::Disk { number } => {
            if dest.disks.contains(number) {
                Verdict::Blocked(format!(
                    "This folder is on disk {number}, the disk you are recovering from. The whole disk is being searched, so anything written to it can land on top of lost data. Choose a folder on another disk, a USB stick or a network share."
                ))
            } else if dest.root.is_empty() || dest.disks.is_empty() {
                Verdict::Warn(
                    "RescueForge could not work out which disk this folder is on. Make sure it is not on the disk you are recovering from.".to_string(),
                )
            } else {
                Verdict::Ok
            }
        }
        SourceSpec::Demo => {
            // The demo pretends to scan drive D:.
            if dest.root.to_ascii_uppercase().starts_with("D:") {
                judge(
                    &SourceSpec::Volume {
                        letter: "D".into(),
                        guid: String::new(),
                    },
                    source_disks,
                    dest,
                )
            } else {
                Verdict::Ok
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoverOptions {
    /// Recreate the original folders below the destination.
    pub keep_folders: bool,
    /// Read every written file back and compare it with what was read from the source.
    pub verify: bool,
}

impl Default for RecoverOptions {
    fn default() -> Self {
        RecoverOptions {
            keep_folders: true,
            verify: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// Written, but not all of the content could be read or located.
    Damaged(String),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub target: PathBuf,
    pub size: u64,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub dest: PathBuf,
    pub total: u64,
    pub ok: u64,
    pub damaged: u64,
    pub failed: u64,
    /// Bytes written.
    pub bytes: u64,
    /// Files that were damaged or failed (at most 5,000 are kept).
    pub problems: Vec<Item>,
    pub stopped: bool,
}

/// Windows refuses paths longer than 260 characters unless they are verbatim.
fn long(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.len() > 240 && s.as_bytes().get(1) == Some(&b':') && !s.starts_with("\\\\") {
        PathBuf::from(format!("\\\\?\\{s}"))
    } else {
        path.to_path_buf()
    }
}

/// Where a file goes below `dest`, without clashing with anything that exists
/// or was already written in this run.
fn target_for(
    dest: &Path,
    f: &FoundFile,
    keep_folders: bool,
    used: &mut HashSet<String>,
) -> PathBuf {
    let mut dir = dest.to_path_buf();
    if keep_folders {
        for part in f.path.split('/').filter(|p| !p.is_empty()) {
            dir.push(sanitize_name(part));
        }
    }
    let name = sanitize_name(&f.name);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.clone(), String::new()),
    };
    let mut candidate = dir.join(&name);
    let mut n = 1;
    loop {
        let key = candidate.to_string_lossy().to_lowercase();
        if !used.contains(&key) && !long(&candidate).exists() {
            used.insert(key);
            return candidate;
        }
        n += 1;
        candidate = dir.join(format!("{stem} ({n}){ext}"));
    }
}

fn write_one(
    src: &Source,
    f: &FoundFile,
    target: &Path,
    verify: bool,
    ctl: &Control,
    progress: &Progress,
) -> Outcome {
    let path = long(target);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Outcome::Failed(format!("Could not create the folder: {e}"));
        }
    }
    let mut out = match File::create(&path) {
        Ok(f) => f,
        Err(e) => return Outcome::Failed(format!("Could not create the file: {e}")),
    };
    let bad_before = src.log().bad_bytes();
    let mut crc = Crc32::new();
    let mut buf = vec![0u8; (1usize << 20).min(f.size.max(1) as usize)];
    let mut pos = 0u64;
    while pos < f.size {
        if ctl.checkpoint() {
            drop(out);
            let _ = std::fs::remove_file(&path);
            return Outcome::Failed("Stopped before this file was complete.".to_string());
        }
        let n = read_file_range(src, f, pos, &mut buf);
        if n == 0 {
            break;
        }
        if let Err(e) = out.write_all(&buf[..n]) {
            drop(out);
            let _ = std::fs::remove_file(&path);
            return Outcome::Failed(format!("Writing failed: {e}"));
        }
        crc.update(&buf[..n]);
        pos += n as u64;
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
    }
    if let Err(e) = out.flush().and_then(|_| out.sync_all()) {
        return Outcome::Failed(format!("Writing failed: {e}"));
    }
    if f.modified > 0 {
        let _ = out.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(f.modified as u64));
    }
    drop(out);

    if verify {
        // Read the output back: it must exist, have the right size and hold
        // exactly the bytes that were read from the source.
        let check = (|| -> std::io::Result<(u64, u32)> {
            let mut file = File::open(&path)?;
            let mut crc = Crc32::new();
            let mut len = 0u64;
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                crc.update(&buf[..n]);
                len += n as u64;
            }
            Ok((len, crc.finish()))
        })();
        match check {
            Ok((len, sum)) if len == f.size && sum == crc.finish() => {}
            Ok(_) => {
                return Outcome::Failed(
                    "The written file does not match what was read from the source.".to_string(),
                )
            }
            Err(e) => return Outcome::Failed(format!("The written file cannot be read back: {e}")),
        }
    }
    let unreadable = src.log().bad_bytes() - bad_before;
    let located = located_bytes(f);
    if unreadable > 0 {
        Outcome::Damaged(format!(
            "{} of this file could not be read from the source and was filled with zeros.",
            crate::util::fmt_size(unreadable)
        ))
    } else if located < f.size {
        Outcome::Damaged(format!(
            "Only {} of {} could be located; the rest was filled with zeros.",
            crate::util::fmt_size(located),
            crate::util::fmt_size(f.size)
        ))
    } else {
        Outcome::Ok
    }
}

/// Copies `files` from the source into `dest`.
pub fn recover(
    src: &Source,
    files: &[FoundFile],
    dest: &Path,
    opts: RecoverOptions,
    ctl: &Control,
    progress: &Progress,
) -> Report {
    let mut report = Report {
        dest: dest.to_path_buf(),
        total: files.len() as u64,
        ..Default::default()
    };
    progress.set(0, files.iter().map(|f| f.size).sum());
    progress.found.store(0, Ordering::Relaxed);
    let mut used = HashSet::new();
    for f in files {
        if ctl.checkpoint() {
            report.stopped = true;
            break;
        }
        progress.set_phase(&f.name);
        let target = target_for(dest, f, opts.keep_folders, &mut used);
        let before = progress.done.load(Ordering::Relaxed);
        let outcome = write_one(src, f, &target, opts.verify, ctl, progress);
        // Keep the bar honest even when a file ended early.
        progress.done.store(before + f.size, Ordering::Relaxed);
        match &outcome {
            Outcome::Ok => {
                report.ok += 1;
                report.bytes += f.size;
            }
            Outcome::Damaged(_) => {
                report.damaged += 1;
                report.bytes += f.size;
            }
            Outcome::Failed(_) => report.failed += 1,
        }
        if outcome != Outcome::Ok && report.problems.len() < 5000 {
            report.problems.push(Item {
                name: f.name.clone(),
                target,
                size: f.size,
                outcome,
            });
        }
        progress.found.fetch_add(1, Ordering::Relaxed);
    }
    if ctl.is_stopped() {
        report.stopped = true;
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Chance, Extent, Origin, Status};

    fn file(name: &str, path: &str, size: u64, data: FileData) -> FoundFile {
        FoundFile {
            name: name.into(),
            path: path.into(),
            size,
            modified: 1_700_000_000,
            created: 0,
            status: Status::Deleted,
            origin: Origin::Carved,
            chance: Chance::Good,
            data,
            note: String::new(),
        }
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rescueforge-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn ranges_across_extents_holes_and_missing_tails() {
        let data: Vec<u8> = (0..4000u32).map(|i| (i % 251) as u8).collect();
        let src = Source::from_bytes(data.clone());
        let f = file(
            "a.bin",
            "",
            1000,
            FileData::Extents(vec![
                Extent {
                    offset: 100,
                    len: 300,
                },
                Extent {
                    offset: Extent::SPARSE,
                    len: 200,
                },
                Extent {
                    offset: 2000,
                    len: 300,
                },
            ]),
        );
        let mut buf = vec![0xAAu8; 1200];
        assert_eq!(read_file_range(&src, &f, 0, &mut buf), 1000);
        assert_eq!(&buf[..300], &data[100..400]);
        assert!(buf[300..500].iter().all(|b| *b == 0), "hole");
        assert_eq!(&buf[500..800], &data[2000..2300]);
        assert!(buf[800..1000].iter().all(|b| *b == 0), "not located");
        assert_eq!(located_bytes(&f), 800);
        let mut mid = [0u8; 100];
        assert_eq!(read_file_range(&src, &f, 250, &mut mid), 100);
        assert_eq!(&mid[..50], &data[350..400]);
        assert!(mid[50..].iter().all(|b| *b == 0));
        assert_eq!(read_file_range(&src, &f, 990, &mut mid), 10);
        assert_eq!(read_file_range(&src, &f, 5000, &mut mid), 0);

        let r = file("r.txt", "", 5, FileData::Resident(b"hello".to_vec()));
        let mut b = [0u8; 16];
        assert_eq!(read_file_range(&src, &r, 1, &mut b), 4);
        assert_eq!(&b[..4], b"ello");
        // Extents that point outside the source read as zeros instead of failing.
        let wild = file(
            "w",
            "",
            64,
            FileData::Extents(vec![Extent {
                offset: u64::MAX - 5,
                len: u64::MAX,
            }]),
        );
        assert_eq!(read_file_range(&src, &wild, 0, &mut b), 16);
        assert_eq!(b, [0u8; 16]);
    }

    #[test]
    fn destination_rules() {
        let vol = SourceSpec::Volume {
            letter: "D".into(),
            guid: "\\\\?\\Volume{1}\\".into(),
        };
        let dest = |root: &str, guid: &str, disks: &[u32]| DestInfo {
            root: root.into(),
            guid: guid.into(),
            disks: disks.to_vec(),
            free: Some(1),
        };
        assert!(matches!(
            judge(&vol, &[1], &dest("D:\\", "\\\\?\\Volume{1}\\", &[1])),
            Verdict::Blocked(_)
        ));
        assert!(matches!(
            judge(&vol, &[1], &dest("d:\\", "", &[])),
            Verdict::Blocked(_)
        ));
        // The same volume mounted in a folder of another drive.
        assert!(matches!(
            judge(&vol, &[1], &dest("C:\\mount\\", "\\\\?\\Volume{1}\\", &[1])),
            Verdict::Blocked(_)
        ));
        assert!(matches!(
            judge(&vol, &[1], &dest("E:\\", "\\\\?\\Volume{2}\\", &[1])),
            Verdict::Warn(_)
        ));
        assert_eq!(
            judge(&vol, &[1], &dest("E:\\", "\\\\?\\Volume{2}\\", &[2])),
            Verdict::Ok
        );
        assert!(matches!(
            judge(&vol, &[1], &dest("", "", &[])),
            Verdict::Warn(_)
        ));
        let disk = SourceSpec::Disk { number: 2 };
        assert!(matches!(
            judge(&disk, &[], &dest("F:\\", "g", &[2])),
            Verdict::Blocked(_)
        ));
        assert_eq!(judge(&disk, &[], &dest("C:\\", "g", &[0])), Verdict::Ok);
        assert!(matches!(
            judge(&disk, &[], &dest("X:\\", "g", &[])),
            Verdict::Warn(_)
        ));
        let image = SourceSpec::Image {
            path: "E:\\disk.img".into(),
        };
        assert_eq!(judge(&image, &[], &dest("E:\\", "g", &[2])), Verdict::Ok);
        assert!(matches!(
            judge(&SourceSpec::Demo, &[], &dest("D:\\", "", &[])),
            Verdict::Blocked(_)
        ));
        assert_eq!(
            judge(&SourceSpec::Demo, &[], &dest("E:\\", "", &[])),
            Verdict::Ok
        );
    }

    #[test]
    fn recovers_with_folders_collisions_and_verification() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 256) as u8).collect();
        let src = Source::from_bytes(data.clone());
        let ext = |o: u64, l: u64| FileData::Extents(vec![Extent { offset: o, len: l }]);
        let files = vec![
            file("photo.jpg", "Trip/Day 1", 50_000, ext(1000, 50_000)),
            file("photo.jpg", "Trip/Day 1", 30_000, ext(60_000, 30_000)),
            file("PHOTO.JPG", "Trip/Day 1", 10, ext(0, 10)),
            file(
                "bad:name?.txt",
                "A<B/..",
                3,
                FileData::Resident(b"abc".to_vec()),
            ),
            file("short.bin", "", 9_000, ext(190_000, 4_000)),
        ];
        let dest = temp("recover");
        // A file that already exists in the destination must not be replaced.
        std::fs::create_dir_all(dest.join("Trip").join("Day 1")).unwrap();
        std::fs::write(
            dest.join("Trip").join("Day 1").join("photo (2).jpg"),
            b"mine",
        )
        .unwrap();
        let progress = Progress::new();
        let report = recover(
            &src,
            &files,
            &dest,
            RecoverOptions::default(),
            &Control::new(),
            &progress,
        );
        assert_eq!(
            (report.ok, report.damaged, report.failed),
            (4, 1, 0),
            "{report:#?}"
        );
        let day = dest.join("Trip").join("Day 1");
        assert_eq!(
            std::fs::read(day.join("photo.jpg")).unwrap(),
            &data[1000..51_000]
        );
        assert_eq!(std::fs::read(day.join("photo (2).jpg")).unwrap(), b"mine");
        assert_eq!(
            std::fs::read(day.join("photo (3).jpg")).unwrap(),
            &data[60_000..90_000]
        );
        assert_eq!(
            std::fs::read(day.join("PHOTO (4).JPG")).unwrap(),
            &data[0..10]
        );
        assert_eq!(
            std::fs::read(dest.join("A_B").join("_").join("bad_name_.txt")).unwrap(),
            b"abc"
        );
        let short = std::fs::read(dest.join("short.bin")).unwrap();
        assert_eq!(short.len(), 9_000);
        assert_eq!(&short[..4_000], &data[190_000..194_000]);
        assert!(short[4_000..].iter().all(|b| *b == 0));
        assert!(matches!(report.problems[0].outcome, Outcome::Damaged(_)));
        assert_eq!(progress.fraction(), 1.0);
        let modified = std::fs::metadata(day.join("photo.jpg"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            modified
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            1_700_000_000
        );

        // Without folders everything lands directly in the destination.
        let flat = temp("recover-flat");
        let r = recover(
            &src,
            &files[..2],
            &flat,
            RecoverOptions {
                keep_folders: false,
                verify: true,
            },
            &Control::new(),
            &Progress::new(),
        );
        assert_eq!(r.ok, 2);
        assert!(flat.join("photo.jpg").exists() && flat.join("photo (2).jpg").exists());

        // A stop request ends the run and reports it.
        let ctl = Control::new();
        ctl.stop();
        let r = recover(
            &src,
            &files,
            &flat,
            RecoverOptions::default(),
            &ctl,
            &Progress::new(),
        );
        assert!(r.stopped);
        assert_eq!(r.ok, 0);
        // An impossible destination is reported per file, not as a crash.
        let blocker = flat.join("photo.jpg");
        let r = recover(
            &src,
            &files[..1],
            &blocker,
            RecoverOptions::default(),
            &Control::new(),
            &Progress::new(),
        );
        assert_eq!(r.failed, 1);
        std::fs::remove_dir_all(&dest).unwrap();
        std::fs::remove_dir_all(&flat).unwrap();
    }

    #[test]
    fn unreadable_sectors_are_reported_as_damage() {
        let data = vec![0x5Au8; 100_000];
        let src = Source::new(std::sync::Arc::new(crate::source::FlakySource {
            data,
            bad: vec![(20_480, 1024)],
            fail_first: false,
            attempts: Default::default(),
        }));
        let f = file(
            "x.bin",
            "",
            60_000,
            FileData::Extents(vec![Extent {
                offset: 0,
                len: 60_000,
            }]),
        );
        let dest = temp("recover-bad");
        let r = recover(
            &src,
            &[f],
            &dest,
            RecoverOptions::default(),
            &Control::new(),
            &Progress::new(),
        );
        assert_eq!((r.ok, r.damaged, r.failed), (0, 1, 0));
        let out = std::fs::read(dest.join("x.bin")).unwrap();
        assert_eq!(out.len(), 60_000);
        assert!(out[20_480..21_504].iter().all(|b| *b == 0));
        assert!(out[..20_480].iter().all(|b| *b == 0x5A));
        std::fs::remove_dir_all(&dest).unwrap();
    }
}
