//! Builds an exFAT image from a [`Tree`].

use super::{Reuse, Tree};
use crate::util::civil;
use std::collections::HashMap;

const BPS: usize = 512;
const SPC: usize = 8;
const CLUSTER: usize = BPS * SPC;
const TOTAL_SECTORS: usize = 32_768;
const FAT_OFFSET: usize = 128;
const FAT_SECTORS: usize = 64;
const HEAP_OFFSET: usize = 256;
const CLUSTERS: usize = (TOTAL_SECTORS - HEAP_OFFSET) / SPC;

fn stamp(unix: i64) -> u32 {
    let (y, m, d, h, mi, s) = civil(unix);
    (((y - 1980).clamp(0, 127) as u32) << 25)
        | ((m as u32) << 21)
        | ((d as u32) << 16)
        | ((h as u32) << 11)
        | ((mi as u32) << 5)
        | (s as u32 / 2)
}

/// One entry set: file entry, stream extension, name entries.
#[allow(clippy::too_many_arguments)]
fn entry_set(
    name: &str,
    dir: bool,
    first: u32,
    size: u64,
    contiguous: bool,
    mtime: i64,
    deleted: bool,
) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let name_entries = units.len().div_ceil(15);
    let mut out = vec![0u8; 32 * (2 + name_entries)];
    out[0] = 0x85;
    out[1] = (1 + name_entries) as u8;
    out[4..6].copy_from_slice(&(if dir { 0x10u16 } else { 0x20 }).to_le_bytes());
    let t = stamp(mtime);
    out[8..12].copy_from_slice(&t.to_le_bytes());
    out[12..16].copy_from_slice(&t.to_le_bytes());
    out[16..20].copy_from_slice(&t.to_le_bytes());
    // UTC offset fields: "valid, +00:00".
    out[22] = 0x80;
    out[23] = 0x80;
    out[24] = 0x80;
    let s = &mut out[32..64];
    s[0] = 0xC0;
    s[1] = if contiguous { 0x03 } else { 0x01 };
    s[3] = units.len() as u8;
    s[8..16].copy_from_slice(&size.to_le_bytes());
    s[20..24].copy_from_slice(&first.to_le_bytes());
    s[24..32].copy_from_slice(&size.to_le_bytes());
    for (k, chunk) in units.chunks(15).enumerate() {
        let n = &mut out[64 + k * 32..96 + k * 32];
        n[0] = 0xC1;
        for (i, u) in chunk.iter().enumerate() {
            n[2 + i * 2..4 + i * 2].copy_from_slice(&u.to_le_bytes());
        }
    }
    if deleted {
        for e in out.chunks_mut(32) {
            e[0] &= 0x7F;
        }
    }
    out
}

