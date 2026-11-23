//! End-to-end tests of the scanners against generated disk images: files are
//! written, deleted the way each file system does it, found again and
//! compared byte for byte.

use crate::fs::FsKind;
use crate::model::{Chance, FoundFile, Origin, Status};
use crate::recover::read_file_range;
use crate::scan::{run_collect, ScanOptions};
use crate::source::{FlakySource, Source};
use crate::testimg::{self, sample_tree, samples, Reuse, TFile, Tree};
use crate::util::Rng;
use std::sync::Arc;

fn quick() -> ScanOptions {
    ScanOptions::default()
}

fn deep() -> ScanOptions {
    ScanOptions {
        deep: true,
        ..Default::default()
    }
}

fn content(src: &Source, f: &FoundFile) -> Vec<u8> {
    let mut buf = vec![0u8; f.size as usize];
    assert_eq!(read_file_range(src, f, 0, &mut buf), buf.len());
    buf
}

fn find<'a>(files: &'a [FoundFile], path: &str) -> Option<&'a FoundFile> {
    files
        .iter()
        .find(|f| crate::fs::join(&f.path, &f.name) == path)
}

/// Checks that a quick scan of `image` finds exactly what `tree` says was deleted.
fn check_quick_scan(image: Vec<u8>, tree: &Tree, prefix: &str, two_second_times: bool) {
    let src = Source::from_bytes(image);
    let (files, log) = run_collect(&src, &quick());
    for t in &tree.files {
        let path = crate::fs::join(prefix, &t.path);
        let found = find(&files, &path);
        if !tree.is_deleted(t) {
            assert!(
                found.is_none(),
                "{path}: existing files are not listed by default"
            );
            continue;
        }
        let f = found.unwrap_or_else(|| panic!("{path} was not found. Log: {log:#?}"));
        assert_eq!(f.status, Status::Deleted, "{path}");
        assert_eq!(f.size, t.data.len() as u64, "{path}");
        let expected_time = if two_second_times {
            t.mtime - t.mtime % 2
        } else {
            t.mtime
        };
        assert_eq!(f.modified, expected_time, "{path}: time");
        match t.reuse {
            Reuse::None => {
                assert_eq!(f.chance, Chance::Good, "{path}");
                assert!(content(&src, f) == t.data, "{path}: content differs");
            }
            Reuse::Tail => {
                assert_eq!(f.chance, Chance::Partial, "{path}");
                let got = content(&src, f);
                assert!(got != t.data && got[..4096] == t.data[..4096], "{path}");
            }
            Reuse::All => assert_eq!(f.chance, Chance::Poor, "{path}"),
        }
    }
    let deleted = tree.files.iter().filter(|t| tree.is_deleted(t)).count();
    assert_eq!(files.len(), deleted, "nothing else is listed: {files:#?}");

    // With "existing files" switched on, everything is listed.
    let (all, _) = run_collect(
        &src,
        &ScanOptions {
            include_existing: true,
            ..quick()
        },
    );
    assert_eq!(all.len(), tree.files.len());
    for t in tree.files.iter().filter(|t| !tree.is_deleted(t)) {
        let path = crate::fs::join(prefix, &t.path);
        let f = find(&all, &path).unwrap_or_else(|| panic!("{path} missing"));
        assert_eq!(f.status, Status::Existing);
        assert_eq!(f.chance, Chance::Good);
        assert!(content(&src, f) == t.data, "{path}: content differs");
    }
}

#[test]
fn fat12_quick_scan() {
    let tree = sample_tree();
    check_quick_scan(testimg::fat::build(FsKind::Fat12, &tree), &tree, "", true);
}

#[test]
fn fat16_quick_scan() {
    let tree = sample_tree();
    check_quick_scan(testimg::fat::build(FsKind::Fat16, &tree), &tree, "", true);
}

#[test]
fn fat32_quick_scan() {
    let tree = sample_tree();
    check_quick_scan(testimg::fat::build(FsKind::Fat32, &tree), &tree, "", true);
}

