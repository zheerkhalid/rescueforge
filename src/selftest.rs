//! `--selftest`: an end-to-end run against disk images that are built in a
//! temp folder. Files are written into the images, deleted the way each
//! file system does it, found again, recovered to a folder and compared
//! byte for byte. No real drive is opened.

use crate::control::{Control, Progress};
use crate::fs::FsKind;
use crate::model::{Chance, FoundFile, Origin, SourceSpec, Status};
use crate::recover::{judge, recover, RecoverOptions, Verdict};
use crate::scan::{run_collect, ScanOptions};
use crate::session::Session;
use crate::source::{open_image, FlakySource, Source};
use crate::testimg::{self, sample_tree, samples, Reuse, Tree};
use crate::util::{crc32, fmt_size};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn check(ok: bool, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("FAILED: {}", what()))
    }
}

fn find<'a>(files: &'a [FoundFile], path: &str) -> Option<&'a FoundFile> {
    files
        .iter()
        .find(|f| crate::fs::join(&f.path, &f.name) == path)
}

fn target(dest: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(dest.to_path_buf(), |p, part| p.join(part))
}

/// Recovers `files` into `dest` and compares each one with what the tree says it was.
fn recover_and_compare(
    src: &Source,
    files: &[FoundFile],
    prefix: &str,
    tree: &Tree,
    dest: &Path,
) -> Result<usize, String> {
    let wanted: Vec<FoundFile> = tree
        .files
        .iter()
        .filter(|t| t.reuse == Reuse::None)
        .filter_map(|t| find(files, &crate::fs::join(prefix, &t.path)).cloned())
        .collect();
    let report = recover(
        src,
        &wanted,
        dest,
        RecoverOptions::default(),
        &Control::new(),
        &Progress::new(),
    );
    check(report.failed == 0 && report.damaged == 0, || {
        format!("recovery reported problems: {:?}", report.problems)
    })?;
    let mut compared = 0;
    for t in tree.files.iter().filter(|t| t.reuse == Reuse::None) {
        let path = crate::fs::join(prefix, &t.path);
        if find(files, &path).is_none() {
            continue;
        }
        let out = target(dest, &path);
        let bytes = std::fs::read(&out)
            .map_err(|e| format!("FAILED: {} is missing: {e}", out.display()))?;
        check(bytes == t.data, || {
            format!("{path} differs from the original")
        })?;
        compared += 1;
    }
    Ok(compared)
}

