//! exFAT: walks the directory tree and lists entry sets whose "in use" bit
//! was cleared (deleted files).
//!
//! exFAT marks a file as stored in one piece ("no FAT chain") or keeps a
//! chain in the allocation table. Deleting a file clears its bits in the
//! allocation bitmap but usually leaves the chain alone, so a deleted
//! fragmented file can often still be followed.

use super::{chance_of, join, AllocMap, Ctx, FsKind, VolumeInfo};
use crate::model::{Chance, Extent, FileData, FoundFile, Origin, Status};
use crate::source::Source;
use crate::util::{dos_to_unix, le16, le32, le64, utf16le};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Boot {
    pub bps: u64,
    /// Bytes per cluster.
    pub cluster: u64,
    pub fat_start: u64,
    pub fat_bytes: u64,
    /// Byte offset of cluster 2.
    pub heap_start: u64,
    pub clusters: u64,
    pub root_cluster: u32,
    pub volume_bytes: u64,
}

pub fn parse_boot(b: &[u8]) -> Option<Boot> {
    if b.len() < 512
        || b[..3] != [0xEB, 0x76, 0x90]
        || &b[3..11] != b"EXFAT   "
        || b[510] != 0x55
        || b[511] != 0xAA
        || b[11..64].iter().any(|x| *x != 0)
    {
        return None;
    }
    let bps_shift = b[108] as u32;
    let spc_shift = b[109] as u32;
    if !(9..=12).contains(&bps_shift) || spc_shift > 25 - bps_shift {
        return None;
    }
    let bps = 1u64 << bps_shift;
    let cluster = bps << spc_shift;
    let fat_offset = le32(b, 80)? as u64;
    let fat_length = le32(b, 84)? as u64;
    let heap_offset = le32(b, 88)? as u64;
    let clusters = le32(b, 92)? as u64;
    let root_cluster = le32(b, 96)?;
    let volume_sectors = le64(b, 72)?;
    if fat_offset < 24
        || fat_length == 0
        || heap_offset < fat_offset + fat_length
        || clusters == 0
        || root_cluster < 2
        || root_cluster as u64 >= clusters + 2
        || (clusters + 2) * 4 > fat_length * bps
    {
        return None;
    }
    let volume_bytes = volume_sectors.checked_mul(bps)?;
    if heap_offset * bps + clusters * cluster > volume_bytes {
        return None;
    }
    Some(Boot {
        bps,
        cluster,
        fat_start: fat_offset * bps,
        fat_bytes: fat_length * bps,
        heap_start: heap_offset * bps,
        clusters,
        root_cluster,
        volume_bytes,
    })
}

const MAX_DIR_BYTES: u64 = 16 << 20;
const MAX_DEPTH: usize = 40;
const MAX_FILES: u64 = 4_000_000;

