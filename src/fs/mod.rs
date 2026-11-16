//! File system readers. Each one lists deleted (and optionally existing)
//! files from the structures the file system keeps about itself.

pub mod exfat;
pub mod fat;
pub mod ntfs;

use crate::control::{Control, Progress};
use crate::model::{Chance, Extent, FoundFile};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsKind {
    Ntfs,
    Fat12,
    Fat16,
    Fat32,
    ExFat,
}

impl FsKind {
    pub fn label(self) -> &'static str {
        match self {
            FsKind::Ntfs => "NTFS",
            FsKind::Fat12 => "FAT12",
            FsKind::Fat16 => "FAT16",
            FsKind::Fat32 => "FAT32",
            FsKind::ExFat => "exFAT",
        }
    }
}

/// Which file system starts with this boot sector, if any.
pub fn detect(sector: &[u8]) -> Option<FsKind> {
    if ntfs::parse_boot(sector).is_some() {
        Some(FsKind::Ntfs)
    } else if exfat::parse_boot(sector).is_some() {
        Some(FsKind::ExFat)
    } else {
        fat::parse_boot(sector).map(|b| b.kind)
    }
}

/// Which clusters of a volume are in use right now.
#[derive(Clone, Debug)]
pub struct AllocMap {
    /// Absolute byte range of the volume on the source.
    pub vol_start: u64,
    pub vol_end: u64,
    /// Absolute offset of cluster index 0.
    pub base: u64,
    pub cluster: u64,
    pub count: u64,
    bits: Vec<u8>,
}

impl AllocMap {
    pub fn new(
        vol_start: u64,
        vol_end: u64,
        base: u64,
        cluster: u64,
        count: u64,
        mut bits: Vec<u8>,
    ) -> AllocMap {
        bits.resize(count.div_ceil(8) as usize, 0);
        AllocMap {
            vol_start,
            vol_end,
            base,
            cluster: cluster.max(1),
            count,
            bits,
        }
    }

    /// Is cluster `index` allocated? `None` outside the map.
    pub fn get(&self, index: u64) -> Option<bool> {
        if index >= self.count {
            return None;
        }
        Some(self.bits[(index / 8) as usize] & (1 << (index % 8)) != 0)
    }

    /// Is the cluster holding absolute byte `abs` in use? Areas of the volume
    /// outside the cluster heap (boot sectors, FATs) count as in use; bytes
    /// outside the volume give `None`.
    pub fn byte_alloc(&self, abs: u64) -> Option<bool> {
        if abs < self.vol_start || abs >= self.vol_end {
            return None;
        }
        if abs < self.base {
            return Some(true);
        }
        Some(self.get((abs - self.base) / self.cluster).unwrap_or(false))
    }

    /// Does `[abs, abs+len)` contain anything that is not in use?
    pub fn any_free(&self, abs: u64, len: u64) -> bool {
        let end = abs.saturating_add(len);
        if abs < self.vol_start || end > self.vol_end {
            return true;
        }
        let from = abs.max(self.base);
        if from >= end {
            return false;
        }
        let first = (from - self.base) / self.cluster;
        let last = (end - 1 - self.base) / self.cluster;
        (first..=last).any(|i| !self.get(i).unwrap_or(false))
    }

    /// `(allocated, total, first_allocated)` over the clusters of some extents.
    pub fn usage(&self, extents: &[Extent]) -> (u64, u64, bool) {
        let (mut used, mut total, mut first) = (0u64, 0u64, false);
        for e in extents {
            if e.is_sparse() || e.len == 0 {
                continue;
            }
            let mut pos = e.offset;
            let end = e.offset.saturating_add(e.len);
            while pos < end {
                let a = self.byte_alloc(pos).unwrap_or(false);
                if a {
                    used += 1;
                    if total == 0 {
                        first = true;
                    }
                }
                total += 1;
                pos = pos - (pos.saturating_sub(self.base) % self.cluster) + self.cluster;
            }
        }
        (used, total, first)
    }
}

/// Rates a deleted file by how much of its old space is in use again.
pub fn chance_of(alloc: Option<&AllocMap>, extents: &[Extent]) -> Chance {
    let Some(map) = alloc else {
        return Chance::Unknown;
    };
    let (used, total, first) = map.usage(extents);
    if total == 0 {
        Chance::Unknown
    } else if used == 0 {
        Chance::Good
    } else if first || used == total {
        Chance::Poor
    } else {
        Chance::Partial
    }
}

