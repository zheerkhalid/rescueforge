//! Builds an NTFS image from a [`Tree`]: boot sector, master file table with
//! resident and non-resident files, cluster bitmap, MFT mirror and backup
//! boot sector. Folder indexes are not written (the scanner does not use them).

use super::{Reuse, Tree};
use std::collections::HashMap;

const CLUSTER: usize = 4096;
const RECORD: usize = 1024;
const MFT_LCN: usize = 4;
const MIRROR_LCN: usize = 2;
const BITMAP_LCN: usize = 3;

/// Layout choices for [`build_with`].
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// Size of the image in bytes.
    pub size: usize,
    /// Number of records in the master file table.
    pub records: usize,
    /// Record number of the first folder or file of the tree.
    pub first_user: usize,
}

fn filetime(unix: i64) -> u64 {
    (unix as u64 + 11_644_473_600) * 10_000_000
}

fn resident(kind: u32, content: &[u8]) -> Vec<u8> {
    let len = (24 + content.len()).div_ceil(8) * 8;
    let mut a = vec![0u8; len];
    a[..4].copy_from_slice(&kind.to_le_bytes());
    a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    a[10..12].copy_from_slice(&24u16.to_le_bytes());
    a[16..20].copy_from_slice(&(content.len() as u32).to_le_bytes());
    a[20..22].copy_from_slice(&24u16.to_le_bytes());
    a[24..24 + content.len()].copy_from_slice(content);
    a
}

fn std_info(time: i64) -> Vec<u8> {
    let mut c = vec![0u8; 48];
    for i in 0..4 {
        c[i * 8..i * 8 + 8].copy_from_slice(&filetime(time).to_le_bytes());
    }
    c[32..36].copy_from_slice(&0x20u32.to_le_bytes());
    resident(0x10, &c)
}

fn file_name(parent: u64, parent_seq: u16, name: &str, size: u64, time: i64, dir: bool) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let mut c = vec![0u8; 66 + units.len() * 2];
    c[..8].copy_from_slice(&(parent | (parent_seq as u64) << 48).to_le_bytes());
    for i in 0..4 {
        c[8 + i * 8..16 + i * 8].copy_from_slice(&filetime(time).to_le_bytes());
    }
    c[40..48].copy_from_slice(
        &size
            .div_ceil(CLUSTER as u64)
            .saturating_mul(CLUSTER as u64)
            .to_le_bytes(),
    );
    c[48..56].copy_from_slice(&size.to_le_bytes());
    c[56..60].copy_from_slice(&(if dir { 0x1000_0000u32 } else { 0x20 }).to_le_bytes());
    c[64] = units.len() as u8;
    c[65] = 1; // Win32 namespace
    for (i, u) in units.iter().enumerate() {
        c[66 + i * 2..68 + i * 2].copy_from_slice(&u.to_le_bytes());
    }
    resident(0x30, &c)
}

