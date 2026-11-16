//! FAT12 / FAT16 / FAT32: walks the directory tree and lists entries whose
//! first byte was replaced by 0xE5 (deleted), including long file names.
//!
//! Deleting a file on FAT also clears its cluster chain, so the chain of a
//! deleted file is unknown. Like every FAT recovery tool, RescueForge assumes
//! the file was stored in one piece starting at its first cluster.

use super::{chance_of, join, AllocMap, Ctx, FsKind, VolumeInfo};
use crate::model::{Chance, Extent, FileData, FoundFile, Origin, Status};
use crate::source::Source;
use crate::util::{dos_to_unix, le16, le32};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Boot {
    pub kind: FsKind,
    pub bps: u64,
    /// Bytes per cluster.
    pub cluster: u64,
    pub total_sectors: u64,
    /// Byte offset and length of the first FAT.
    pub fat_start: u64,
    pub fat_bytes: u64,
    /// Byte offset and length of the fixed root directory (FAT12/16).
    pub root_start: u64,
    pub root_bytes: u64,
    pub root_cluster: u32,
    /// Byte offset of cluster 2.
    pub data_start: u64,
    /// Number of data clusters.
    pub clusters: u64,
}

pub fn parse_boot(b: &[u8]) -> Option<Boot> {
    if b.len() < 512 || b[510] != 0x55 || b[511] != 0xAA {
        return None;
    }
    if !(b[0] == 0xEB || b[0] == 0xE9) || &b[3..11] == b"NTFS    " || &b[3..11] == b"EXFAT   " {
        return None;
    }
    let bps = le16(b, 11)? as u64;
    if !bps.is_power_of_two() || !(512..=4096).contains(&bps) {
        return None;
    }
    let spc = b[13] as u64;
    if spc == 0 || !spc.is_power_of_two() {
        return None;
    }
    let reserved = le16(b, 14)? as u64;
    let nfats = b[16] as u64;
    let root_entries = le16(b, 17)? as u64;
    let total16 = le16(b, 19)? as u64;
    let fat16 = le16(b, 22)? as u64;
    let total32 = le32(b, 32)? as u64;
    if reserved == 0 || !(1..=2).contains(&nfats) || b[21] < 0xF0 {
        return None;
    }
    let fat_sectors = if fat16 != 0 {
        fat16
    } else {
        le32(b, 36)? as u64
    };
    let total_sectors = if total16 != 0 { total16 } else { total32 };
    if fat_sectors == 0 || total_sectors == 0 {
        return None;
    }
    let root_sectors = (root_entries * 32).div_ceil(bps);
    let meta = reserved + nfats * fat_sectors + root_sectors;
    let clusters = total_sectors.checked_sub(meta)? / spc;
    if clusters == 0 {
        return None;
    }
    let kind = if clusters < 4085 {
        FsKind::Fat12
    } else if clusters < 65525 {
        FsKind::Fat16
    } else {
        FsKind::Fat32
    };
    let bits = match kind {
        FsKind::Fat12 => 12,
        FsKind::Fat16 => 16,
        _ => 32,
    };
    // The FAT must be big enough to describe every cluster.
    if (clusters + 2) * bits / 8 > fat_sectors * bps {
        return None;
    }
    let root_cluster = if kind == FsKind::Fat32 {
        if root_entries != 0 || fat16 != 0 {
            return None;
        }
        let rc = le32(b, 44)?;
        if rc < 2 || rc as u64 >= clusters + 2 {
            return None;
        }
        rc
    } else {
        if root_entries == 0 || fat16 == 0 {
            return None;
        }
        0
    };
    Some(Boot {
        kind,
        bps,
        cluster: bps * spc,
        total_sectors,
        fat_start: reserved * bps,
        fat_bytes: fat_sectors * bps,
        root_start: (reserved + nfats * fat_sectors) * bps,
        root_bytes: root_entries * 32,
        root_cluster,
        data_start: meta * bps,
        clusters,
    })
}

