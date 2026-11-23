//! The scan itself.
//!
//! * Quick scan: finds the file systems on the source (directly, or through
//!   the partition table) and asks each one for its deleted files.
//! * Deep scan: afterwards reads the source sector by sector, looking for
//!   files by content, for boot sectors of lost partitions, and for file
//!   records that an earlier NTFS left behind.

use crate::carve::{is_text, Carver, Hit};
pub use crate::control::ResumePoint;
use crate::control::{Control, Progress};
use crate::fs::{self, ntfs, AllocMap, Ctx, FsKind};
use crate::model::{Category, Chance, Extent, FileData, FoundFile, Origin, Status};
use crate::partition;
use crate::source::Source;
use crate::util::{fmt_count, fmt_size};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanOptions {
    /// Also read every sector (carving, lost partitions, earlier file systems).
    pub deep: bool,
    /// Deep scan: skip space that a working file system reports as in use.
    pub free_only: bool,
    /// Deep scan: look for boot sectors of partitions that are no longer listed.
    pub find_partitions: bool,
    /// Also list files that were never deleted.
    pub include_existing: bool,
    /// File type ids (see `carve::TYPES`) that the deep scan should ignore.
    pub carve_off: Vec<String>,
    /// Local offset from UTC in seconds.
    pub tz: i32,
    /// Deep scan: continue a scan that was stopped here instead of
    /// starting the sector-by-sector pass again.
    pub resume: Option<ResumePoint>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            deep: false,
            free_only: true,
            find_partitions: true,
            include_existing: false,
            carve_off: crate::carve::default_off(),
            tz: 0,
            resume: None,
        }
    }
}

pub enum ScanMsg {
    Files(Vec<FoundFile>),
    /// Files found by content that turned out to be known by name after all
    /// (a file system record for them was found later): the content-only
    /// entries starting at these offsets should be dropped.
    Retract(HashSet<u64>),
    Log(String),
    Finished {
        stopped: bool,
    },
}

struct Volume {
    start: u64,
    len: u64,
    prefix: String,
    cluster: u64,
    alloc: Option<AllocMap>,
}

struct Sink<'a> {
    emit: &'a mut dyn FnMut(ScanMsg),
    /// Where the content of every listed file starts: the deep scan does not
    /// list the same file a second time.
    known: HashSet<u64>,
    /// Start offsets of the files that were listed by content only.
    carved: HashSet<u64>,
    total: u64,
    capped: bool,
    /// A continued scan reads the file tables again to learn which space
    /// is in use; what they list is already known and is not sent again.
    muted: bool,
}

/// The most entries one scan lists.
pub const MAX_LISTED: u64 = 5_000_000;

impl Sink<'_> {
    fn files(&mut self, mut files: Vec<FoundFile>, progress: &Progress) {
        if self.muted {
            self.known
                .extend(files.iter().filter_map(|f| f.first_offset()));
            return;
        }
        // Memory stays bounded on enormous disks: the list is capped.
        let room = MAX_LISTED.saturating_sub(self.total) as usize;
        if files.len() > room {
            files.truncate(room);
            if !self.capped {
                self.capped = true;
                (self.emit)(ScanMsg::Log(format!(
                    "More than {} files were found. The list stops there; narrow the scan (fewer file types, one partition) to see the rest.",
                    fmt_count(MAX_LISTED)
                )));
            }
        }
        if files.is_empty() {
            return;
        }
        let mut retract = HashSet::new();
        for f in &files {
            if let Some(o) = f.first_offset() {
                self.known.insert(o);
                if f.origin == Origin::Carved {
                    self.carved.insert(o);
                } else if self.carved.remove(&o) {
                    retract.insert(o);
                }
            }
        }
        if !retract.is_empty() {
            self.total -= retract.len() as u64;
            (self.emit)(ScanMsg::Retract(retract));
        }
        self.total += files.len() as u64;
        progress.found.store(self.total, Ordering::Relaxed);
        (self.emit)(ScanMsg::Files(files));
    }

    fn log(&mut self, text: String) {
        if self.muted {
            return;
        }
        (self.emit)(ScanMsg::Log(text));
    }
}

