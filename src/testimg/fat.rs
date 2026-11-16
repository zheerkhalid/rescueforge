//! Builds FAT12, FAT16 and FAT32 images from a [`Tree`].

use super::{Reuse, Tree};
use crate::fs::FsKind;
use crate::util::civil;
use std::collections::HashMap;

struct Geometry {
    total_sectors: u32,
    spc: u32,
    reserved: u32,
    fat_sectors: u32,
    root_entries: u32,
}

fn geometry(kind: FsKind) -> Geometry {
    match kind {
        // ~3,900 clusters of 1 KB.
        FsKind::Fat12 => Geometry {
            total_sectors: 8_000,
            spc: 2,
            reserved: 1,
            fat_sectors: 12,
            root_entries: 512,
        },
        // ~16,000 clusters of 1 KB.
        FsKind::Fat16 => Geometry {
            total_sectors: 32_768,
            spc: 2,
            reserved: 1,
            fat_sectors: 64,
            root_entries: 512,
        },
        // FAT32 needs at least 65,525 clusters.
        _ => Geometry {
            total_sectors: 70_000,
            spc: 1,
            reserved: 32,
            fat_sectors: 540,
            root_entries: 0,
        },
    }
}

fn dos_stamp(unix: i64) -> (u16, u16) {
    let (y, m, d, h, mi, s) = civil(unix);
    (
        (((y - 1980).clamp(0, 127) as u16) << 9) | ((m as u16) << 5) | d as u16,
        ((h as u16) << 11) | ((mi as u16) << 5) | (s as u16 / 2),
    )
}

fn short_sum(short: &[u8; 11]) -> u8 {
    short
        .iter()
        .fold(0u8, |s, c| (s >> 1 | (s & 1) << 7).wrapping_add(*c))
}

/// The directory entries for one name: long-name pieces, then the 8.3 entry.
fn entries(
    name: &str,
    serial: usize,
    attr: u8,
    cluster: u32,
    size: u32,
    mtime: i64,
    deleted: bool,
) -> Vec<u8> {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    };
    let clean = |s: &str, n: usize| -> Vec<u8> {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase() as u8)
            .take(n)
            .collect()
    };
    let mut short = [b' '; 11];
    let base = clean(stem, 6);
    short[..base.len()].copy_from_slice(&base);
    let tail = format!("~{}", serial % 10);
    short[base.len()..base.len() + 2].copy_from_slice(tail.as_bytes());
    let e = clean(ext, 3);
    short[8..8 + e.len()].copy_from_slice(&e);

    let mut units: Vec<u16> = name.encode_utf16().collect();
    let pieces = units.len().div_ceil(13);
    if !units.len().is_multiple_of(13) {
        units.push(0);
    }
    units.resize(pieces * 13, 0xFFFF);
    let sum = short_sum(&short);
    let mut out = Vec::new();
    for p in (0..pieces).rev() {
        let mut e = [0u8; 32];
        e[0] = (p + 1) as u8 | if p + 1 == pieces { 0x40 } else { 0 };
        e[11] = 0x0F;
        e[13] = sum;
        for (i, o) in [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
            .iter()
            .enumerate()
        {
            e[*o..*o + 2].copy_from_slice(&units[p * 13 + i].to_le_bytes());
        }
        out.extend_from_slice(&e);
    }
    let (date, time) = dos_stamp(mtime);
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(&short);
    e[11] = attr;
    e[14..16].copy_from_slice(&time.to_le_bytes());
    e[16..18].copy_from_slice(&date.to_le_bytes());
    e[18..20].copy_from_slice(&date.to_le_bytes());
    e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    e[22..24].copy_from_slice(&time.to_le_bytes());
    e[24..26].copy_from_slice(&date.to_le_bytes());
    e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&e);
    if deleted {
        for chunk in out.chunks_mut(32) {
            chunk[0] = 0xE5;
        }
    }
    out
}