#[test]
fn exfat_quick_scan() {
    let tree = sample_tree();
    check_quick_scan(testimg::exfat::build(&tree), &tree, "", true);
}

#[test]
fn ntfs_quick_scan() {
    let tree = sample_tree();
    check_quick_scan(testimg::ntfs::build(&tree, 16 << 20), &tree, "", false);
}

#[test]
fn ntfs_details() {
    let mut tree = sample_tree();
    // A file whose run list continues in an extension record.
    tree.files.push(
        TFile::new(
            "Big/split-archive.zip",
            samples::zip(&[("x.bin", Rng(5).bytes(60_000))], false),
        )
        .deleted()
        .fragmented(),
    );
    let img = testimg::ntfs::build(&tree, 16 << 20);
    let src = Source::from_bytes(img.clone());
    let (files, _) = run_collect(&src, &quick());
    let split = find(&files, "Big/split-archive.zip").expect("split file");
    assert_eq!(split.chance, Chance::Good);
    assert!(content(&src, split) == tree.files.last().unwrap().data);
    // Small files live inside their record; the fragmented one has two pieces.
    let notes = find(&files, "Documents/notes.txt").unwrap();
    assert!(matches!(notes.data, crate::model::FileData::Resident(_)));
    let song = find(&files, "Music/song.wav").unwrap();
    match &song.data {
        crate::model::FileData::Extents(e) => assert_eq!(e.len(), 2),
        _ => panic!("song.wav should be stored outside its record"),
    }
    assert_eq!(notes.origin, Origin::Ntfs);

    // A damaged first record: the mirror copy is used instead.
    let mut broken = img.clone();
    let mft = 4 * 4096;
    broken[mft..mft + 1024].fill(0x11);
    let (again, _) = run_collect(&Source::from_bytes(broken), &quick());
    assert_eq!(again.len(), files.len());

    // Both copies damaged: the table is read as one block, and the scan still works.
    let mut worse = img.clone();
    worse[mft..mft + 1024].fill(0x11);
    worse[2 * 4096..2 * 4096 + 1024].fill(0x11);
    let (third, log) = run_collect(&Source::from_bytes(worse), &quick());
    assert_eq!(third.len(), files.len(), "{log:#?}");
    assert!(log.iter().any(|l| l.contains("damaged")));
}

#[test]
fn partitioned_disks() {
    let tree = sample_tree();
    let fat = testimg::fat::build(FsKind::Fat16, &tree);
    let ntfs = testimg::ntfs::build(&tree, 16 << 20);
    let exfat = testimg::exfat::build(&tree);
    let deleted = tree.files.iter().filter(|t| tree.is_deleted(t)).count();

    let disk = testimg::disk::mbr(&[(0x06, &fat), (0x07, &ntfs)], &[(0x07, &exfat)]);
    let src = Source::from_bytes(disk);
    let (files, log) = run_collect(&src, &quick());
    assert_eq!(files.len(), deleted * 3, "{log:#?}");
    for prefix in [
        "Partition 1 (FAT16, 16.0 MB)",
        "Partition 2 (NTFS, 16.0 MB)",
        "Partition 3 (exFAT, 16.0 MB)",
    ] {
        let f = find(&files, &format!("{prefix}/Photos/Summer trip/beach.jpg"))
            .unwrap_or_else(|| panic!("{prefix}: {log:#?}"));
        assert!(content(&src, f) == tree.files[2].data, "{prefix}");
    }

    let disk = testimg::disk::gpt(&[("Basic data partition", &ntfs), ("Card", &exfat)]);
    let src = Source::from_bytes(disk);
    let (files, _) = run_collect(&src, &quick());
    assert_eq!(files.len(), deleted * 2);
    let f = find(&files, "Partition 2 (exFAT, 16.0 MB)/Music/song.wav").unwrap();
    assert!(content(&src, f) == tree.files[7].data);
}