/// Decodes the allocation table into one entry per cluster.
pub fn decode_fat(raw: &[u8], kind: FsKind, clusters: u64) -> Vec<u32> {
    let n = (clusters + 2) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let v = match kind {
            FsKind::Fat12 => {
                let o = i + i / 2;
                let Some(w) = le16(raw, o) else { break };
                (if i % 2 == 0 { w & 0x0FFF } else { w >> 4 }) as u32
            }
            FsKind::Fat16 => match le16(raw, i * 2) {
                Some(w) => w as u32,
                None => break,
            },
            _ => match le32(raw, i * 4) {
                Some(w) => w & 0x0FFF_FFFF,
                None => break,
            },
        };
        out.push(v);
    }
    out.resize(n, 0);
    out
}

const MAX_DIR_BYTES: usize = 8 << 20;
const MAX_DEPTH: usize = 40;
const MAX_FILES: u64 = 4_000_000;

struct Walker<'a, 'b, 'c> {
    vol: &'a Source,
    boot: &'a Boot,
    fat: Vec<u32>,
    alloc: AllocMap,
    cx: &'b mut Ctx<'c>,
    visited: HashSet<u32>,
    batch: Vec<FoundFile>,
    listed: u64,
    label: String,
}

impl Walker<'_, '_, '_> {
    fn valid(&self, c: u32) -> bool {
        c >= 2 && (c as u64) < self.boot.clusters + 2
    }

    fn offset(&self, c: u32) -> u64 {
        self.boot.data_start + (c as u64 - 2) * self.boot.cluster
    }

    fn eoc(&self, v: u32) -> bool {
        match self.boot.kind {
            FsKind::Fat12 => v >= 0xFF8,
            FsKind::Fat16 => v >= 0xFFF8,
            _ => v >= 0x0FFF_FFF8,
        }
    }

    /// The clusters of a live chain.
    fn chain(&self, first: u32, max: u64) -> Vec<u32> {
        let mut out = Vec::new();
        let mut c = first;
        while self.valid(c) && (out.len() as u64) < max {
            out.push(c);
            let next = self.fat[c as usize];
            if self.eoc(next) {
                break;
            }
            c = next;
        }
        out
    }

    fn extents_of(&self, clusters: &[u32], size: u64) -> Vec<Extent> {
        let mut out: Vec<Extent> = Vec::new();
        let mut left = size;
        for &c in clusters {
            if left == 0 {
                break;
            }
            let len = left.min(self.boot.cluster);
            let offset = self.vol.base() + self.offset(c);
            match out.last_mut() {
                Some(e) if e.offset + e.len == offset && e.len % self.boot.cluster == 0 => {
                    e.len += len
                }
                _ => out.push(Extent { offset, len }),
            }
            left -= len;
        }
        out
    }

    fn read_clusters(&self, clusters: &[u32]) -> Vec<u8> {
        let cl = self.boot.cluster as usize;
        let mut data = Vec::new();
        for &c in clusters {
            if data.len() + cl > MAX_DIR_BYTES {
                break;
            }
            let start = data.len();
            data.resize(start + cl, 0);
            self.vol.read_at(self.offset(c), &mut data[start..]);
        }
        data
    }

    /// The content of a deleted directory: its chain is gone, so clusters are
    /// read one after another while they are free and still look like a directory.
    fn read_deleted_dir(&self, first: u32) -> Option<Vec<u8>> {
        let cl = self.boot.cluster as usize;
        let mut data = vec![0u8; cl];
        self.vol.read_at(self.offset(first), &mut data);
        // A directory starts with the "." entry that points back at itself.
        let dot = &data[..32];
        if &dot[..11] != b".          " || dot[11] & 0x10 == 0 {
            return None;
        }
        let mut c = first;
        loop {
            let ended = data[data.len() - cl..].chunks_exact(32).any(|e| e[0] == 0);
            c += 1;
            if ended
                || !self.valid(c)
                || self.fat[c as usize] != 0
                || data.len() + cl > MAX_DIR_BYTES.min(64 * cl)
            {
                break;
            }
            let start = data.len();
            data.resize(start + cl, 0);
            self.vol.read_at(self.offset(c), &mut data[start..]);
        }
        Some(data)
    }

    fn flush(&mut self, force: bool) {
        if self.batch.len() >= 2000 || (force && !self.batch.is_empty()) {
            (self.cx.emit)(std::mem::take(&mut self.batch));
        }
    }

    fn walk(&mut self, data: &[u8], folder: &str, dir_deleted: bool, depth: usize) {
        if self.cx.ctl.checkpoint() || depth > MAX_DEPTH {
            return;
        }
        let fat32 = self.boot.kind == FsKind::Fat32;
        let vol_bytes = self.boot.clusters * self.boot.cluster;
        let mut lfn: Vec<[u16; 13]> = Vec::new();
        let mut lfn_deleted = false;
        let mut lfn_sum = 0u8;
        for e in data.chunks_exact(32) {
            if e[0] == 0 {
                break;
            }
            let attr = e[11];
            let deleted = e[0] == 0xE5;
            if attr & 0x3F == 0x0F {
                // One piece of a long name (stored last piece first).
                // All pieces of one name carry the same checksum of the short name.
                if (!deleted && e[0] & 0x40 != 0)
                    || lfn_deleted != deleted
                    || (!lfn.is_empty() && lfn_sum != e[13])
                {
                    lfn.clear();
                }
                lfn_deleted = deleted;
                lfn_sum = e[13];
                let mut part = [0u16; 13];
                for (i, o) in [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
                    .iter()
                    .enumerate()
                {
                    part[i] = u16::from_le_bytes([e[*o], e[*o + 1]]);
                }
                if lfn.len() < 20 {
                    lfn.push(part);
                }
                continue;
            }
            let long = std::mem::take(&mut lfn);
            if attr & 0xC0 != 0 || !short_name_ok(e, deleted) {
                continue;
            }
            if attr & 0x08 != 0 {
                if !deleted && self.label.is_empty() && depth == 0 {
                    self.label = String::from_utf8_lossy(&e[..11]).trim().to_string();
                }
                continue;
            }
            // For a deleted entry the checksum cannot be verified: the first
            // byte of the short name, which it covers, has been overwritten.
            let name = long_name(&long)
                .filter(|_| lfn_deleted == deleted && (deleted || lfn_sum == short_sum(e)))
                .unwrap_or_else(|| short_name(e, deleted));
            if name == "." || name == ".." || name.is_empty() {
                continue;
            }
            let first = le16(e, 26).unwrap_or(0) as u32
                | if fat32 {
                    (le16(e, 20).unwrap_or(0) as u32) << 16
                } else {
                    0
                };
            let size = le32(e, 28).unwrap_or(0) as u64;
            let gone = deleted || dir_deleted;
            if attr & 0x10 != 0 {
                if !self.valid(first) || !self.visited.insert(first) {
                    continue;
                }
                let sub = join(folder, &name);
                let content = if gone {
                    self.read_deleted_dir(first)
                } else {
                    Some(self.read_clusters(&self.chain(first, 1 << 16)))
                };
                if let Some(content) = content {
                    self.walk(&content, &sub, gone, depth + 1);
                }
                continue;
            }
            if size == 0 || size > vol_bytes || !self.valid(first) || self.listed >= MAX_FILES {
                continue;
            }
            let status = match (gone, self.cx.lost) {
                (true, _) => Status::Deleted,
                (false, true) => Status::Lost,
                (false, false) => Status::Existing,
            };
            if status == Status::Existing && !self.cx.include_existing {
                continue;
            }
            let need = size.div_ceil(self.boot.cluster);
            let (extents, chance, note) = if gone {
                // One piece, starting at the first cluster.
                let avail = (self.boot.clusters + 2 - first as u64).min(need);
                let list: Vec<u32> = (first..first + avail as u32).collect();
                let extents = self.extents_of(&list, size);
                let mut chance = chance_of(Some(&self.alloc), &extents);
                let mut note = String::new();
                if avail < need {
                    note = "The file would run past the end of the volume.".to_string();
                    chance = Chance::Poor;
                }
                (extents, chance, note)
            } else {
                let list = self.chain(first, need);
                let complete = list.len() as u64 == need;
                (
                    self.extents_of(&list, size),
                    if complete {
                        Chance::Good
                    } else {
                        Chance::Partial
                    },
                    if complete {
                        String::new()
                    } else {
                        "The cluster chain ends before the file does.".to_string()
                    },
                )
            };
            let tz = self.cx.tz as i64;
            let stamp = |date: usize, time: usize| {
                let t = dos_to_unix(le16(e, date).unwrap_or(0), le16(e, time).unwrap_or(0));
                if t == 0 {
                    0
                } else {
                    t - tz
                }
            };
            self.batch.push(FoundFile {
                name,
                path: self.cx.path(folder),
                size,
                modified: stamp(24, 22),
                created: stamp(16, 14),
                status,
                origin: Origin::Fat,
                chance,
                data: FileData::Extents(extents),
                note,
            });
            self.listed += 1;
            self.cx
                .progress
                .found
                .store(self.listed, std::sync::atomic::Ordering::Relaxed);
            self.flush(false);
        }
    }
}