/// Everything a volume reader needs from its caller.
pub struct Ctx<'a> {
    pub ctl: &'a Control,
    pub progress: &'a Progress,
    /// Also list files that still exist.
    pub include_existing: bool,
    /// The volume is a lost partition: its existing files are what the user
    /// is looking for, so they are always listed (as "Lost").
    pub lost: bool,
    /// Local offset from UTC in seconds (FAT stores local times).
    pub tz: i32,
    /// Prepended to every folder ("" for none).
    pub prefix: &'a str,
    pub emit: &'a mut dyn FnMut(Vec<FoundFile>),
    pub log: &'a mut dyn FnMut(String),
}

impl Ctx<'_> {
    pub fn path(&self, folder: &str) -> String {
        join(self.prefix, folder)
    }
}

pub fn join(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_string(),
        (_, true) => a.to_string(),
        _ => format!("{a}/{b}"),
    }
}

/// What a volume reader learned about the volume as a whole.
#[derive(Clone, Debug)]
pub struct VolumeInfo {
    pub kind: FsKind,
    pub label: String,
    pub cluster: u64,
    pub alloc: Option<AllocMap>,
    /// Absolute areas that hold file system records, not file content
    /// (the NTFS master file table): the deep scan does not carve inside them.
    pub reserved: Vec<Extent>,
    pub listed: u64,
}

/// Reads a volume whose first sector is `sector`. `None` when no supported
/// file system starts there.
pub fn scan_volume(
    vol: &crate::source::Source,
    sector: &[u8],
    cx: &mut Ctx,
) -> Option<Result<VolumeInfo, String>> {
    if let Some(boot) = ntfs::parse_boot(sector) {
        Some(ntfs::scan(vol, &boot, cx))
    } else if let Some(boot) = exfat::parse_boot(sector) {
        Some(exfat::scan(vol, &boot, cx))
    } else {
        fat::parse_boot(sector).map(|boot| fat::scan(vol, &boot, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_map_bounds_and_usage() {
        // Volume at 1000..1000+100*10, heap starts 20 bytes in, 10-byte clusters.
        let map = AllocMap::new(1000, 2000, 1020, 10, 50, vec![0b0000_0101, 0xFF]);
        assert_eq!(map.get(0), Some(true));
        assert_eq!(map.get(1), Some(false));
        assert_eq!(map.get(2), Some(true));
        assert_eq!(map.get(49), Some(false));
        assert_eq!(map.get(50), None);
        assert_eq!(map.byte_alloc(999), None);
        assert_eq!(map.byte_alloc(1005), Some(true), "system area");
        assert_eq!(map.byte_alloc(1020), Some(true));
        assert_eq!(map.byte_alloc(1035), Some(false));
        assert_eq!(map.byte_alloc(1999), Some(false), "slack after the heap");
        assert_eq!(map.byte_alloc(2000), None);
        assert!(map.any_free(1030, 10));
        assert!(!map.any_free(1100, 80), "clusters 8..16 are all in use");
        assert!(!map.any_free(1000, 20));
        assert!(
            map.any_free(900, 200),
            "outside the volume nothing is known"
        );

        let free = [Extent {
            offset: 1030,
            len: 10,
        }];
        assert_eq!(chance_of(Some(&map), &free), Chance::Good);
        let first_used = [Extent {
            offset: 1020,
            len: 20,
        }];
        assert_eq!(chance_of(Some(&map), &first_used), Chance::Poor);
        let later_used = [Extent {
            offset: 1030,
            len: 20,
        }];
        assert_eq!(chance_of(Some(&map), &later_used), Chance::Partial);
        assert_eq!(chance_of(None, &free), Chance::Unknown);
        assert_eq!(chance_of(Some(&map), &[]), Chance::Unknown);
        let sparse = [Extent {
            offset: Extent::SPARSE,
            len: 100,
        }];
        assert_eq!(chance_of(Some(&map), &sparse), Chance::Unknown);
    }

    #[test]
    fn detect_rejects_garbage() {
        assert_eq!(detect(&[]), None);
        assert_eq!(detect(&[0u8; 512]), None);
        let mut junk = [0xFFu8; 512];
        junk[510] = 0x55;
        junk[511] = 0xAA;
        assert_eq!(detect(&junk), None);
        assert_eq!(join("", "a"), "a");
        assert_eq!(join("a", ""), "a");
        assert_eq!(join("a", "b/c"), "a/b/c");
    }
}