#[test]
fn deep_scan_carves_free_space_without_duplicates() {
    let tree = sample_tree();
    let mut img = testimg::ntfs::build(&tree, 16 << 20);
    // Files that no record points at any more, in space the volume calls free.
    let planted = [
        (9 << 20, samples::jpeg(40, 300, 200), "jpg"),
        (10 << 20, samples::pdf(41), "pdf"),
        (11 << 20, samples::docx(42), "docx"),
        (12 << 20, samples::mp3(43, 80, true), "mp3"),
        ((12 << 20) + (1 << 19), samples::png(44, 90, 90), "png"),
    ];
    for (off, data, _) in &planted {
        img[*off..*off + data.len()].copy_from_slice(data);
    }
    let src = Source::from_bytes(img);
    let (quick_files, _) = run_collect(&src, &quick());
    let (files, log) = run_collect(&src, &deep());
    let carved: Vec<&FoundFile> = files
        .iter()
        .filter(|f| f.origin == Origin::Carved)
        .collect();
    assert_eq!(carved.len(), planted.len(), "{carved:#?}\n{log:#?}");
    for (off, data, ext) in &planted {
        let f = carved
            .iter()
            .find(|f| f.first_offset() == Some(*off as u64))
            .unwrap_or_else(|| panic!("planted .{ext} at {off} not found"));
        assert_eq!(f.ext(), *ext);
        assert!(f.path.starts_with("Found by content/"));
        assert_eq!(f.status, Status::Deleted);
        assert!(content(&src, f) == *data, ".{ext}: content differs");
    }
    // Everything the quick scan found is still there, once.
    assert_eq!(files.len(), quick_files.len() + planted.len());

    // Reading every sector also meets the files that still exist. They are
    // only listed when asked for.
    let whole = ScanOptions {
        free_only: false,
        ..deep()
    };
    let (all, _) = run_collect(&src, &whole);
    assert_eq!(all.len(), files.len());
    let (with_existing, _) = run_collect(
        &src,
        &ScanOptions {
            include_existing: true,
            ..whole
        },
    );
    assert!(with_existing.len() > files.len());
    let kept = find(&with_existing, "Photos/keep.png").expect("existing file by record");
    assert_eq!(kept.status, Status::Existing);
    assert_eq!(
        with_existing
            .iter()
            .filter(|f| f.first_offset() == kept.first_offset())
            .count(),
        1,
        "the carver does not list a file the file system already listed"
    );
}

#[test]
fn deep_scan_of_a_source_without_any_file_system() {
    let mut rng = Rng(123);
    let mut img = vec![0u8; 3 << 20];
    let planted = [
        (4096usize, samples::jpeg(1, 200, 150)),
        (300_032, samples::xlsx(2)),
        (1 << 20, samples::mp4(3, 50_000)),
        (2 << 20, samples::rar5(4, 7000)),
    ];
    // Old junk between the files.
    rng.fill(&mut img[600_000..700_000]);
    for (off, data) in &planted {
        img[*off..*off + data.len()].copy_from_slice(data);
    }
    let src = Source::from_bytes(img);
    let (none, log) = run_collect(&src, &quick());
    assert!(none.is_empty());
    assert!(log.iter().any(|l| l.contains("No working file system")));
    let (files, _) = run_collect(&src, &deep());
    assert_eq!(files.len(), planted.len(), "{files:#?}");
    for (off, data) in &planted {
        let f = files
            .iter()
            .find(|f| f.first_offset() == Some(*off as u64))
            .unwrap();
        assert!(content(&src, f) == *data);
        assert_eq!(f.chance, Chance::Good);
    }
    let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert!(names.contains(&"Found by content/Pictures/JPG"));
    assert!(names.contains(&"Found by content/Documents/XLSX"));
    assert!(names.contains(&"Found by content/Video/MP4"));
    assert!(names.contains(&"Found by content/Archives/RAR"));
}