/// Runs a scan on the calling thread. Results and log lines go to `emit`.
pub fn run(
    src: &Source,
    opts: &ScanOptions,
    ctl: &Control,
    progress: &Progress,
    emit: &mut dyn FnMut(ScanMsg),
) {
    let src = src.root();
    // Only a deep scan of the same source can be continued.
    let resume = opts
        .resume
        .as_ref()
        .filter(|r| opts.deep && r.pos <= src.len());
    progress.set_resume(resume.cloned());
    let sink = RefCell::new(Sink {
        emit,
        known: HashSet::new(),
        carved: HashSet::new(),
        total: resume.map(|r| r.listed).unwrap_or(0),
        capped: false,
        muted: false,
    });
    let mut volumes: Vec<Volume> = Vec::new();
    let mut reserved: Vec<(u64, u64)> = Vec::new();
    match resume {
        Some(r) => {
            sink.borrow_mut().log(format!(
                "Continuing the deep scan at {} of {}. The source is opened read-only.",
                fmt_size(r.pos),
                fmt_size(src.len())
            ));
            sink.borrow_mut().muted = true;
        }
        None => sink.borrow_mut().log(format!(
            "{} of {} ({} sectors). The source is opened read-only.",
            if opts.deep { "Deep scan" } else { "Quick scan" },
            fmt_size(src.len()),
            fmt_count(src.len() / 512)
        )),
    }

    // Which file systems are there?
    let sector0 = src.read_vec(0, 512);
    let mut targets: Vec<(u64, u64, String, Vec<u8>)> = Vec::new();
    if fs::detect(&sector0).is_some() {
        targets.push((0, src.len(), String::new(), sector0));
    } else {
        let parts = partition::read_partitions(&src);
        if !parts.is_empty() {
            sink.borrow_mut().log(format!(
                "Partition table with {} partition{}.",
                parts.len(),
                if parts.len() == 1 { "" } else { "s" }
            ));
        }
        for p in parts {
            let boot = src.read_vec(p.offset, 512);
            match fs::detect(&boot) {
                Some(kind) => targets.push((
                    p.offset,
                    p.len,
                    format!(
                        "Partition {} ({}, {})",
                        p.index,
                        kind.label(),
                        fmt_size(p.len)
                    ),
                    boot,
                )),
                None => sink.borrow_mut().log(format!(
                    "Partition {} ({}, {}): no supported file system. A deep scan can still find files by content.",
                    p.index,
                    p.name,
                    fmt_size(p.len)
                )),
            }
        }
    }
    if targets.is_empty() {
        sink.borrow_mut().log(
            if opts.deep {
                "No working file system was found at the start of this source. The deep scan looks for lost partitions and for files by content."
            } else {
                "No working file system was found on this source, so there are no file records to read. Run a deep scan to look for lost partitions and for files by content."
            }
            .to_string(),
        );
    }

    for (start, len, prefix, boot) in targets {
        if ctl.checkpoint() {
            break;
        }
        let vol = src.slice(start, len);
        let base = sink.borrow().total;
        let result = {
            let mut emit = |files: Vec<FoundFile>| sink.borrow_mut().files(files, progress);
            let mut log = |text: String| sink.borrow_mut().log(text);
            let mut cx = Ctx {
                ctl,
                progress,
                include_existing: opts.include_existing,
                lost: false,
                tz: opts.tz,
                prefix: &prefix,
                emit: &mut emit,
                log: &mut log,
            };
            fs::scan_volume(&vol, &boot, &mut cx)
        };
        let place = if prefix.is_empty() {
            "This volume".to_string()
        } else {
            prefix.clone()
        };
        match result {
            Some(Ok(info)) => {
                let listed = sink.borrow().total - base;
                sink.borrow_mut().log(format!(
                    "{place}: {} file system{}, {} entr{} listed.",
                    info.kind.label(),
                    if info.label.is_empty() {
                        String::new()
                    } else {
                        format!(" \"{}\"", info.label)
                    },
                    fmt_count(listed),
                    if listed == 1 { "y" } else { "ies" }
                ));
                reserved.extend(info.reserved.iter().map(|e| (e.offset, e.offset + e.len)));
                volumes.push(Volume {
                    start,
                    len,
                    prefix,
                    cluster: info.cluster,
                    alloc: info.alloc,
                });
            }
            Some(Err(e)) => sink.borrow_mut().log(format!("{place}: {e}")),
            None => {}
        }
    }

    sink.borrow_mut().muted = false;
    if opts.deep && !ctl.is_stopped() {
        deep(
            &src,
            opts,
            resume,
            ctl,
            progress,
            &mut volumes,
            &mut reserved,
            &sink,
        );
    }

    let bad = src.log().bad_bytes();
    if bad > 0 {
        sink.borrow_mut().log(format!(
            "{} in {} area{} could not be read and was treated as empty.",
            fmt_size(bad),
            fmt_count(src.log().ranges().len() as u64),
            if src.log().ranges().len() == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    let stopped = ctl.is_stopped();
    if !stopped {
        progress.set_resume(None);
    }
    progress.set_phase(if stopped { "Stopped" } else { "Finished" });
    let mut s = sink.borrow_mut();
    (s.emit)(ScanMsg::Finished { stopped });
}

fn carved_file(abs: u64, hit: &Hit, status: Status, chance: Chance) -> FoundFile {
    let cat = Category::from_ext(hit.ext);
    let (chance, note) = if hit.partial {
        (
            Chance::Partial,
            "The file's structure breaks off early: it was probably stored in several pieces or is partly overwritten.",
        )
    } else if hit.estimated {
        (
            chance,
            "This format does not record its own length, so the end of the file was estimated.",
        )
    } else {
        (chance, "")
    };
    FoundFile {
        name: format!("recovered-{:09}.{}", abs / 512, hit.ext),
        path: format!(
            "Found by content/{}/{}",
            cat.label(),
            hit.ext.to_uppercase()
        ),
        size: hit.size,
        modified: 0,
        created: 0,
        status,
        origin: Origin::Carved,
        chance,
        data: FileData::Extents(vec![Extent {
            offset: abs,
            len: hit.size,
        }]),
        note: note.to_string(),
    }
}

/// Extensions [`magic_matches`] knows a signature for.
const MAGIC_EXTS: [&str; 34] = [
    "jpg", "jpeg", "png", "gif", "pdf", "zip", "docx", "xlsx", "pptx", "jar", "apk", "odt", "doc",
    "xls", "ppt", "msi", "mp4", "mov", "m4a", "m4v", "3gp", "heic", "rar", "7z", "exe", "dll",
    "mp3", "bmp", "avi", "wav", "webp", "mkv", "flac", "ogg",
];

/// Is the start of a file with this extension what one would expect?
fn magic_matches(ext: &str, b: &[u8]) -> bool {
    if b.len() < 8 {
        return false;
    }
    match ext {
        "jpg" | "jpeg" => b.starts_with(&[0xFF, 0xD8, 0xFF]),
        "png" => b.starts_with(b"\x89PNG"),
        "gif" => b.starts_with(b"GIF8"),
        "pdf" => b.starts_with(b"%PDF"),
        "zip" | "docx" | "xlsx" | "pptx" | "jar" | "apk" | "odt" => b.starts_with(b"PK\x03\x04"),
        "doc" | "xls" | "ppt" | "msi" => b.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]),
        "mp4" | "mov" | "m4a" | "m4v" | "3gp" | "heic" => &b[4..8] == b"ftyp",
        "rar" => b.starts_with(b"Rar!"),
        "7z" => b.starts_with(b"7z\xBC\xAF"),
        "exe" | "dll" => b.starts_with(b"MZ"),
        "mp3" => b.starts_with(b"ID3") || (b[0] == 0xFF && b[1] & 0xE0 == 0xE0),
        "bmp" => b.starts_with(b"BM"),
        "avi" | "wav" | "webp" => b.starts_with(b"RIFF"),
        "mkv" | "webm" => b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]),
        "flac" => b.starts_with(b"fLaC"),
        "ogg" => b.starts_with(b"OggS"),
        _ => false,
    }
}