/// Encodes a run list: `(first cluster, cluster count)` pairs.
pub fn encode_runs(runs: &[(u64, u64)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut prev = 0i64;
    for (lcn, len) in runs {
        let delta = *lcn as i64 - prev;
        prev = *lcn as i64;
        let len_bytes = (1..=8usize)
            .find(|n| *n == 8 || *len < 1u64 << (8 * n))
            .unwrap_or(8);
        let off_bytes = (1..=8usize)
            .find(|n| *n == 8 || (delta >= -(1i64 << (8 * n - 1)) && delta < 1i64 << (8 * n - 1)))
            .unwrap_or(8);
        out.push((off_bytes << 4 | len_bytes) as u8);
        out.extend_from_slice(&len.to_le_bytes()[..len_bytes]);
        out.extend_from_slice(&delta.to_le_bytes()[..off_bytes]);
    }
    out.push(0);
    out
}

fn data_runs(runs: &[(u64, u64)], start_vcn: u64, size: u64) -> Vec<u8> {
    let list = encode_runs(runs);
    let clusters: u64 = runs.iter().map(|r| r.1).sum();
    let len = (64 + list.len()).div_ceil(8) * 8;
    let mut a = vec![0u8; len];
    a[..4].copy_from_slice(&0x80u32.to_le_bytes());
    a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    a[8] = 1;
    a[10..12].copy_from_slice(&64u16.to_le_bytes());
    a[16..24].copy_from_slice(&start_vcn.to_le_bytes());
    a[24..32].copy_from_slice(&(start_vcn + clusters - 1).to_le_bytes());
    a[32..34].copy_from_slice(&64u16.to_le_bytes());
    a[40..48].copy_from_slice(&(clusters * CLUSTER as u64).to_le_bytes());
    a[48..56].copy_from_slice(&size.to_le_bytes());
    a[56..64].copy_from_slice(&size.to_le_bytes());
    a[64..64 + list.len()].copy_from_slice(&list);
    a
}

/// Assembles one file record and applies the update-sequence protection.
pub fn record(number: u64, seq: u16, flags: u16, base: u64, attrs: &[Vec<u8>]) -> Vec<u8> {
    let mut r = vec![0u8; RECORD];
    r[..4].copy_from_slice(b"FILE");
    r[4..6].copy_from_slice(&0x30u16.to_le_bytes());
    r[6..8].copy_from_slice(&3u16.to_le_bytes());
    r[0x10..0x12].copy_from_slice(&seq.to_le_bytes());
    r[0x12..0x14].copy_from_slice(&1u16.to_le_bytes());
    r[0x14..0x16].copy_from_slice(&0x38u16.to_le_bytes());
    r[0x16..0x18].copy_from_slice(&flags.to_le_bytes());
    r[0x1C..0x20].copy_from_slice(&(RECORD as u32).to_le_bytes());
    r[0x20..0x28].copy_from_slice(&base.to_le_bytes());
    r[0x2C..0x30].copy_from_slice(&(number as u32).to_le_bytes());
    let mut off = 0x38;
    for a in attrs {
        assert!(
            off + a.len() + 8 <= RECORD - 2,
            "record {number} is too full"
        );
        r[off..off + a.len()].copy_from_slice(a);
        off += a.len();
    }
    r[off..off + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    r[0x18..0x1C].copy_from_slice(&((off + 8) as u32).to_le_bytes());
    // Update sequence: the last two bytes of each 512-byte block are saved in
    // the array and replaced by the sequence number.
    let usn = [0x01u8, 0x00];
    r[0x30..0x32].copy_from_slice(&usn);
    for i in 1..=2 {
        let end = i * 512;
        let saved = [r[end - 2], r[end - 1]];
        r[0x30 + i * 2..0x32 + i * 2].copy_from_slice(&saved);
        r[end - 2..end].copy_from_slice(&usn);
    }
    r
}

/// Builds an image of `size` bytes with a 256-record file table.
pub fn build(tree: &Tree, size: usize) -> Vec<u8> {
    build_with(
        tree,
        Layout {
            size,
            records: 256,
            first_user: 24,
        },
    )
}

pub fn build_with(tree: &Tree, layout: Layout) -> Vec<u8> {
    let total_clusters = layout.size / CLUSTER;
    let mft_clusters = layout.records * RECORD / CLUSTER;
    let mut img = vec![0u8; layout.size];
    let mut bitmap = vec![0u8; total_clusters.div_ceil(8)];
    assert!(
        bitmap.len() <= CLUSTER,
        "image too large for a one-cluster bitmap"
    );
    let set = |bitmap: &mut Vec<u8>, first: usize, n: usize, on: bool| {
        for c in first..first + n {
            if on {
                bitmap[c / 8] |= 1 << (c % 8);
            } else {
                bitmap[c / 8] &= !(1 << (c % 8));
            }
        }
    };
    let data_start = MFT_LCN + mft_clusters;
    set(&mut bitmap, 0, data_start, true);
    let mut next_cluster = data_start;
    let mut records: Vec<Vec<u8>> = Vec::new();
    let t_sys = 1_700_000_000i64;

    // System records 0..16.
    let names = [
        "$MFT", "$MFTMirr", "$LogFile", "$Volume", "$AttrDef", ".", "$Bitmap", "$Boot", "$BadClus",
        "$Secure", "$UpCase", "$Extend",
    ];
    for (i, name) in names.iter().enumerate() {
        let i = i as u64;
        let dir = i == 5 || i == 11;
        let mut attrs = vec![std_info(t_sys), file_name(5, 5, name, 0, t_sys, dir)];
        match i {
            0 => attrs.push(data_runs(
                &[(MFT_LCN as u64, mft_clusters as u64)],
                0,
                (layout.records * RECORD) as u64,
            )),
            1 => attrs.push(data_runs(&[(MIRROR_LCN as u64, 1)], 0, 4 * RECORD as u64)),
            6 => attrs.push(data_runs(&[(BITMAP_LCN as u64, 1)], 0, bitmap.len() as u64)),
            _ => {}
        }
        let seq = if i == 5 { 5 } else { i.max(1) as u16 };
        records.push(record(i, seq, if dir { 3 } else { 1 }, 0, &attrs));
    }
    while records.len() < layout.first_user {
        let n = records.len() as u64;
        records.push(record(n, 1, 0, 0, &[]));
    }

    // Folders first, so that files can refer to their record numbers.
    let dirs = tree.dirs();
    let mut dir_number: HashMap<String, (u64, u16)> = HashMap::new();
    dir_number.insert(String::new(), (5, 5));
    for d in &dirs {
        let (parent, name) = d.rsplit_once('/').unwrap_or(("", d));
        let (p, pseq) = dir_number[parent];
        let n = records.len() as u64;
        // A deleted record keeps its contents; the flag is cleared and the
        // sequence number goes up by one.
        let gone = tree.dir_deleted(d);
        let seq = if gone { 2 } else { 1 };
        let attrs = vec![
            std_info(1_740_000_000),
            file_name(p, pseq, name, 0, 1_740_000_000, true),
        ];
        records.push(record(n, seq, if gone { 2 } else { 3 }, 0, &attrs));
        dir_number.insert(d.clone(), (n, 1));
    }

    for f in &tree.files {
        let gone = tree.is_deleted(f);
        let (p, pseq) = dir_number[f.folder()];
        let n = records.len() as u64;
        let size = f.data.len() as u64;
        let mut attrs = vec![
            std_info(f.mtime),
            file_name(p, pseq, f.name(), size, f.mtime, false),
        ];
        let mut extension: Option<Vec<u8>> = None;
        if f.data.len() <= 600 {
            attrs.push(resident(0x80, &f.data));
        } else {
            let need = f.data.len().div_ceil(CLUSTER);
            // A fragmented file gets two runs with a foreign cluster between them.
            let runs: Vec<(usize, usize)> = if f.fragmented && need >= 2 {
                let a = next_cluster;
                let gap = a + need / 2;
                let b = gap + 1;
                set(&mut bitmap, gap, 1, true);
                next_cluster = b + (need - need / 2);
                vec![(a, need / 2), (b, need - need / 2)]
            } else {
                let a = next_cluster;
                next_cluster += need;
                vec![(a, need)]
            };
            let mut written = 0usize;
            let mut all: Vec<usize> = Vec::new();
            for (lcn, len) in &runs {
                set(&mut bitmap, *lcn, *len, !gone);
                let end = (written + len * CLUSTER).min(f.data.len());
                img[lcn * CLUSTER..lcn * CLUSTER + (end - written)]
                    .copy_from_slice(&f.data[written..end]);
                written = end;
                all.extend(*lcn..lcn + len);
            }
            let reused = match f.reuse {
                Reuse::None => 0..0,
                Reuse::Tail => need / 2..need,
                Reuse::All => 0..need,
            };
            for i in reused {
                set(&mut bitmap, all[i], 1, true);
                img[all[i] * CLUSTER..(all[i] + 1) * CLUSTER].fill(0xEE);
            }
            let as_u64 = |r: &[(usize, usize)]| -> Vec<(u64, u64)> {
                r.iter().map(|(a, b)| (*a as u64, *b as u64)).collect()
            };
            if runs.len() == 2 && f.name().starts_with("split-") {
                // Many-extent files spill into an extension record: the base
                // record keeps the first run, the extension holds the rest.
                attrs.push(data_runs(&as_u64(&runs[..1]), 0, size));
                extension = Some(data_runs(&as_u64(&runs[1..]), runs[0].1 as u64, 0));
            } else {
                attrs.push(data_runs(&as_u64(&runs), 0, size));
            }
        }
        let (seq, flags) = if gone { (2, 0) } else { (1, 1) };
        records.push(record(n, seq, flags, 0, &attrs));
        if let Some(ext) = extension {
            let m = records.len() as u64;
            records.push(record(m, seq, flags, n | 1 << 48, &[ext]));
        }
    }
    assert!(records.len() <= layout.records, "file table is too small");
    assert!(
        next_cluster < total_clusters - 1,
        "tree does not fit the image"
    );
    while records.len() < layout.records {
        let n = records.len() as u64;
        records.push(record(n, 1, 0, 0, &[]));
    }
    // Unused records are blank on a real volume.
    for (i, r) in records.iter().enumerate() {
        let used = i < 12 || (i >= layout.first_user && r[0x38] != 0xFF);
        let off = MFT_LCN * CLUSTER + i * RECORD;
        if used {
            img[off..off + RECORD].copy_from_slice(r);
        }
    }
    for (i, r) in records.iter().take(4).enumerate() {
        let off = MIRROR_LCN * CLUSTER + i * RECORD;
        img[off..off + RECORD].copy_from_slice(r);
    }
    img[BITMAP_LCN * CLUSTER..BITMAP_LCN * CLUSTER + bitmap.len()].copy_from_slice(&bitmap);
    let boot = boot_sector(layout.size);
    img[..512].copy_from_slice(&boot);
    let last = layout.size - 512;
    img[last..].copy_from_slice(&boot);
    img
}

fn boot_sector(size: usize) -> [u8; 512] {
    let mut b = [0u8; 512];
    b[..3].copy_from_slice(&[0xEB, 0x52, 0x90]);
    b[3..11].copy_from_slice(b"NTFS    ");
    b[11..13].copy_from_slice(&512u16.to_le_bytes());
    b[13] = (CLUSTER / 512) as u8;
    b[21] = 0xF8;
    b[24..26].copy_from_slice(&63u16.to_le_bytes());
    b[26..28].copy_from_slice(&255u16.to_le_bytes());
    b[0x24..0x28].copy_from_slice(&0x0080_0080u32.to_le_bytes());
    b[0x28..0x30].copy_from_slice(&((size / 512 - 1) as u64).to_le_bytes());
    b[0x30..0x38].copy_from_slice(&(MFT_LCN as u64).to_le_bytes());
    b[0x38..0x40].copy_from_slice(&(MIRROR_LCN as u64).to_le_bytes());
    b[0x40] = 0xF6; // 2^10 bytes per file record
    b[0x44] = 1;
    b[0x48..0x50].copy_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
    b[510] = 0x55;
    b[511] = 0xAA;
    b
}

/// What a quick format does to an NTFS volume: a new, empty file table is
/// written over the first `fresh_records` records and the cluster bitmap is
/// reset. Everything else, including the older file records further along
/// and all file contents, stays where it was.
pub fn quick_format(img: &mut [u8], fresh_records: usize) {
    let size = img.len();
    let empty = build_with(
        &Tree::default(),
        Layout {
            size,
            records: fresh_records,
            first_user: 24,
        },
    );
    let table = MFT_LCN * CLUSTER;
    img[table..table + fresh_records * RECORD]
        .copy_from_slice(&empty[table..table + fresh_records * RECORD]);
    for lcn in [MIRROR_LCN, BITMAP_LCN] {
        img[lcn * CLUSTER..(lcn + 1) * CLUSTER]
            .copy_from_slice(&empty[lcn * CLUSTER..(lcn + 1) * CLUSTER]);
    }
    img[..512].copy_from_slice(&empty[..512]);
    img[size - 512..].copy_from_slice(&empty[size - 512..]);
}