#[test]
fn lost_partitions_are_found_by_their_boot_sectors() {
    let tree = sample_tree();
    let fat = testimg::fat::build(FsKind::Fat16, &tree);
    let ntfs = testimg::ntfs::build(&tree, 16 << 20);
    let fat32 = testimg::fat::build(FsKind::Fat32, &tree);
    let exfat = testimg::exfat::build(&tree);
    // Only the first volume is in the partition table; the others follow it unlisted.
    let mut disk = testimg::disk::mbr(&[(0x06, &fat)], &[]);
    let ntfs_at = disk.len();
    disk.extend_from_slice(&ntfs);
    disk.resize(disk.len() + 64 * 512, 0);
    let fat32_at = disk.len();
    disk.extend_from_slice(&fat32);
    disk.resize(disk.len() + 64 * 512, 0);
    let exfat_at = disk.len();
    disk.extend_from_slice(&exfat);
    disk.resize(disk.len() + 64 * 512, 0);
    // The primary boot sectors of the lost volumes are wiped: only the backups remain.
    disk[ntfs_at..ntfs_at + 512].fill(0);
    disk[fat32_at..fat32_at + 512].fill(0);
    disk[exfat_at..exfat_at + 512].fill(0);

    let src = Source::from_bytes(disk);
    let (quick_files, _) = run_collect(&src, &quick());
    assert!(quick_files
        .iter()
        .all(|f| f.path.starts_with("Partition 1 (FAT16")));
    let (files, log) = run_collect(&src, &deep());
    for (n, kind, size) in [
        (1, "NTFS", "16.0 MB"),
        (2, "FAT32", "34.2 MB"),
        (3, "exFAT", "16.0 MB"),
    ] {
        let prefix = format!("Lost partition {n} ({kind}, {size})");
        // Deleted files of the lost volume, and the ones that were never deleted.
        let beach = find(&files, &format!("{prefix}/Photos/Summer trip/beach.jpg"))
            .unwrap_or_else(|| panic!("{prefix}: {log:#?}"));
        assert_eq!(beach.status, Status::Deleted);
        assert!(content(&src, beach) == tree.files[2].data, "{prefix}");
        let kept = find(&files, &format!("{prefix}/Photos/keep.png"))
            .unwrap_or_else(|| panic!("{prefix}: existing file"));
        assert_eq!(kept.status, Status::Lost);
        assert!(content(&src, kept) == tree.files[1].data, "{prefix}");
    }
    assert_eq!(
        log.iter()
            .filter(|l| l.contains("not in the partition table"))
            .count(),
        3,
        "{log:#?}"
    );
    // With the search for partitions switched off, they are not reported.
    let (without, _) = run_collect(
        &src,
        &ScanOptions {
            find_partitions: false,
            ..deep()
        },
    );
    assert!(!without.iter().any(|f| f.path.starts_with("Lost partition")));
}

#[test]
fn files_come_back_after_a_quick_format() {
    let tree = sample_tree();
    let layout = testimg::ntfs::Layout {
        size: 16 << 20,
        records: 512,
        first_user: 64,
    };
    let mut img = testimg::ntfs::build_with(&tree, layout);
    testimg::ntfs::quick_format(&mut img, 64);
    let src = Source::from_bytes(img);
    let (quick_files, _) = run_collect(&src, &quick());
    assert!(quick_files.is_empty(), "the new volume is empty");
    let (files, log) = run_collect(&src, &deep());
    assert!(log.iter().any(|l| l.contains("earlier NTFS")), "{log:#?}");
    for t in &tree.files {
        if t.reuse != Reuse::None {
            continue;
        }
        let path = format!("Earlier file system (NTFS)/{}", t.path);
        let f = find(&files, &path).unwrap_or_else(|| panic!("{path} not found: {log:#?}"));
        assert_eq!(f.origin, Origin::OrphanNtfs);
        assert!(content(&src, f) == t.data, "{path}: content differs");
        assert_eq!(
            f.status,
            if tree.is_deleted(t) {
                Status::Deleted
            } else {
                Status::Lost
            }
        );
        assert_eq!(f.chance, Chance::Good, "{path}");
    }
    // The same content is not listed a second time by the carver.
    let beach = find(
        &files,
        "Earlier file system (NTFS)/Photos/Summer trip/beach.jpg",
    )
    .unwrap();
    assert_eq!(
        files
            .iter()
            .filter(|f| f.first_offset() == beach.first_offset())
            .count(),
        1
    );
}

