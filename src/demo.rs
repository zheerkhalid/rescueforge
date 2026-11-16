//! Demo mode: an invented disk, built in memory, that the real scanners run
//! on. Nothing on this PC is read, and every name on screen is made up.

use crate::testimg::{ntfs, samples as s, Reuse, TFile, Tree};

/// One line of the invented source list.
pub struct DemoSource {
    /// 0 drive, 1 physical disk, 2 image file
    pub kind: u8,
    pub title: &'static str,
    pub detail: &'static str,
    pub hardware: &'static str,
    pub total: u64,
    pub free: Option<u64>,
    pub removable: bool,
    pub ssd_trim: bool,
    pub letter: &'static str,
}

const GB: u64 = 1 << 30;

pub fn sources() -> Vec<DemoSource> {
    let d = |kind,
             title,
             detail,
             hardware,
             total: f64,
             free: Option<f64>,
             removable,
             ssd_trim,
             letter| DemoSource {
        kind,
        title,
        detail,
        hardware,
        total: (total * GB as f64) as u64,
        free: free.map(|f| (f * GB as f64) as u64),
        removable,
        ssd_trim,
        letter,
    };
    vec![
        d(
            0,
            "Windows (C:)",
            "NTFS",
            "NVMe solid-state drive",
            476.3,
            Some(118.4),
            false,
            true,
            "C",
        ),
        d(
            0,
            "Media (D:)",
            "NTFS",
            "SATA hard disk",
            931.5,
            Some(212.7),
            false,
            false,
            "D",
        ),
        d(
            0,
            "CAMERA (E:)",
            "exFAT",
            "SD card",
            59.4,
            Some(31.0),
            true,
            false,
            "E",
        ),
        d(
            0,
            "USB STICK (F:)",
            "FAT32",
            "USB flash drive",
            28.8,
            Some(9.1),
            true,
            false,
            "F",
        ),
        d(
            1,
            "Disk 0",
            "Northlake NV2 500",
            "NVMe solid-state drive",
            476.9,
            None,
            false,
            true,
            "",
        ),
        d(
            1,
            "Disk 1",
            "Halden HD-1000",
            "SATA hard disk",
            931.5,
            None,
            false,
            false,
            "",
        ),
        d(
            1,
            "Disk 2",
            "Generic card reader",
            "SD card",
            59.5,
            None,
            true,
            false,
            "",
        ),
        d(
            2,
            "old-laptop.img",
            "F:\\Images\\old-laptop.img",
            "Disk image file",
            238.5,
            None,
            false,
            false,
            "",
        ),
    ]
}