pub fn build(tree: &Tree) -> Vec<u8> {
    let mut img = vec![0u8; TOTAL_SECTORS * BPS];
    let mut fat = vec![0u32; CLUSTERS + 2];
    let mut bitmap = vec![0u8; CLUSTERS.div_ceil(8)];
    fat[0] = 0xFFFF_FFF8;
    fat[1] = 0xFFFF_FFFF;
    let mut next = 2u32;
    let off = |c: u32| HEAP_OFFSET * BPS + (c as usize - 2) * CLUSTER;
    let set_bits = |bitmap: &mut Vec<u8>, first: u32, n: usize, on: bool| {
        for c in first..first + n as u32 {
            let i = c as usize - 2;
            if on {
                bitmap[i / 8] |= 1 << (i % 8);
            } else {
                bitmap[i / 8] &= !(1 << (i % 8));
            }
        }
    };
    let mut take = |n: usize| -> u32 {
        let first = next;
        next += n as u32;
        first
    };
    let chain = |fat: &mut Vec<u32>, clusters: &[u32]| {
        for w in clusters.windows(2) {
            fat[w[0] as usize] = w[1];
        }
        if let Some(last) = clusters.last() {
            fat[*last as usize] = 0xFFFF_FFFF;
        }
    };

    // Cluster 2: allocation bitmap. Cluster 3: up-case table (not needed to read files).
    let bitmap_cluster = take(1);
    let upcase_cluster = take(1);
    // The root directory is given two clusters linked through the FAT.
    let root_clusters = [take(1), take(1)];
    for c in [
        bitmap_cluster,
        upcase_cluster,
        root_clusters[0],
        root_clusters[1],
    ] {
        set_bits(&mut bitmap, c, 1, true);
    }
    chain(&mut fat, &[bitmap_cluster]);
    chain(&mut fat, &[upcase_cluster]);
    chain(&mut fat, &root_clusters);

    let mut root = vec![0u8; 96];
    root[0] = 0x83;
    root[1] = 6;
    for (i, u) in "RESCUE".encode_utf16().enumerate() {
        root[2 + i * 2..4 + i * 2].copy_from_slice(&u.to_le_bytes());
    }
    root[32] = 0x81;
    root[52..56].copy_from_slice(&bitmap_cluster.to_le_bytes());
    root[56..64].copy_from_slice(&(bitmap.len() as u64).to_le_bytes());
    root[64] = 0x82;
    root[84..88].copy_from_slice(&upcase_cluster.to_le_bytes());
    root[88..96].copy_from_slice(&128u64.to_le_bytes());

    let dirs = tree.dirs();
    let mut content: HashMap<String, Vec<u8>> = HashMap::new();
    let mut dir_first: HashMap<String, u32> = HashMap::new();
    content.insert(String::new(), root);
    for d in &dirs {
        let c = take(1);
        set_bits(&mut bitmap, c, 1, !tree.dir_deleted(d));
        dir_first.insert(d.clone(), c);
        content.insert(d.clone(), Vec::new());
    }
    for d in &dirs {
        let (parent, name) = d.rsplit_once('/').unwrap_or(("", d));
        let e = entry_set(
            name,
            true,
            dir_first[d],
            CLUSTER as u64,
            true,
            1_740_000_000,
            tree.dir_deleted(d),
        );
        content
            .get_mut(parent)
            .expect("parent")
            .extend_from_slice(&e);
    }
    for f in &tree.files {
        let gone = tree.is_deleted(f);
        let n = f.data.len().div_ceil(CLUSTER).max(1);
        // A fragmented file leaves one cluster (used by something else) between its halves.
        let clusters: Vec<u32> = if f.fragmented && n >= 2 {
            let a = take(n / 2);
            let gap = take(1);
            set_bits(&mut bitmap, gap, 1, true);
            chain(&mut fat, &[gap]);
            let b = take(n - n / 2);
            (a..a + (n / 2) as u32)
                .chain(b..b + (n - n / 2) as u32)
                .collect()
        } else {
            let a = take(n);
            (a..a + n as u32).collect()
        };
        let contiguous = !(f.fragmented && n >= 2);
        if !contiguous {
            // The chain stays in the table even after the file is deleted.
            chain(&mut fat, &clusters);
        }
        for (i, c) in clusters.iter().enumerate() {
            let piece =
                &f.data[(i * CLUSTER).min(f.data.len())..((i + 1) * CLUSTER).min(f.data.len())];
            img[off(*c)..off(*c) + piece.len()].copy_from_slice(piece);
            set_bits(&mut bitmap, *c, 1, !gone);
        }
        let reused = match f.reuse {
            Reuse::None => 0..0,
            Reuse::Tail => n / 2..n,
            Reuse::All => 0..n,
        };
        for i in reused {
            set_bits(&mut bitmap, clusters[i], 1, true);
            img[off(clusters[i])..off(clusters[i]) + CLUSTER].fill(0xEE);
        }
        let e = entry_set(
            f.name(),
            false,
            clusters[0],
            f.data.len() as u64,
            contiguous,
            f.mtime,
            gone,
        );
        content
            .get_mut(f.folder())
            .expect("folder")
            .extend_from_slice(&e);
    }
    assert!(
        (next as usize) < CLUSTERS,
        "sample tree does not fit the image"
    );

    for (d, bytes) in &content {
        if d.is_empty() {
            assert!(bytes.len() < 2 * CLUSTER, "root directory overflows");
            let (a, b) = bytes.split_at(bytes.len().min(CLUSTER));
            img[off(root_clusters[0])..off(root_clusters[0]) + a.len()].copy_from_slice(a);
            img[off(root_clusters[1])..off(root_clusters[1]) + b.len()].copy_from_slice(b);
        } else {
            assert!(bytes.len() < CLUSTER, "directory {d} overflows");
            let o = off(dir_first[d]);
            img[o..o + bytes.len()].copy_from_slice(bytes);
        }
    }
    let o = off(bitmap_cluster);
    img[o..o + bitmap.len()].copy_from_slice(&bitmap);
    for (i, v) in fat.iter().enumerate() {
        let o = FAT_OFFSET * BPS + i * 4;
        img[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    // Main boot sector and its backup (sector 12).
    let mut b = [0u8; 512];
    b[..3].copy_from_slice(&[0xEB, 0x76, 0x90]);
    b[3..11].copy_from_slice(b"EXFAT   ");
    b[72..80].copy_from_slice(&(TOTAL_SECTORS as u64).to_le_bytes());
    b[80..84].copy_from_slice(&(FAT_OFFSET as u32).to_le_bytes());
    b[84..88].copy_from_slice(&(FAT_SECTORS as u32).to_le_bytes());
    b[88..92].copy_from_slice(&(HEAP_OFFSET as u32).to_le_bytes());
    b[92..96].copy_from_slice(&(CLUSTERS as u32).to_le_bytes());
    b[96..100].copy_from_slice(&root_clusters[0].to_le_bytes());
    b[100..104].copy_from_slice(&0x1234_ABCDu32.to_le_bytes());
    b[104..106].copy_from_slice(&0x0100u16.to_le_bytes());
    b[108] = 9;
    b[109] = 3;
    b[110] = 1;
    b[111] = 0x80;
    b[510] = 0x55;
    b[511] = 0xAA;
    img[..512].copy_from_slice(&b);
    img[12 * 512..13 * 512].copy_from_slice(&b);
    img
}