struct Walker<'a, 'b, 'c> {
    vol: &'a Source,
    boot: &'a Boot,
    fat: Vec<u32>,
    alloc: Option<AllocMap>,
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
        self.boot.heap_start + (c as u64 - 2) * self.boot.cluster
    }

    /// Follows the allocation table. `None` when the chain is broken or does
    /// not have exactly `need` clusters.
    fn chain(&self, first: u32, need: u64, exact: bool) -> Option<Vec<u32>> {
        let mut out = Vec::new();
        let mut c = first;
        loop {
            if !self.valid(c) {
                return None;
            }
            out.push(c);
            if out.len() as u64 > self.boot.clusters {
                // A loop in the table.
                return None;
            }
            let next = *self.fat.get(c as usize)?;
            if out.len() as u64 == need {
                return if !exact || next >= 0xFFFF_FFF8 {
                    Some(out)
                } else {
                    None
                };
            }
            if next >= 0xFFFF_FFF8 {
                return if exact { None } else { Some(out) };
            }
            c = next;
        }
    }

    fn contiguous(&self, first: u32, need: u64) -> Vec<u32> {
        let avail = (self.boot.clusters + 2)
            .saturating_sub(first as u64)
            .min(need);
        (first..first + avail as u32).collect()
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

    fn read_clusters(&self, clusters: &[u32], len: u64) -> Vec<u8> {
        let cl = self.boot.cluster as usize;
        let want = len.min(MAX_DIR_BYTES) as usize;
        let mut data = Vec::new();
        for &c in clusters {
            if data.len() >= want {
                break;
            }
            let start = data.len();
            data.resize(start + cl, 0);
            self.vol.read_at(self.offset(c), &mut data[start..]);
        }
        data.truncate(want);
        data
    }

    /// The clusters of a stream. Deleted streams fall back to "one piece"
    /// when the chain cannot be followed.
    fn clusters_of(&self, first: u32, size: u64, no_chain: bool, gone: bool) -> (Vec<u32>, bool) {
        let need = size.div_ceil(self.boot.cluster).max(1);
        if !self.valid(first) {
            return (Vec::new(), false);
        }
        if no_chain {
            let list = self.contiguous(first, need);
            let ok = list.len() as u64 == need;
            return (list, ok);
        }
        match self.chain(first, need, gone) {
            Some(list) => {
                let ok = list.len() as u64 == need;
                (list, ok)
            }
            None => {
                let list = self.contiguous(first, need);
                let ok = list.len() as u64 == need;
                (list, ok)
            }
        }
    }

    fn flush(&mut self, force: bool) {
        if self.batch.len() >= 2000 || (force && !self.batch.is_empty()) {
            (self.cx.emit)(std::mem::take(&mut self.batch));
        }
    }

    fn stamp(&self, e: &[u8], field: usize, tz_field: usize) -> i64 {
        let v = le32(e, field).unwrap_or(0);
        let t = dos_to_unix((v >> 16) as u16, v as u16);
        if t == 0 {
            return 0;
        }
        let tzb = e[tz_field];
        if tzb & 0x80 != 0 {
            // Offset from UTC in quarter hours, 7-bit two's complement.
            let q = ((tzb & 0x7F) as i8) << 1 >> 1;
            t - q as i64 * 900
        } else {
            t - self.cx.tz as i64
        }
    }

    fn walk(&mut self, data: &[u8], folder: &str, dir_deleted: bool, depth: usize) {
        if self.cx.ctl.checkpoint() || depth > MAX_DEPTH {
            return;
        }
        let mut i = 0usize;
        while i + 32 <= data.len() {
            let t = data[i];
            if t == 0 {
                break;
            }
            if t == 0x83 && depth == 0 && self.label.is_empty() {
                let n = (data[i + 1] as usize).min(11);
                self.label = utf16le(&data[i + 2..i + 2 + n * 2]);
            }
            if t & 0x7F != 0x05 {
                i += 32;
                continue;
            }
            let deleted = t & 0x80 == 0;
            let count = data[i + 1] as usize;
            let set_end = i + 32 * (count + 1);
            if !(2..=18).contains(&count) || set_end > data.len() {
                i += 32;
                continue;
            }
            let e = &data[i..i + 32];
            let s = &data[i + 32..i + 64];
            if s[0] & 0x7F != 0x40 || (s[0] & 0x80 == 0) != deleted {
                i += 32;
                continue;
            }
            let name_len = s[3] as usize;
            let mut units: Vec<u16> = Vec::with_capacity(name_len);
            for k in 2..=count {
                let n = &data[i + 32 * k..i + 32 * (k + 1)];
                if n[0] & 0x7F != 0x41 {
                    break;
                }
                units.extend(
                    n[2..32]
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]])),
                );
            }
            i = set_end;
            units.truncate(name_len);
            let name = String::from_utf16_lossy(&units);
            if name.is_empty()
                || units.len() < name_len
                || name
                    .chars()
                    .any(|c| (c as u32) < 0x20 || "\"*/:<>?\\|".contains(c))
            {
                continue;
            }
            let attr = le16(e, 4).unwrap_or(0);
            let no_chain = s[1] & 0x02 != 0;
            let first = le32(s, 20).unwrap_or(0);
            let size = le64(s, 24).unwrap_or(0);
            let gone = deleted || dir_deleted;
            if size > self.boot.volume_bytes {
                continue;
            }
            if attr & 0x10 != 0 {
                if !self.valid(first) || size == 0 || !self.visited.insert(first) {
                    continue;
                }
                let (list, _) = self.clusters_of(first, size, no_chain, gone);
                let content = self.read_clusters(&list, size);
                let sub = join(folder, &name);
                self.walk(&content, &sub, gone, depth + 1);
                continue;
            }
            if size == 0 || self.listed >= MAX_FILES {
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
            let (list, complete) = self.clusters_of(first, size, no_chain, gone);
            let extents = self.extents_of(&list, size);
            let (chance, note) = if list.is_empty() {
                (
                    Chance::Poor,
                    "The entry no longer says where the content was stored.".to_string(),
                )
            } else if !complete {
                (
                    Chance::Partial,
                    "Only part of the content could be located.".to_string(),
                )
            } else if gone {
                (chance_of(self.alloc.as_ref(), &extents), String::new())
            } else {
                (Chance::Good, String::new())
            };
            self.batch.push(FoundFile {
                name,
                path: self.cx.path(folder),
                size,
                modified: self.stamp(e, 12, 23),
                created: self.stamp(e, 8, 22),
                status,
                origin: Origin::ExFat,
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

/// Lists the files of an exFAT volume.
pub fn scan(vol: &Source, boot: &Boot, cx: &mut Ctx) -> Result<VolumeInfo, String> {
    cx.progress.set_phase("Reading the exFAT directory tree");
    // Never trust the boot sector further than the source reaches.
    let mut boot = *boot;
    boot.clusters = boot
        .clusters
        .min(vol.len().saturating_sub(boot.heap_start) / boot.cluster);
    if (boot.root_cluster as u64) >= boot.clusters + 2 {
        return Err("The volume is cut off before its root folder.".to_string());
    }
    let boot = &boot;
    let need = ((boot.clusters + 2) * 4).min(boot.fat_bytes);
    if need > 1 << 30 {
        return Err("The allocation table of this volume is too large to load.".to_string());
    }
    let raw = vol.read_vec(boot.fat_start, need as usize);
    let mut fat: Vec<u32> = raw
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    fat.resize((boot.clusters + 2) as usize, 0);
    let mut w = Walker {
        vol,
        boot,
        fat,
        alloc: None,
        cx,
        visited: HashSet::new(),
        batch: Vec::new(),
        listed: 0,
        label: String::new(),
    };
    w.visited.insert(boot.root_cluster);
    let root_list = w
        .chain(boot.root_cluster, MAX_DIR_BYTES / boot.cluster + 1, false)
        .unwrap_or_else(|| vec![boot.root_cluster]);
    let root = w.read_clusters(&root_list, MAX_DIR_BYTES);

    // The allocation bitmap is described by an entry in the root directory.
    for e in root.chunks_exact(32) {
        if e[0] == 0 {
            break;
        }
        if e[0] == 0x81 && e[1] & 1 == 0 {
            let first = le32(e, 20).unwrap_or(0);
            let len = le64(e, 24).unwrap_or(0);
            if w.valid(first) && len > 0 && len <= boot.clusters.div_ceil(8) + boot.cluster {
                let need = len.div_ceil(boot.cluster);
                let list = w
                    .chain(first, need, false)
                    .unwrap_or_else(|| w.contiguous(first, need));
                let cl = boot.cluster as usize;
                let mut bits = Vec::with_capacity(len as usize);
                for c in list {
                    let start = bits.len();
                    bits.resize(start + cl, 0);
                    vol.read_at(w.offset(c), &mut bits[start..]);
                }
                bits.truncate(len as usize);
                w.alloc = Some(AllocMap::new(
                    vol.base(),
                    vol.base() + vol.len(),
                    vol.base() + boot.heap_start,
                    boot.cluster,
                    boot.clusters,
                    bits,
                ));
            }
            break;
        }
    }
    if w.alloc.is_none() {
        (w.cx.log)(
            "The exFAT allocation bitmap could not be read, so recoverability cannot be rated."
                .to_string(),
        );
    }
    w.walk(&root, "", false, 0);
    w.flush(true);
    Ok(VolumeInfo {
        kind: FsKind::ExFat,
        label: w.label,
        cluster: boot.cluster,
        alloc: w.alloc,
        reserved: Vec::new(),
        listed: w.listed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::Rng;

    #[test]
    fn boot_validation() {
        let img = crate::testimg::exfat::build(&crate::testimg::sample_tree());
        let boot = parse_boot(&img[..512]).expect("valid");
        assert_eq!(boot.cluster, 4096);
        assert!(crate::fs::fat::parse_boot(&img[..512]).is_none());
        assert!(crate::fs::ntfs::parse_boot(&img[..512]).is_none());
        let mut bad = img[..512].to_vec();
        bad[108] = 40; // sector size 2^40
        assert!(parse_boot(&bad).is_none());
        let mut bad = img[..512].to_vec();
        bad[20] = 1; // the old BPB area must be zero
        assert!(parse_boot(&bad).is_none());
        let mut bad = img[..512].to_vec();
        bad[96..100].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_boot(&bad).is_none());

        let mut rng = Rng(11);
        for _ in 0..20_000 {
            let mut b = img[..512].to_vec();
            // Random values in every field that is used for arithmetic.
            for o in (72..112).step_by(4) {
                if rng.below(3) == 0 {
                    let v = rng.next_u64() as u32;
                    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            let _ = parse_boot(&b);
        }
    }
}