#[test]
fn unreadable_sectors_do_not_stop_a_scan() {
    let tree = sample_tree();
    let img = testimg::ntfs::build(&tree, 16 << 20);
    let src = Source::new(Arc::new(FlakySource {
        data: img,
        // One bad sector inside the file table, a larger dead area in the data.
        bad: vec![(4 * 4096 + 30 * 1024, 512), (6 << 20, 300_000)],
        fail_first: false,
        attempts: Default::default(),
    }));
    let (files, log) = run_collect(&src, &deep());
    assert!(files.len() >= 8, "{} files", files.len());
    assert!(
        log.iter().any(|l| l.contains("could not be read")),
        "{log:#?}"
    );
    assert!(src.log().bad_bytes() >= 300_000);
}

#[test]
fn corrupted_volumes_never_panic_or_hang() {
    let tree = sample_tree();
    let images: Vec<(Vec<u8>, usize)> = vec![
        // (image, size of the area that holds the file system's own structures)
        (testimg::fat::build(FsKind::Fat12, &tree), 80_000),
        (testimg::fat::build(FsKind::Fat16, &tree), 120_000),
        (testimg::exfat::build(&tree), 180_000),
        (testimg::ntfs::build(&tree, 16 << 20), 4 * 4096 + 80 * 1024),
    ];
    let mut rng = Rng(0xD15C);
    for (image, meta) in &images {
        for round in 0..40 {
            let mut img = image.clone();
            match round % 4 {
                // Random bytes sprinkled over the metadata (boot sector kept valid).
                0 => {
                    for _ in 0..400 {
                        let at = 512 + rng.below(*meta as u64 - 512) as usize;
                        img[at] = rng.next_u64() as u8;
                    }
                }
                // Blocks of 0xFF: every size, offset and cluster number becomes huge.
                1 => {
                    for _ in 0..12 {
                        let at = 512 + rng.below(*meta as u64 - 1024) as usize;
                        img[at..at + 64].fill(0xFF);
                    }
                }
                // Random boot sector fields (signature kept).
                2 => {
                    for _ in 0..6 {
                        let at = 11 + rng.below(100) as usize;
                        img[at] = rng.next_u64() as u8;
                    }
                }
                // The image cut short.
                _ => img.truncate(4096 + rng.below(*meta as u64) as usize),
            }
            let src = Source::from_bytes(img);
            let opts = ScanOptions {
                deep: round % 8 < 2,
                include_existing: true,
                ..Default::default()
            };
            let (files, _) = run_collect(&src, &opts);
            // Whatever was listed can be read without trouble.
            for f in files.iter().take(40) {
                let mut buf = vec![0u8; f.size.min(70_000) as usize];
                read_file_range(&src, f, 0, &mut buf);
            }
        }
    }
}

/// Runs a deep scan that is stopped while reading at `at`.
fn scan_until(data: &[u8], at: u64) -> (Vec<FoundFile>, Option<crate::scan::ResumePoint>, bool) {
    use crate::control::{Control, Progress};
    use crate::scan::{run, ScanMsg};
    let ctl = Arc::new(Control::new());
    let src = Source::new(Arc::new(crate::source::StopAt {
        inner: Source::from_bytes(data.to_vec()),
        at,
        ctl: ctl.clone(),
    }));
    let progress = Progress::new();
    let mut files: Vec<FoundFile> = Vec::new();
    let mut stopped = false;
    run(&src, &deep(), &ctl, &progress, &mut |m| match m {
        ScanMsg::Files(f) => files.extend(f),
        ScanMsg::Retract(set) => files.retain(|f| !crate::scan::is_retracted(f, &set)),
        ScanMsg::Finished { stopped: s } => stopped = s,
        ScanMsg::Log(_) => {}
    });
    (files, progress.resume(), stopped)
}