/// Checks whether a boot sector found mid-disk really begins (or backs up) a
/// volume, and returns where that volume starts.
fn locate_lost_volume(src: &Source, abs: u64, head: &[u8], known: &HashSet<u64>) -> Option<u64> {
    let ntfs_ok = |start: u64, b: &ntfs::Boot| {
        b.mft_lcn
            .checked_mul(b.cluster)
            .and_then(|o| start.checked_add(o))
            .and_then(|o| src.read_exact_vec(o, 8))
            .map(|r| r.starts_with(b"FILE"))
            .unwrap_or(false)
    };
    let fat_ok = |start: u64, b: &fs::fat::Boot| {
        let f = src.read_vec(start + b.fat_start, 4);
        // The table opens with the media byte followed by all-ones.
        f.len() == 4 && f[0] == head[21] && f[1] == 0xFF && f[2] == 0xFF
    };
    let exfat_ok = |start: u64, b: &fs::exfat::Boot| {
        src.read_vec(start + b.fat_start, 4) == [0xF8, 0xFF, 0xFF, 0xFF]
    };
    // A backup boot sector sits at a fixed distance from the start of its volume.
    let (backup_distance, check): (Option<u64>, Box<dyn Fn(u64) -> bool + '_>) =
        if let Some(b) = ntfs::parse_boot(head) {
            (
                b.total_sectors.checked_mul(b.bps),
                Box::new(move |s| ntfs_ok(s, &b)),
            )
        } else if let Some(b) = fs::exfat::parse_boot(head) {
            (Some(12 * b.bps), Box::new(move |s| exfat_ok(s, &b)))
        } else {
            let b = fs::fat::parse_boot(head)?;
            let distance = (b.kind == FsKind::Fat32).then_some(6 * b.bps);
            (distance, Box::new(move |s| fat_ok(s, &b)))
        };
    if let Some(start) = backup_distance.and_then(|d| abs.checked_sub(d)) {
        if known.contains(&start) {
            // The backup of a volume that is already listed.
            return None;
        }
        if start != abs && check(start) {
            // Found through the backup copy (the primary may be damaged).
            return Some(start);
        }
    }
    if !known.contains(&abs) && check(abs) {
        return Some(abs);
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn deep(
    src: &Source,
    opts: &ScanOptions,
    resume: Option<&ResumePoint>,
    ctl: &Control,
    progress: &Progress,
    volumes: &mut Vec<Volume>,
    reserved: &mut Vec<(u64, u64)>,
    sink: &RefCell<Sink>,
) {
    const CHUNK: usize = 4 << 20;
    let carver = Carver::new(&opts.carve_off);
    let total = src.len();
    let free_only = opts.free_only && volumes.iter().any(|v| v.alloc.is_some());
    progress.set_phase(if free_only {
        "Deep scan: reading free space"
    } else {
        "Deep scan: reading every sector"
    });
    progress.set(0, total);
    let mut vol_starts: HashSet<u64> = volumes.iter().map(|v| v.start).collect();
    let mut orphans: Vec<(u64, ntfs::Record)> = Vec::new();
    let mut batch: Vec<FoundFile> = Vec::new();
    let mut last_flush = Instant::now();
    let mut buf = vec![0u8; CHUNK];
    let mut pos = 0u64;
    let mut skip_until = 0u64;
    let mut after_text = false;
    let mut lost_count = 0u32;
    let mut lost_starts: Vec<u64> = Vec::new();
    let mut carved_count = 0u64;
    let mut skipped = 0u64;
    if let Some(r) = resume {
        // Chunks are read whole: start again at the chunk that was cut short.
        pos = (r.pos / CHUNK as u64 * CHUNK as u64).min(total);
        skip_until = r.skip_until;
        lost_starts = r.lost.clone();
        lost_count = lost_starts.len() as u32;
        vol_starts.extend(lost_starts.iter().copied());
    }

    while pos < total {
        progress.set_resume(Some(ResumePoint {
            pos,
            skip_until,
            lost: lost_starts.clone(),
            // What waits in `batch` is sent before the scan ends.
            listed: sink.borrow().total + batch.len() as u64,
        }));
        if ctl.checkpoint() {
            break;
        }
        let n = (total - pos).min(CHUNK as u64) as usize;
        let chunk_end = pos + n as u64;
        let skip_chunk = skip_until >= chunk_end
            || (free_only
                && volumes.iter().any(|v| match &v.alloc {
                    Some(m) => {
                        pos >= m.vol_start && chunk_end <= m.vol_end && !m.any_free(pos, n as u64)
                    }
                    None => false,
                }));
        if skip_chunk {
            if skip_until < chunk_end {
                skipped += n as u64;
            }
            pos = chunk_end;
            progress.set(pos, total);
            continue;
        }
        src.read_at(pos, &mut buf[..n]);
        let mut off = (skip_until.max(pos).div_ceil(512) * 512 - pos) as usize;
        while off + 512 <= n {
            let abs = pos + off as u64;
            let head = &buf[off..off + 512];
            if head[..8] == [0u8; 8] && head[257] == 0 {
                after_text = false;
                off += 512;
                continue;
            }
            if reserved.iter().any(|(s, e)| abs >= *s && abs < *e) {
                off += 512;
                continue;
            }
            let in_use = volumes
                .iter()
                .find_map(|v| v.alloc.as_ref().and_then(|m| m.byte_alloc(abs)))
                == Some(true);
            if free_only && in_use {
                after_text = false;
                skipped += 512;
                off += 512;
                continue;
            }

            // A boot sector that no partition table entry points at?
            if opts.find_partitions && head[510] == 0x55 && head[511] == 0xAA && lost_count < 64 {
                if let Some(start) = locate_lost_volume(src, abs, head, &vol_starts) {
                    vol_starts.insert(start);
                    let boot = head.to_vec();
                    if let Some(kind) = fs::detect(&boot) {
                        lost_count += 1;
                        let len = volume_len(&boot, src.len() - start);
                        let prefix = format!(
                            "Lost partition {lost_count} ({}, {})",
                            kind.label(),
                            fmt_size(len)
                        );
                        sink.borrow_mut()
                            .files(std::mem::take(&mut batch), progress);
                        sink.borrow_mut().log(format!(
                            "Found a {} volume of {} that is not in the partition table, starting at byte {}{}.",
                            kind.label(),
                            fmt_size(len),
                            fmt_count(start),
                            if start == abs { "" } else { " (located through its backup boot sector)" }
                        ));
                        let phase = progress.phase();
                        let inner = Progress::new();
                        let vol = src.slice(start, len);
                        let before = sink.borrow().total;
                        let result = {
                            let mut emit =
                                |files: Vec<FoundFile>| sink.borrow_mut().files(files, progress);
                            let mut log = |text: String| sink.borrow_mut().log(text);
                            let mut cx = Ctx {
                                ctl,
                                progress: &inner,
                                include_existing: true,
                                lost: true,
                                tz: opts.tz,
                                prefix: &prefix,
                                emit: &mut emit,
                                log: &mut log,
                            };
                            progress.set_phase(&format!("Reading lost partition {lost_count}"));
                            fs::scan_volume(&vol, &boot, &mut cx)
                        };
                        progress.set_phase(&phase);
                        match result {
                            Some(Ok(info)) => {
                                let listed = sink.borrow().total - before;
                                sink.borrow_mut().log(format!(
                                    "{prefix}: {} entr{} listed.",
                                    fmt_count(listed),
                                    if listed == 1 { "y" } else { "ies" }
                                ));
                                reserved.extend(
                                    info.reserved.iter().map(|e| (e.offset, e.offset + e.len)),
                                );
                                // Only a volume that was read to the end counts as done.
                                if !ctl.is_stopped() {
                                    lost_starts.push(start);
                                }
                                volumes.push(Volume {
                                    start,
                                    len,
                                    prefix,
                                    cluster: info.cluster,
                                    // The old allocation says nothing about what is in use now.
                                    alloc: None,
                                });
                            }
                            Some(Err(e)) => sink.borrow_mut().log(format!("{prefix}: {e}")),
                            None => {}
                        }
                        if ctl.is_stopped() {
                            break;
                        }
                    }
                }
            }

            // A file record of an NTFS that is no longer mounted here?
            if head.starts_with(b"FILE") {
                if let Some(size) = ntfs::looks_like_record(head) {
                    let mut rec = if off + size <= n {
                        buf[off..off + size].to_vec()
                    } else {
                        src.read_vec(abs, size)
                    };
                    if let Some(r) = ntfs::parse_record(&mut rec, None) {
                        if orphans.len() < ntfs::MAX_RECORDS {
                            orphans.push((abs, r));
                        }
                    }
                    off += size;
                    after_text = false;
                    continue;
                }
            }

            if let Some(hit) = carver.identify(src, abs, head, after_text) {
                if !sink.borrow().known.contains(&abs) {
                    let status = if in_use {
                        Status::Existing
                    } else {
                        Status::Deleted
                    };
                    if status == Status::Deleted || opts.include_existing {
                        batch.push(carved_file(abs, &hit, status, Chance::Good));
                        carved_count += 1;
                    }
                }
                skip_until = (abs + hit.size).div_ceil(512) * 512;
                after_text = false;
                if skip_until >= chunk_end {
                    break;
                }
                off = (skip_until - pos) as usize;
                continue;
            }
            if carver.wants_text() {
                after_text = is_text(head[511]);
            }
            off += 512;
        }
        pos = chunk_end;
        progress.set(pos, total);
        if batch.len() >= 500
            || (!batch.is_empty() && last_flush.elapsed() > Duration::from_millis(400))
        {
            sink.borrow_mut()
                .files(std::mem::take(&mut batch), progress);
            last_flush = Instant::now();
        }
    }
    sink.borrow_mut()
        .files(std::mem::take(&mut batch), progress);
    if !ctl.is_stopped() {
        sink.borrow_mut().log(format!(
            "Deep scan: {} file{} recognised by content{}{}.",
            fmt_count(carved_count),
            if carved_count == 1 { "" } else { "s" },
            if resume.is_some() {
                " since the scan was continued"
            } else {
                ""
            },
            if free_only {
                format!(
                    " in free space ({} that is in use by existing files was skipped)",
                    fmt_size(skipped)
                )
            } else {
                " (every sector was read)".to_string()
            }
        ));
    }

    // Records that were inside the file table of a volume found later in the
    // pass are already listed by that volume.
    orphans.retain(|(abs, _)| !reserved.iter().any(|(s, e)| abs >= s && abs < e));
    if orphans.is_empty() {
        return;
    }
    // A stopped scan still lists the records it has seen: they would be
    // lost otherwise, because a continued scan does not read that part again.
    progress.set_phase("Rebuilding folders of an earlier file system");
    // Group by the volume the record lies in (usize::MAX: outside all of them).
    let mut groups: Vec<(usize, Vec<ntfs::Record>)> = Vec::new();
    for (abs, r) in orphans {
        let key = volumes
            .iter()
            .position(|v| abs >= v.start && abs < v.start + v.len)
            .unwrap_or(usize::MAX);
        match groups.iter_mut().find(|g| g.0 == key) {
            Some(g) => g.1.push(r),
            None => groups.push((key, vec![r])),
        }
    }
    for (key, records) in groups {
        let vol = volumes.get(key);
        let (base, len) = vol.map(|v| (v.start, v.len)).unwrap_or((0, src.len()));
        // The cluster size of the old file system is not recorded anywhere
        // that survived: try the likely ones against files whose type is
        // known from their extension.
        let samples: Vec<(String, u64)> = records
            .iter()
            .filter(|r| !r.is_dir && r.base == 0)
            .filter_map(|r| {
                let lcn = r.parts.iter().find(|p| p.0 == 0)?.1.first()?.lcn?;
                Some((crate::util::ext_of(&r.name), lcn))
            })
            .filter(|(ext, _)| MAGIC_EXTS.contains(&ext.as_str()))
            .take(400)
            .collect();
        let mut candidates: Vec<u64> = vec![4096, 512, 1024, 2048, 8192, 16_384, 32_768, 65_536];
        if let Some(v) = vol {
            candidates.retain(|c| *c != v.cluster);
            candidates.insert(0, v.cluster);
        }
        let mut best = (candidates[0], 0usize);
        for c in &candidates {
            let score = samples
                .iter()
                .filter(|(ext, lcn)| {
                    lcn.checked_mul(*c)
                        .filter(|o| *o < len)
                        .map(|o| magic_matches(ext, &src.read_vec(base + o, 16)))
                        .unwrap_or(false)
                })
                .count();
            if score > best.1 {
                best = (*c, score);
            }
        }
        let geo = ntfs::Geometry {
            vol_base: base,
            cluster: best.0,
            clusters: len / best.0,
        };
        let mut col = ntfs::Collector::default();
        let count = records.len();
        for r in records {
            col.add(r, true);
        }
        let prefix = fs::join(
            vol.map(|v| v.prefix.as_str()).unwrap_or(""),
            "Earlier file system (NTFS)",
        );
        let files = col.finish(
            &geo,
            vol.and_then(|v| v.alloc.as_ref()),
            &prefix,
            Origin::OrphanNtfs,
            &|in_use| {
                if in_use {
                    Status::Lost
                } else {
                    Status::Deleted
                }
            },
        );
        // Files the deep scan has just listed by content are the same data.
        let files: Vec<FoundFile> = files
            .into_iter()
            .filter(|f| f.first_offset().map(|o| o < src.len()).unwrap_or(true))
            .collect();
        sink.borrow_mut().log(format!(
            "Found {} file record{} of an earlier NTFS ({} file{} with names and folders; cluster size {}).",
            fmt_count(count as u64),
            if count == 1 { "" } else { "s" },
            fmt_count(files.len() as u64),
            if files.len() == 1 { "" } else { "s" },
            fmt_size(best.0)
        ));
        let mut it = files.into_iter().peekable();
        while it.peek().is_some() {
            sink.borrow_mut()
                .files(it.by_ref().take(5000).collect(), progress);
        }
    }
}

/// The size a boot sector claims for its volume, limited to what is there.
fn volume_len(boot: &[u8], available: u64) -> u64 {
    let claimed = if let Some(b) = ntfs::parse_boot(boot) {
        // One more sector: the backup boot sector follows the volume.
        (b.total_sectors + 1).saturating_mul(b.bps)
    } else if let Some(b) = fs::exfat::parse_boot(boot) {
        b.volume_bytes
    } else if let Some(b) = fs::fat::parse_boot(boot) {
        b.total_sectors.saturating_mul(b.bps)
    } else {
        available
    };
    claimed.min(available)
}

/// Should this entry be dropped because of a [`ScanMsg::Retract`]?
pub fn is_retracted(f: &FoundFile, set: &HashSet<u64>) -> bool {
    f.origin == Origin::Carved && f.first_offset().map(|o| set.contains(&o)).unwrap_or(false)
}

/// A scan running on its own thread.
pub struct ScanHandle {
    pub ctl: Arc<Control>,
    pub progress: Arc<Progress>,
    pub rx: Receiver<ScanMsg>,
}

/// Starts a scan on a worker thread. `wake` is called whenever there is news.
pub fn start(src: Source, opts: ScanOptions, wake: impl Fn() + Send + 'static) -> ScanHandle {
    let ctl = Arc::new(Control::new());
    let progress = Arc::new(Progress::new());
    let (tx, rx) = channel();
    let (c, p) = (ctl.clone(), progress.clone());
    std::thread::Builder::new()
        .name("scan".into())
        .spawn(move || {
            let tx2 = tx.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(&src, &opts, &c, &p, &mut |m| {
                    let _ = tx.send(m);
                    wake();
                });
            }));
            if result.is_err() {
                let _ = tx2.send(ScanMsg::Log(
                    "The scan ran into an internal error and was ended. The files found so far are kept.".to_string(),
                ));
                let _ = tx2.send(ScanMsg::Finished { stopped: true });
            }
        })
        .expect("scan thread");
    ScanHandle { ctl, progress, rx }
}

/// Runs a scan to the end and collects everything (tests, self-test, demo).
pub fn run_collect(src: &Source, opts: &ScanOptions) -> (Vec<FoundFile>, Vec<String>) {
    let mut files = Vec::new();
    let mut log = Vec::new();
    run(
        src,
        opts,
        &Control::new(),
        &Progress::new(),
        &mut |m| match m {
            ScanMsg::Files(f) => files.extend(f),
            ScanMsg::Retract(set) => files.retain(|f: &FoundFile| !is_retracted(f, &set)),
            ScanMsg::Log(l) => log.push(l),
            ScanMsg::Finished { .. } => {}
        },
    );
    (files, log)
}