/// The invented disk: an NTFS volume with a few hundred files, most of them
/// deleted, plus files in free space that only a deep scan can find.
pub fn disk() -> Vec<u8> {
    let day = 86_400i64;
    let base = 1_757_000_000i64; // September 2025
    let mut files: Vec<TFile> = Vec::new();
    let mut seed = 100u64;
    let mut next = || {
        seed += 1;
        seed
    };
    for i in 0..26 {
        let n = next();
        let mut f = TFile::new(
            &format!("Photos/2025 Summer in Lisbon/IMG_{}.jpg", 2031 + i),
            s::jpeg(n, 1024, 768),
        )
        .deleted()
        .at(base - 60 * day + i as i64 * 3_700);
        if i == 7 || i == 19 {
            f = f.reuse(Reuse::Tail);
        }
        if i == 12 {
            f = f.reuse(Reuse::All);
        }
        files.push(f);
    }
    files.push(
        TFile::new(
            "Photos/2025 Summer in Lisbon/Panorama from the castle.png",
            s::png(next(), 1500, 500),
        )
        .deleted()
        .at(base - 58 * day),
    );
    for i in 0..14 {
        let n = next();
        files.push(
            TFile::new(
                &format!("Photos/Birthday/DSC0{}.jpg", 4410 + i),
                s::jpeg(n, 900, 900),
            )
            .deleted()
            .at(base - 20 * day + i as i64 * 900),
        );
    }
    for (i, name) in [
        "Passport.png",
        "Lease page 1.png",
        "Lease page 2.png",
        "Receipt.tif",
    ]
    .iter()
    .enumerate()
    {
        let n = next();
        let data = if name.ends_with("tif") {
            s::tiff(n, 600, 800)
        } else {
            s::png(n, 600, 840)
        };
        let f =
            TFile::new(&format!("Photos/Scans/{name}"), data).at(base - 200 * day + i as i64 * day);
        files.push(if i < 3 { f.deleted() } else { f });
    }
    for i in 0..12 {
        files.push(
            TFile::new(
                &format!("Documents/Invoices 2025/Invoice {:04}.pdf", 412 + i * 7),
                s::pdf_with(next(), 90_000 + i as usize * 23_000),
            )
            .deleted()
            .at(base - (300 - i as i64 * 22) * day),
        );
    }
    for (i, name) in [
        "Chapter 1 - Introduction.docx",
        "Chapter 2 - Method.docx",
        "Chapter 3 - Results.docx",
        "References.docx",
        "Figures.pptx",
        "Measurements.xlsx",
    ]
    .iter()
    .enumerate()
    {
        let n = next();
        let data = if name.ends_with("pptx") {
            s::pptx_with(n, 2_400_000)
        } else if name.ends_with("xlsx") {
            s::xlsx_with(n, 180_000)
        } else {
            s::docx_with(n, 240_000 + i * 150_000)
        };
        files.push(
            TFile::new(&format!("Documents/Thesis/{name}"), data)
                .deleted()
                .at(base - 9 * day + i as i64 * 5_000),
        );
    }
    files.push(TFile::new("Documents/Household budget.xlsx", s::xlsx(next())).at(base - 3 * day));
    files.push(
        TFile::new("Documents/Letter to the landlord.rtf", s::rtf(next()))
            .deleted()
            .at(base - 40 * day),
    );
    files.push(
        TFile::new("Documents/Shopping list.txt", s::txt(next(), 40))
            .deleted()
            .at(base - day),
    );
    files.push(
        TFile::new(
            "Documents/Old report.doc",
            s::ole(next(), "WordDocument", 30_000),
        )
        .deleted()
        .at(base - 700 * day),
    );
    for i in 0..10 {
        files.push(
            TFile::new(
                &format!("Music/Road trip mix/Track {:02}.mp3", i + 1),
                s::mp3(next(), 1500 + i * 200, true),
            )
            .deleted()
            .at(base - 400 * day),
        );
    }
    files.push(
        TFile::new("Music/Voice memo.wav", s::wav(next(), 600_000))
            .deleted()
            .fragmented()
            .at(base - 2 * day),
    );
    files.push(
        TFile::new("Videos/Garden in spring.mp4", s::mp4(next(), 7_400_000))
            .deleted()
            .at(base - 150 * day),
    );
    files.push(
        TFile::new("Videos/First steps.mov", s::mov(next(), 5_700_000))
            .deleted()
            .at(base - 420 * day),
    );
    files.push(
        TFile::new(
            "Videos/School concert.mkv",
            s::mkv(next(), 9_900_000, false),
        )
        .reuse(Reuse::Tail)
        .at(base - 90 * day),
    );
    files.push(
        TFile::new("Videos/Holiday clip.avi", s::avi(next(), 3_200_000))
            .deleted()
            .at(base - 61 * day),
    );
    files.push(
        TFile::new(
            "Projects/Website backup.zip",
            s::zip(
                &[
                    ("index.html", s::html(1)),
                    ("style.css", s::txt(2, 300)),
                    ("notes.txt", s::txt(3, 900)),
                ],
                false,
            ),
        )
        .deleted()
        .at(base - 33 * day),
    );
    files.push(
        TFile::new("Projects/Source archive.7z", s::sevenz(next(), 3_380_000))
            .deleted()
            .at(base - 31 * day),
    );
    files.push(
        TFile::new("Projects/Logo.psd", s::psd(next(), 800, 600))
            .deleted()
            .at(base - 77 * day),
    );
    files.push(
        TFile::new("Downloads/setup.exe", s::exe(next(), 3_900_000, false))
            .deleted()
            .at(base - 5 * day),
    );
    files.push(
        TFile::new("Downloads/Manual.pdf", s::pdf_with(next(), 1_300_000)).at(base - 5 * day),
    );
    files.push(TFile::new("readme.txt", s::txt(next(), 30)).at(base - 900 * day));

    let tree = Tree {
        files,
        deleted_dirs: vec!["Photos/Birthday".to_string()],
    };
    let mut img = ntfs::build_with(
        &tree,
        ntfs::Layout {
            size: 128 << 20,
            records: 512,
            first_user: 24,
        },
    );
    // Free space: files without any record left, for the deep scan.
    let mut at = 80usize << 20;
    let mut plant = |data: Vec<u8>| {
        img[at..at + data.len()].copy_from_slice(&data);
        at = (at + data.len()).div_ceil(4096) * 4096 + 8192;
    };
    for i in 0..34u64 {
        plant(s::jpeg(500 + i, 1024, 768));
    }
    for i in 0..8u64 {
        plant(s::png(600 + i, 640, 480));
    }
    for i in 0..6u64 {
        plant(s::pdf_with(700 + i, 150_000 + i as usize * 40_000));
        plant(s::docx_with(720 + i, 300_000));
    }
    for i in 0..5u64 {
        plant(s::mp3(800 + i, 2400, true));
    }
    plant(s::mp4(900, 4_500_000));
    plant(s::mp4(901, 3_800_000));
    plant(s::xlsx(902));
    plant(s::gif(903, 120, 90));
    plant(s::zip(&[("a.txt", s::txt(904, 2000))], false));
    plant(s::rar5(905, 200_000));
    // A JPEG that lost its second half.
    let cut = s::jpeg(906, 1280, 960);
    plant(cut[..cut.len() / 2].to_vec());
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Chance, Origin, Status};
    use crate::scan::{run_collect, ScanOptions};
    use crate::source::Source;

    #[test]
    fn demo_disk_scans_as_intended() {
        let src = Source::from_bytes(disk());
        let (quick, _) = run_collect(&src, &ScanOptions::default());
        assert!(quick.len() > 80, "{}", quick.len());
        assert!(quick.iter().all(|f| f.status == Status::Deleted));
        assert!(quick.iter().any(|f| f.chance == Chance::Partial));
        assert!(quick.iter().any(|f| f.chance == Chance::Poor));
        assert!(quick.iter().any(|f| f.path == "Photos/Birthday"));
        let (deep, _) = run_collect(
            &src,
            &ScanOptions {
                deep: true,
                ..Default::default()
            },
        );
        let carved = deep.iter().filter(|f| f.origin == Origin::Carved).count();
        assert!(carved >= 60, "{carved}");
        assert_eq!(deep.len(), quick.len() + carved);
        assert_eq!(sources().len(), 8);
    }
}