fn dot_entries(own: u32, parent: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, cluster) in [(b".          ", own), (b"..         ", parent)] {
        let mut e = [0u8; 32];
        e[..11].copy_from_slice(name);
        e[11] = 0x10;
        e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
        out.extend_from_slice(&e);
    }
    out
}

/// Builds the image. Fragmentation is not simulated on FAT: a deleted FAT
/// file's chain is gone, so only files stored in one piece can come back.
pub fn build(kind: FsKind, tree: &Tree) -> Vec<u8> {
    let g = geometry(kind);
    let fat32 = kind == FsKind::Fat32;
    let bps = 512u32;
    let cluster = (g.spc * bps) as usize;
    let root_sectors = (g.root_entries * 32).div_ceil(bps);
    let data_start = ((g.reserved + 2 * g.fat_sectors + root_sectors) * bps) as usize;
    let clusters = (g.total_sectors - g.reserved - 2 * g.fat_sectors - root_sectors) / g.spc;
    let mut img = vec![0u8; (g.total_sectors * bps) as usize];
    let mut fat = vec![0u32; clusters as usize + 2];
    let eoc = 0x0FFF_FFFFu32;
    fat[0] = 0x0FFF_FFF8;
    fat[1] = eoc;
    let mut next = 2u32;
    let cluster_off = |c: u32| data_start + (c as usize - 2) * cluster;
    // Reserves `n` clusters in a row; `live` writes the chain into the table.
    let mut alloc = |fat: &mut Vec<u32>, n: usize, live: bool| -> u32 {
        let first = next;
        for i in 0..n as u32 {
            fat[(first + i) as usize] = match (live, i + 1 == n as u32) {
                (false, _) => 0,
                (true, true) => eoc,
                (true, false) => first + i + 1,
            };
        }
        next += n as u32;
        first
    };

    let dirs = tree.dirs();
    let mut content: HashMap<String, Vec<u8>> = HashMap::new();
    let mut dir_cluster: HashMap<String, u32> = HashMap::new();
    content.insert(String::new(), Vec::new());
    // Rough size of each directory so that its clusters can be set aside first.
    let dir_clusters = |d: &str| -> usize {
        let children = tree.files.iter().filter(|f| f.folder() == d).count()
            + dirs
                .iter()
                .filter(|x| x.rsplit_once('/').map(|p| p.0).unwrap_or("") == d && *x != d)
                .count();
        ((children * 5 + 3) * 32).div_ceil(cluster)
    };
    let root_cluster = if fat32 {
        alloc(&mut fat, dir_clusters("").max(2), true)
    } else {
        0
    };
    dir_cluster.insert(String::new(), root_cluster);
    for d in &dirs {
        let live = !tree.dir_deleted(d);
        let c = alloc(&mut fat, dir_clusters(d), live);
        dir_cluster.insert(d.clone(), c);
        let parent = d.rsplit_once('/').map(|p| p.0).unwrap_or("");
        content.insert(d.clone(), dot_entries(c, dir_cluster[parent]));
    }
    let mut serial = 0usize;
    for d in &dirs {
        serial += 1;
        let (parent, name) = d.rsplit_once('/').unwrap_or(("", d));
        let e = entries(
            name,
            serial,
            0x10,
            dir_cluster[d],
            0,
            1_740_000_000,
            tree.dir_deleted(d),
        );
        content
            .get_mut(parent)
            .expect("parent")
            .extend_from_slice(&e);
    }
    for f in &tree.files {
        serial += 1;
        let gone = tree.is_deleted(f);
        let n = f.data.len().div_ceil(cluster).max(1);
        let first = alloc(&mut fat, n, !gone);
        let off = cluster_off(first);
        img[off..off + f.data.len()].copy_from_slice(&f.data);
        // Space that was handed to another file after the deletion.
        let reused = match f.reuse {
            Reuse::None => 0..0,
            Reuse::Tail => n / 2..n,
            Reuse::All => 0..n,
        };
        for i in reused {
            fat[first as usize + i] = eoc;
            let o = cluster_off(first + i as u32);
            img[o..o + cluster].fill(0xEE);
        }
        let e = entries(
            f.name(),
            serial,
            0x20,
            first,
            f.data.len() as u32,
            f.mtime,
            gone,
        );
        content
            .get_mut(f.folder())
            .expect("folder")
            .extend_from_slice(&e);
    }
    assert!(next < clusters, "sample tree does not fit the image");

    // Write the directories.
    for (d, bytes) in &content {
        if d.is_empty() && !fat32 {
            let off = ((g.reserved + 2 * g.fat_sectors) * bps) as usize;
            assert!(bytes.len() <= (g.root_entries * 32) as usize);
            img[off..off + bytes.len()].copy_from_slice(bytes);
        } else {
            let off = cluster_off(dir_cluster[d]);
            let room = if d.is_empty() {
                dir_clusters("").max(2)
            } else {
                dir_clusters(d)
            } * cluster;
            assert!(bytes.len() < room, "directory {d} overflows");
            img[off..off + bytes.len()].copy_from_slice(bytes);
        }
    }

    // Boot sector.
    let mut b = [0u8; 512];
    b[..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    b[3..11].copy_from_slice(b"MSDOS5.0");
    b[11..13].copy_from_slice(&(bps as u16).to_le_bytes());
    b[13] = g.spc as u8;
    b[14..16].copy_from_slice(&(g.reserved as u16).to_le_bytes());
    b[16] = 2;
    b[17..19].copy_from_slice(&(g.root_entries as u16).to_le_bytes());
    if g.total_sectors < 65_536 && !fat32 {
        b[19..21].copy_from_slice(&(g.total_sectors as u16).to_le_bytes());
    } else {
        b[32..36].copy_from_slice(&g.total_sectors.to_le_bytes());
    }
    b[21] = 0xF8;
    b[24..26].copy_from_slice(&63u16.to_le_bytes());
    b[26..28].copy_from_slice(&255u16.to_le_bytes());
    if fat32 {
        b[36..40].copy_from_slice(&g.fat_sectors.to_le_bytes());
        b[44..48].copy_from_slice(&root_cluster.to_le_bytes());
        b[48..50].copy_from_slice(&1u16.to_le_bytes());
        b[50..52].copy_from_slice(&6u16.to_le_bytes());
        b[66] = 0x29;
        b[71..82].copy_from_slice(b"RESCUE     ");
        b[82..90].copy_from_slice(b"FAT32   ");
    } else {
        b[22..24].copy_from_slice(&(g.fat_sectors as u16).to_le_bytes());
        b[38] = 0x29;
        b[43..54].copy_from_slice(b"RESCUE     ");
        b[54..62].copy_from_slice(if kind == FsKind::Fat12 {
            b"FAT12   "
        } else {
            b"FAT16   "
        });
    }
    b[510] = 0x55;
    b[511] = 0xAA;
    img[..512].copy_from_slice(&b);
    if fat32 {
        img[6 * 512..7 * 512].copy_from_slice(&b);
    }

    // Both copies of the allocation table.
    let mut raw = vec![0u8; (g.fat_sectors * bps) as usize];
    for (i, v) in fat.iter().enumerate() {
        match kind {
            FsKind::Fat12 => {
                let v = (*v & 0xFFF) as u16;
                let o = i + i / 2;
                if i % 2 == 0 {
                    raw[o] = v as u8;
                    raw[o + 1] = (raw[o + 1] & 0xF0) | (v >> 8) as u8;
                } else {
                    raw[o] = (raw[o] & 0x0F) | ((v << 4) as u8);
                    raw[o + 1] = (v >> 4) as u8;
                }
            }
            FsKind::Fat16 => raw[i * 2..i * 2 + 2].copy_from_slice(&(*v as u16).to_le_bytes()),
            _ => raw[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes()),
        }
    }
    for copy in 0..2 {
        let off = ((g.reserved + copy * g.fat_sectors) * bps) as usize;
        img[off..off + raw.len()].copy_from_slice(&raw);
    }
    img
}