fn short_sum(e: &[u8]) -> u8 {
    e[..11]
        .iter()
        .fold(0u8, |s, c| (s >> 1 | (s & 1) << 7).wrapping_add(*c))
}

fn short_name_ok(e: &[u8], deleted: bool) -> bool {
    let bad = |c: u8| c < 0x20 || b"\"*+,/:;<=>?[\\]|".contains(&c);
    let start = if deleted { 1 } else { 0 };
    if !deleted && (e[0] == b' ' || (e[0] == b'.' && e[1] != b' ' && e[1] != b'.')) {
        return false;
    }
    if e[0] == b'.' {
        // "." and ".." are the only names that may start with a dot.
        return e[2..11].iter().all(|c| *c == b' ');
    }
    !e[start..11].iter().any(|c| bad(*c))
}

fn short_name(e: &[u8], deleted: bool) -> String {
    let ch = |b: u8, lower: bool| {
        let c = b as char;
        if lower {
            c.to_ascii_lowercase()
        } else {
            c
        }
    };
    let (lower_base, lower_ext) = (e[12] & 0x08 != 0, e[12] & 0x10 != 0);
    let mut base: String = e[..8].iter().map(|b| ch(*b, lower_base)).collect();
    if deleted {
        // The first character was overwritten by the deletion mark.
        base.replace_range(..base.chars().next().map_or(0, |c| c.len_utf8()), "_");
    } else if e[0] == 0x05 {
        base.replace_range(..1, "\u{e5}");
    }
    let ext: String = e[8..11].iter().map(|b| ch(*b, lower_ext)).collect();
    let (base, ext) = (base.trim_end(), ext.trim_end());
    if ext.is_empty() {
        base.to_string()
    } else {
        format!("{base}.{ext}")
    }
}