pub fn run() -> Result<Vec<String>, String> {
    let base = std::env::temp_dir().join(format!("rescueforge-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let result = run_in(&base);
    let cleanup = std::fs::remove_dir_all(&base);
    let mut log = result?;
    cleanup.map_err(|e| format!("FAILED: could not remove {}: {e}", base.display()))?;
    log.push("Temp folder removed.".to_string());
    Ok(log)
}

fn run_in(base: &Path) -> Result<Vec<String>, String> {
    let mut log = Vec::new();
    let tree = sample_tree();
    let deleted = tree.files.iter().filter(|t| tree.is_deleted(t)).count();
    let intact = tree
        .files
        .iter()
        .filter(|t| tree.is_deleted(t) && t.reuse == Reuse::None)
        .count();
    let quick = ScanOptions::default();
    let deep = ScanOptions {
        deep: true,
        ..Default::default()
    };
    let mut checksums: Vec<(PathBuf, u32)> = Vec::new();
    let mut write_image = |name: &str, bytes: &[u8]| -> Result<(PathBuf, Source), String> {
        let path = base.join(name);
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        checksums.push((path.clone(), crc32(bytes)));
        let src = open_image(&path)?;
        Ok((path, src))
    };

    // 1. Quick scan and recovery on every supported file system.
    let images: Vec<(&str, Vec<u8>)> = vec![
        ("FAT12", testimg::fat::build(FsKind::Fat12, &tree)),
        ("FAT16", testimg::fat::build(FsKind::Fat16, &tree)),
        ("FAT32", testimg::fat::build(FsKind::Fat32, &tree)),
        ("exFAT", testimg::exfat::build(&tree)),
        ("NTFS", testimg::ntfs::build(&tree, 16 << 20)),
    ];
    let mut ntfs_source = None;
    for (name, bytes) in &images {
        let (_, src) = write_image(&format!("{name}.img"), bytes)?;
        let (files, _) = run_collect(&src, &quick);
        check(files.len() == deleted, || {
            format!(
                "{name}: quick scan listed {} files, expected {deleted}",
                files.len()
            )
        })?;
        check(files.iter().all(|f| f.status == Status::Deleted), || {
            format!("{name}: only deleted files are listed")
        })?;
        for t in tree.files.iter().filter(|t| tree.is_deleted(t)) {
            let f = find(&files, &t.path).ok_or(format!("FAILED: {name}: {} not found", t.path))?;
            let expected = match t.reuse {
                Reuse::None => Chance::Good,
                Reuse::Tail => Chance::Partial,
                Reuse::All => Chance::Poor,
            };
            check(f.chance == expected, || {
                format!(
                    "{name}: {} rated {:?}, expected {expected:?}",
                    t.path, f.chance
                )
            })?;
        }
        let n = recover_and_compare(&src, &files, "", &tree, &base.join(format!("out-{name}")))?;
        check(n == intact, || {
            format!("{name}: {n} files compared, expected {intact}")
        })?;
        log.push(format!(
            "{name}: image of {} built, {deleted} deleted files found by quick scan, {n} recovered and identical to the originals, 2 correctly rated as (partly) overwritten",
            fmt_size(bytes.len() as u64)
        ));
        if *name == "NTFS" {
            ntfs_source = Some(src);
        }
    }

    // 2. Deep scan: files that no record points at, in free space.
    let mut carve_image = images[4].1.clone();
    let planted: Vec<(usize, Vec<u8>)> = vec![
        (9 << 20, samples::jpeg(40, 300, 200)),
        (10 << 20, samples::pdf(41)),
        (11 << 20, samples::docx(42)),
        (12 << 20, samples::mp3(43, 80, true)),
        (13 << 20, samples::mp4(44, 300_000)),
        (
            14 << 20,
            samples::zip(&[("a.txt", samples::txt(45, 500))], false),
        ),
    ];
    for (off, data) in &planted {
        carve_image[*off..*off + data.len()].copy_from_slice(data);
    }
    let (_, src) = write_image("deep.img", &carve_image)?;
    let (files, _) = run_collect(&src, &deep);
    let carved: Vec<FoundFile> = files
        .iter()
        .filter(|f| f.origin == Origin::Carved)
        .cloned()
        .collect();
    check(carved.len() == planted.len(), || {
        format!(
            "deep scan found {} files by content, expected {}",
            carved.len(),
            planted.len()
        )
    })?;
    let out = base.join("out-deep");
    let report = recover(
        &src,
        &carved,
        &out,
        RecoverOptions::default(),
        &Control::new(),
        &Progress::new(),
    );
    check(report.ok == planted.len() as u64, || {
        format!("deep recovery: {:?}", report.problems)
    })?;
    for (off, data) in &planted {
        let f = carved
            .iter()
            .find(|f| f.first_offset() == Some(*off as u64))
            .ok_or(format!("FAILED: nothing found at offset {off}"))?;
        let bytes = std::fs::read(target(&out, &crate::fs::join(&f.path, &f.name)))
            .map_err(|e| e.to_string())?;
        check(bytes == *data, || {
            format!("{} differs from what was planted", f.name)
        })?;
    }
    check(files.len() == deleted + planted.len(), || {
        "deep scan listed duplicates".to_string()
    })?;
    log.push(format!(
        "Deep scan: {} files without any record found by content (JPEG, PDF, DOCX, MP3, MP4, ZIP), recovered and identical; no duplicates of the {deleted} named files",
        planted.len()
    ));

    // 3. Quick format: the old file records are found and the tree is rebuilt.
    let layout = testimg::ntfs::Layout {
        size: 16 << 20,
        records: 512,
        first_user: 64,
    };
    let mut formatted = testimg::ntfs::build_with(&tree, layout);
    testimg::ntfs::quick_format(&mut formatted, 64);
    let (_, src) = write_image("formatted.img", &formatted)?;
    let (before, _) = run_collect(&src, &quick);
    check(before.is_empty(), || {
        "the formatted volume should look empty to a quick scan".to_string()
    })?;
    let (files, _) = run_collect(&src, &deep);
    let prefix = "Earlier file system (NTFS)";
    let n = recover_and_compare(&src, &files, prefix, &tree, &base.join("out-formatted"))?;
    let expected = tree.files.iter().filter(|t| t.reuse == Reuse::None).count();
    check(n == expected, || {
        format!("after format: {n} files compared, expected {expected}")
    })?;
    log.push(format!(
        "Quick format: the new volume is empty, the deep scan rebuilt {n} files with names and folders from the old file records, all identical"
    ));

    // 4. A lost partition whose primary boot sector is gone.
    let fat = &images[1].1;
    let mut disk = testimg::disk::mbr(&[(0x06, fat)], &[]);
    let lost_at = disk.len();
    disk.extend_from_slice(&images[4].1);
    disk.resize(disk.len() + 64 * 512, 0);
    disk[lost_at..lost_at + 512].fill(0);
    let (_, src) = write_image("disk.img", &disk)?;
    let (files, log_lines) = run_collect(&src, &deep);
    let prefix = "Lost partition 1 (NTFS, 16.0 MB)";
    check(
        log_lines
            .iter()
            .any(|l| l.contains("not in the partition table")),
        || "the lost partition was not reported".to_string(),
    )?;
    let n = recover_and_compare(&src, &files, prefix, &tree, &base.join("out-lost"))?;
    check(n == expected, || {
        format!("lost partition: {n} files compared, expected {expected}")
    })?;
    let listed = recover_and_compare(
        &src,
        &files,
        "Partition 1 (FAT16, 16.0 MB)",
        &tree,
        &base.join("out-part"),
    )?;
    check(listed == intact, || {
        format!("listed partition: {listed} files compared, expected {intact}")
    })?;
    log.push(format!(
        "Partitioned disk: {listed} files from the listed FAT16 partition and {n} from an NTFS partition missing from the table (found through its backup boot sector), all identical"
    ));

    // 5. Session: save, reopen, recover from the reopened list.
    let src = ntfs_source.ok_or("FAILED: no NTFS source")?;
    let (files, scan_log) = run_collect(&src, &quick);
    let session = Session {
        created: 1_760_000_000,
        source: SourceSpec::Image {
            path: base.join("NTFS.img").to_string_lossy().into_owned(),
        },
        source_label: "NTFS.img".to_string(),
        source_len: src.len(),
        options: quick.clone(),
        finished: true,
        files,
        log: scan_log,
    };
    let session_path = base.join("scan.rfsession");
    session.save(&session_path)?;
    let loaded = Session::load(&session_path)?;
    check(loaded == session, || {
        "the reopened session differs".to_string()
    })?;
    let n = recover_and_compare(&src, &loaded.files, "", &tree, &base.join("out-session"))?;
    check(n == intact, || {
        "recovery from a reopened session".to_string()
    })?;
    log.push(format!(
        "Session: {} entries saved ({}), reopened without rescanning, {n} files recovered from it and identical",
        loaded.files.len(),
        fmt_size(std::fs::metadata(&session_path).map(|m| m.len()).unwrap_or(0))
    ));

    // 6. Imaging a source with unreadable sectors, then scanning the image.
    let flaky = Source::new(Arc::new(FlakySource {
        data: images[4].1.clone(),
        bad: vec![(15 << 20, 2048)],
        fail_first: false,
        attempts: Default::default(),
    }));
    let image_path = base.join("copy-of-failing-drive.img");
    let report =
        crate::imaging::create_image(&flaky, &image_path, &Control::new(), &Progress::new())?;
    check(
        report.bad_bytes == 2048 && report.bytes == images[4].1.len() as u64,
        || format!("imaging report: {report:?}"),
    )?;
    let copy = open_image(&image_path)?;
    let (files, _) = run_collect(&copy, &quick);
    let n = recover_and_compare(&copy, &files, "", &tree, &base.join("out-image"))?;
    check(n == intact, || {
        "recovery from the image of a failing drive".to_string()
    })?;
    log.push(format!(
        "Imaging: a drive with {} of unreadable sectors copied to an image (zeros in their place), the image scanned, {n} files recovered and identical",
        fmt_size(report.bad_bytes)
    ));

    // 7. The guard against recovering onto the scanned drive.
    let source = SourceSpec::Volume {
        letter: "D".into(),
        guid: "volume-1".into(),
    };
    let dest = |root: &str, guid: &str, disk: u32| crate::sys::DestInfo {
        root: root.into(),
        guid: guid.into(),
        disks: vec![disk],
        free: None,
    };
    check(
        matches!(
            judge(&source, &[1], &dest("D:\\", "volume-1", 1)),
            Verdict::Blocked(_)
        ),
        || "recovering onto the scanned volume must be refused".to_string(),
    )?;
    check(
        matches!(
            judge(&source, &[1], &dest("E:\\", "volume-2", 1)),
            Verdict::Warn(_)
        ),
        || "another volume on the same disk must ask for confirmation".to_string(),
    )?;
    check(
        judge(&source, &[1], &dest("F:\\", "volume-3", 2)) == Verdict::Ok,
        || "another disk must be accepted".to_string(),
    )?;
    log.push(
        "Destination guard: same volume refused, same disk needs confirmation, other disk accepted"
            .to_string(),
    );

    // 8. Nothing was written to any scanned image.
    for (path, sum) in &checksums {
        let now = std::fs::read(path).map_err(|e| e.to_string())?;
        check(crc32(&now) == *sum, || {
            format!("{} was modified", path.display())
        })?;
    }
    log.push(format!(
        "Read-only: all {} scanned images are byte-for-byte unchanged",
        checksums.len()
    ));
    Ok(log)
}

/// `--list`: what RescueForge can see on this PC (reads no file contents).
pub fn list_sources() -> Vec<String> {
    let mut out = vec![format!(
        "Administrator rights: {}",
        if crate::sys::is_elevated() {
            "yes"
        } else {
            "no"
        }
    )];
    for d in crate::sys::list_drives() {
        out.push(format!(
            "Drive {}:  {:8}  {:>10} total  {:>10} free  disk {:?}  {}{}",
            d.letter,
            if d.fs.is_empty() { "unknown" } else { &d.fs },
            fmt_size(d.total),
            fmt_size(d.free),
            d.disks,
            d.hw.bus,
            if d.hw.solid_state && d.hw.trim {
                "  SSD with TRIM"
            } else {
                ""
            }
        ));
    }
    for d in crate::sys::list_disks() {
        out.push(format!(
            "Disk {}:  {:>10}  {} bytes per sector  {}{}",
            d.number,
            fmt_size(d.size),
            d.sector,
            d.hw.bus,
            if d.hw.solid_state && d.hw.trim {
                "  SSD with TRIM"
            } else {
                ""
            }
        ));
    }
    out
}