/// An image without a file system, several read chunks long, with files
/// spread over it; one of them lies across a chunk border.
fn long_image() -> (Vec<u8>, Vec<(usize, Vec<u8>)>) {
    const MB: usize = 1 << 20;
    let mut img = vec![0u8; 21 * MB];
    let planted = vec![
        (4096usize, samples::jpeg(1, 200, 150)),
        (2 * MB, samples::xlsx(2)),
        (4 * MB - 8192, samples::mp4(3, 60_000)),
        (6 * MB, samples::rar5(4, 7000)),
        (9 * MB + 512, samples::png(5, 320, 200)),
        (13 * MB, samples::pdf(6)),
        (17 * MB, samples::jpeg(7, 240, 180)),
        (20 * MB + 4096, samples::docx(8)),
    ];
    for (off, data) in &planted {
        img[*off..*off + data.len()].copy_from_slice(data);
    }
    (img, planted)
}

#[test]
fn a_stopped_deep_scan_continues_where_it_was() {
    use crate::control::{Control, Progress};
    use crate::scan::{run, ResumePoint, ScanMsg};
    const MB: u64 = 1 << 20;
    let (img, planted) = long_image();
    let src = Source::from_bytes(img.clone());
    let (whole, _) = run_collect(&src, &deep());
    assert_eq!(whole.len(), planted.len(), "{whole:#?}");

    // "Stop" is pressed while the third chunk (8 to 12 MB) is being read:
    // that chunk is finished, the rest is left.
    let (first, point, stopped) = scan_until(&img, 8 * MB);
    assert!(stopped);
    let point = point.expect("a stopped deep scan can be continued");
    assert_eq!(first.len(), 5, "{first:#?}");
    assert_eq!(
        point,
        ResumePoint {
            pos: 12 * MB,
            skip_until: point.skip_until,
            lost: Vec::new(),
            listed: 5,
        }
    );
    assert!(point.skip_until > 9 * MB && point.skip_until < 10 * MB);

    // Continue: only the rest is read, and nothing is listed twice.
    let progress = Progress::new();
    let mut log = Vec::new();
    let mut rest: Vec<FoundFile> = Vec::new();
    let opts = ScanOptions {
        resume: Some(point.clone()),
        ..deep()
    };
    run(&src, &opts, &Control::new(), &progress, &mut |m| match m {
        ScanMsg::Files(f) => rest.extend(f),
        ScanMsg::Log(l) => log.push(l),
        _ => {}
    });
    assert!(
        log[0].starts_with("Continuing the deep scan at 12.0 MB"),
        "{log:#?}"
    );
    assert!(log
        .iter()
        .any(|l| l.contains("3 files recognised by content since the scan was continued")));
    assert_eq!(
        progress.resume(),
        None,
        "finished: nothing left to continue"
    );
    assert_eq!(progress.fraction(), 1.0);
    assert_eq!(rest.len(), 3, "{rest:#?}");
    assert!(rest.iter().all(|f| f.first_offset().unwrap() >= point.pos));
    for (off, data) in &planted {
        let f = first
            .iter()
            .chain(&rest)
            .find(|f| f.first_offset() == Some(*off as u64))
            .unwrap_or_else(|| panic!("the file at {off} is missing"));
        assert!(content(&src, f) == *data, "file at {off}");
    }

    // A file that reaches past the stop is not looked into again.
    let (cut, point, _) = scan_until(&img, 0);
    let point = point.unwrap();
    assert_eq!(cut.len(), 3);
    assert_eq!(point.pos, 4 * MB);
    assert_eq!(
        point.skip_until,
        ((4 * MB - 8192 + planted[2].1.len() as u64).div_ceil(512)) * 512
    );
    let (after, _) = run_collect(
        &src,
        &ScanOptions {
            resume: Some(point.clone()),
            ..deep()
        },
    );
    assert_eq!(after.len(), 5, "{after:#?}");

    // A resume point that lies beyond this source is ignored: full scan.
    let (full, log) = run_collect(
        &src,
        &ScanOptions {
            resume: Some(ResumePoint {
                pos: 1 << 40,
                ..Default::default()
            }),
            ..deep()
        },
    );
    assert_eq!(full.len(), planted.len());
    assert!(log[0].starts_with("Deep scan of"));
    // A quick scan never resumes.
    let (quick_files, log) = run_collect(
        &src,
        &ScanOptions {
            resume: Some(point),
            ..quick()
        },
    );
    assert!(quick_files.is_empty());
    assert!(log[0].starts_with("Quick scan of"));
}