fn long_name(parts: &[[u16; 13]]) -> Option<String> {
    if parts.is_empty() {
        return None;
    }
    let mut units: Vec<u16> = Vec::with_capacity(parts.len() * 13);
    for p in parts.iter().rev() {
        units.extend_from_slice(p);
    }
    if let Some(end) = units.iter().position(|u| *u == 0) {
        units.truncate(end);
    }
    while units.last() == Some(&0xFFFF) {
        units.pop();
    }
    let name = String::from_utf16(&units).ok()?;
    if name.is_empty()
        || name
            .chars()
            .any(|c| (c as u32) < 0x20 || "\"*/:<>?\\|".contains(c))
    {
        return None;
    }
    Some(name)
}

/// Lists the files of a FAT volume.
pub fn scan(vol: &Source, boot: &Boot, cx: &mut Ctx) -> Result<VolumeInfo, String> {
    cx.progress.set_phase("Reading the FAT directory tree");
    // Never trust the boot sector further than the source reaches.
    let mut boot = boot.clone();
    boot.clusters = boot
        .clusters
        .min(vol.len().saturating_sub(boot.data_start) / boot.cluster);
    if boot.clusters == 0
        || (boot.kind == FsKind::Fat32 && boot.root_cluster as u64 >= boot.clusters + 2)
    {
        return Err("The volume is cut off before its data area.".to_string());
    }
    let boot = &boot;
    let bits = match boot.kind {
        FsKind::Fat12 => 12,
        FsKind::Fat16 => 16,
        _ => 32,
    };
    let need = ((boot.clusters + 2) * bits / 8 + 2).min(boot.fat_bytes);
    if need > 1 << 30 {
        return Err("The allocation table of this volume is too large to load.".to_string());
    }
    let raw = vol.read_vec(boot.fat_start, need as usize);
    let fat = decode_fat(&raw, boot.kind, boot.clusters);
    let mut bitmap = vec![0u8; (boot.clusters as usize).div_ceil(8)];
    for (i, v) in fat.iter().enumerate().skip(2) {
        if *v != 0 {
            bitmap[(i - 2) / 8] |= 1 << ((i - 2) % 8);
        }
    }
    let alloc = AllocMap::new(
        vol.base(),
        vol.base() + vol.len(),
        vol.base() + boot.data_start,
        boot.cluster,
        boot.clusters,
        bitmap,
    );
    let mut w = Walker {
        vol,
        boot,
        fat,
        alloc,
        cx,
        visited: HashSet::new(),
        batch: Vec::new(),
        listed: 0,
        label: String::new(),
    };
    let root = if boot.kind == FsKind::Fat32 {
        w.visited.insert(boot.root_cluster);
        w.read_clusters(&w.chain(boot.root_cluster, 1 << 16))
    } else {
        vol.read_vec(
            boot.root_start,
            (boot.root_bytes as usize).min(MAX_DIR_BYTES),
        )
    };
    w.walk(&root, "", false, 0);
    w.flush(true);
    Ok(VolumeInfo {
        kind: boot.kind,
        label: w.label,
        cluster: boot.cluster,
        alloc: Some(w.alloc),
        reserved: Vec::new(),
        listed: w.listed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::Rng;

    #[test]
    fn fat12_entries_decode() {
        // Entries 0..4 of a FAT12: F8F, FFF, 003, FFF.
        let raw = [0x8F, 0xFF, 0xFF, 0x03, 0xF0, 0xFF];
        let fat = decode_fat(&raw, FsKind::Fat12, 2);
        assert_eq!(fat, vec![0xF8F, 0xFFF, 0x003, 0xFFF]);
        // A short table is padded with "free", never read out of bounds.
        assert_eq!(
            decode_fat(&raw[..2], FsKind::Fat16, 3),
            vec![0xFF8F, 0, 0, 0, 0]
        );
        assert_eq!(decode_fat(&[], FsKind::Fat32, 1), vec![0, 0, 0]);
    }

    #[test]
    fn names() {
        let mut e = [b' '; 32];
        e[..8].copy_from_slice(b"REPORT  ");
        e[8..11].copy_from_slice(b"TXT");
        e[11] = 0x20;
        e[12] = 0x18;
        assert!(short_name_ok(&e, false));
        assert_eq!(short_name(&e, false), "report.txt");
        e[12] = 0;
        e[0] = 0xE5;
        assert_eq!(short_name(&e, true), "_EPORT.TXT");
        e[3] = b'*';
        assert!(!short_name_ok(&e, true));

        let mut part = [0xFFFFu16; 13];
        for (i, c) in "Summer.jpeg".encode_utf16().enumerate() {
            part[i] = c;
        }
        part[11] = 0;
        assert_eq!(long_name(&[part]).as_deref(), Some("Summer.jpeg"));
        assert_eq!(long_name(&[]), None);
        assert_eq!(long_name(&[[0xD800u16; 13]]), None, "unpaired surrogates");
        assert_eq!(long_name(&[[b'?' as u16; 13]]), None);
    }

    #[test]
    fn boot_validation() {
        for kind in [FsKind::Fat12, FsKind::Fat16, FsKind::Fat32] {
            let img = crate::testimg::fat::build(kind, &crate::testimg::sample_tree());
            let boot = parse_boot(&img[..512]).expect("valid");
            assert_eq!(boot.kind, kind);
            let mut bad = img[..512].to_vec();
            bad[13] = 0;
            assert!(parse_boot(&bad).is_none());
            let mut bad = img[..512].to_vec();
            bad[511] = 0;
            assert!(parse_boot(&bad).is_none());
            let mut bad = img[..512].to_vec();
            bad[16] = 9;
            assert!(parse_boot(&bad).is_none());
        }
        let mut rng = Rng(7);
        for _ in 0..20_000 {
            let mut b = rng.bytes(512);
            b[0] = 0xEB;
            b[510] = 0x55;
            b[511] = 0xAA;
            b[11] = 0;
            b[12] = 2;
            b[13] = 1 << rng.below(8);
            b[16] = 1 + rng.below(2) as u8;
            b[21] = 0xF8;
            let _ = parse_boot(&b);
        }
    }
}