#[test]
fn continuing_on_a_volume_lists_only_what_is_new() {
    // An NTFS volume with deleted files, and files without any record in
    // its free space behind the first read chunks.
    let mut img = testimg::ntfs::build(&sample_tree(), 16 << 20);
    let extra = [
        (9usize << 20, samples::jpeg(21, 200, 150)),
        (13 << 20, samples::png(22, 200, 150)),
    ];
    for (off, data) in &extra {
        assert!(
            img[*off..*off + data.len()].iter().all(|b| *b == 0),
            "the test needs free space at {off}"
        );
        img[*off..*off + data.len()].copy_from_slice(data);
    }
    let src = Source::from_bytes(img.clone());
    let (whole, _) = run_collect(&src, &deep());
    let named = whole.iter().filter(|f| f.origin != Origin::Carved).count();
    assert!(named >= 8);
    assert!(whole.iter().filter(|f| f.origin == Origin::Carved).count() >= 2);

    let (first, point, stopped) = scan_until(&img, 4 << 20);
    assert!(stopped);
    let point = point.expect("can be continued");
    assert_eq!(point.pos, 8 << 20);
    assert_eq!(
        first.iter().filter(|f| f.origin != Origin::Carved).count(),
        named,
        "the file table was read before the stop"
    );
    let (rest, log) = run_collect(
        &src,
        &ScanOptions {
            resume: Some(point),
            ..deep()
        },
    );
    assert!(
        rest.iter().all(|f| f.origin == Origin::Carved),
        "the file table is read again but not listed again: {rest:#?}"
    );
    assert!(rest.len() >= 2);
    assert_eq!(
        log.iter().filter(|l| l.contains("file system")).count(),
        0,
        "{log:#?}"
    );
    let key = |f: &FoundFile| (f.path.clone(), f.name.clone(), f.size);
    let mut both: Vec<_> = first.iter().chain(&rest).map(key).collect();
    both.sort();
    let mut expected: Vec<_> = whole.iter().map(key).collect();
    expected.sort();
    assert_eq!(both, expected);
}

#[test]
fn scans_can_be_stopped_and_run_on_a_worker_thread() {
    use crate::control::{Control, Progress};
    use crate::scan::{run, start, ScanMsg};
    let img = testimg::ntfs::build(&sample_tree(), 16 << 20);
    let src = Source::from_bytes(img);
    let ctl = Control::new();
    ctl.stop();
    let mut finished = None;
    let mut count = 0;
    run(&src, &deep(), &ctl, &Progress::new(), &mut |m| match m {
        ScanMsg::Finished { stopped } => finished = Some(stopped),
        ScanMsg::Files(f) => count += f.len(),
        _ => {}
    });
    assert_eq!(finished, Some(true));
    assert_eq!(count, 0);

    let handle = start(src, deep(), || {});
    let mut files = 0;
    loop {
        match handle
            .rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("scan answers")
        {
            ScanMsg::Files(f) => files += f.len(),
            ScanMsg::Log(_) | ScanMsg::Retract(_) => {}
            ScanMsg::Finished { stopped } => {
                assert!(!stopped);
                break;
            }
        }
    }
    assert!(files >= 8);
    assert_eq!(handle.progress.fraction(), 1.0);
    assert_eq!(handle.progress.phase(), "Finished");
}
