//! NTFS: reads the master file table (MFT) and lists records that are marked
//! "not in use" (deleted files), with their original names, folders, times
//! and the location of their data.
//!
//! The same record parser is used by the deep scan for file records that an
//! earlier NTFS left behind (for example after a quick format).

use super::{chance_of, join, AllocMap, Ctx, FsKind, VolumeInfo};
use crate::model::{Chance, Extent, FileData, FoundFile, Origin, Status};
use crate::source::Source;
use crate::util::{filetime_to_unix, le16, le32, le64, utf16le};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Boot {
    pub bps: u64,
    /// Bytes per cluster.
    pub cluster: u64,
    pub total_sectors: u64,
    pub mft_lcn: u64,
    pub mirr_lcn: u64,
    pub record_size: usize,
}

pub fn parse_boot(b: &[u8]) -> Option<Boot> {
    if b.len() < 512 || &b[3..11] != b"NTFS    " || b[510] != 0x55 || b[511] != 0xAA {
        return None;
    }
    let bps = le16(b, 11)? as u64;
    if !bps.is_power_of_two() || !(512..=4096).contains(&bps) {
        return None;
    }
    let spc = match b[13] {
        0 => return None,
        n if n <= 0x80 => n as u64,
        // Large clusters are stored as a negative power of two.
        n => {
            let shift = 256 - n as u32;
            if shift > 12 {
                return None;
            }
            1u64 << shift
        }
    };
    if !spc.is_power_of_two() {
        return None;
    }
    let cluster = bps * spc;
    let total_sectors = le64(b, 0x28)?;
    let mft_lcn = le64(b, 0x30)?;
    let mirr_lcn = le64(b, 0x38)?;
    let raw = b[0x40] as i8;
    let record_size = if raw > 0 {
        (raw as u64).checked_mul(cluster)?
    } else {
        let shift = (-(raw as i32)) as u32;
        if shift > 16 {
            return None;
        }
        1u64 << shift
    };
    if !record_size.is_power_of_two() || !(512..=4096).contains(&record_size) {
        return None;
    }
    let volume_bytes = total_sectors.checked_mul(bps)?;
    if total_sectors == 0 || mft_lcn.checked_mul(cluster)? >= volume_bytes {
        return None;
    }
    Some(Boot {
        bps,
        cluster,
        total_sectors,
        mft_lcn,
        mirr_lcn,
        record_size: record_size as usize,
    })
}

/// A run of clusters; `lcn` is `None` for a hole in a sparse file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub lcn: Option<u64>,
    pub len: u64,
}

/// Decodes a run list. Returns the runs read before the terminator and
/// whether the list was well formed.
pub fn parse_runs(b: &[u8]) -> (Vec<Run>, bool) {
    let mut runs = Vec::new();
    let mut pos = 0usize;
    let mut lcn: i64 = 0;
    loop {
        let Some(&head) = b.get(pos) else {
            return (runs, false);
        };
        if head == 0 {
            return (runs, true);
        }
        let len_size = (head & 15) as usize;
        let off_size = (head >> 4) as usize;
        if len_size == 0 || len_size > 8 || off_size > 8 {
            return (runs, false);
        }
        let Some(field) = b.get(pos + 1..pos + 1 + len_size + off_size) else {
            return (runs, false);
        };
        let mut len = 0u64;
        for (i, x) in field[..len_size].iter().enumerate() {
            len |= (*x as u64) << (8 * i);
        }
        if len == 0 || len > 1 << 40 {
            return (runs, false);
        }
        if off_size == 0 {
            runs.push(Run { lcn: None, len });
        } else {
            let mut off = 0i64;
            for (i, x) in field[len_size..].iter().enumerate() {
                off |= (*x as i64) << (8 * i);
            }
            // Sign-extend.
            let shift = 64 - 8 * off_size as u32;
            off = (off << shift) >> shift;
            lcn = match lcn.checked_add(off) {
                Some(v) if v >= 0 => v,
                _ => return (runs, false),
            };
            runs.push(Run {
                lcn: Some(lcn as u64),
                len,
            });
        }
        pos += 1 + len_size + off_size;
        if runs.len() >= 200_000 {
            return (runs, false);
        }
    }
}

/// What RescueForge keeps from one file record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub number: u64,
    pub seq: u16,
    pub in_use: bool,
    pub is_dir: bool,
    /// Record number of the base record when this is an extension record, else 0.
    pub base: u64,
    pub parent: u64,
    pub parent_seq: u16,
    pub name: String,
    pub size: u64,
    pub created: i64,
    pub modified: i64,
    pub resident: Option<Vec<u8>>,
    /// Pieces of the unnamed data stream: `(first virtual cluster, runs)`.
    pub parts: Vec<(u64, Vec<Run>)>,
    pub compressed: bool,
    pub encrypted: bool,
    pub runs_ok: bool,
}

/// Does this sector start a file record? Returns the record size.
pub fn looks_like_record(head: &[u8]) -> Option<usize> {
    if head.len() < 48 || &head[..4] != b"FILE" {
        return None;
    }
    let usa_off = le16(head, 4)? as usize;
    let usa_n = le16(head, 6)? as usize;
    let attrs = le16(head, 0x14)? as usize;
    let used = le32(head, 0x18)? as usize;
    let alloc = le32(head, 0x1C)? as usize;
    if !(0x28..=0x40).contains(&usa_off)
        || !(2..=9).contains(&usa_n)
        || !(0x30..512).contains(&attrs)
        || !attrs.is_multiple_of(8)
        || !(alloc == 1024 || alloc == 4096)
        || used > alloc
        || used < attrs
        || usa_n != alloc / 512 + 1
    {
        return None;
    }
    Some(alloc)
}

/// Undoes the "update sequence" protection: the last two bytes of every
/// 512-byte block were swapped out when the record was written. Returns
/// false for a torn record.
fn apply_fixups(rec: &mut [u8]) -> bool {
    let (Some(usa_off), Some(usa_n)) = (le16(rec, 4), le16(rec, 6)) else {
        return false;
    };
    let (usa_off, usa_n) = (usa_off as usize, usa_n as usize);
    if usa_n < 2 {
        return true;
    }
    if usa_off + usa_n * 2 > rec.len().min(510) {
        return false;
    }
    let usn = [rec[usa_off], rec[usa_off + 1]];
    for i in 1..usa_n {
        let pos = i * 512 - 2;
        if pos + 2 > rec.len() {
            break;
        }
        if rec[pos..pos + 2] != usn {
            return false;
        }
        rec[pos] = rec[usa_off + i * 2];
        rec[pos + 1] = rec[usa_off + i * 2 + 1];
    }
    true
}

/// Parses one file record. `number` overrides the record number stored in
/// the record (known when walking a live MFT).
pub fn parse_record(rec: &mut [u8], number: Option<u64>) -> Option<Record> {
    if rec.len() < 48 || &rec[..4] != b"FILE" || !apply_fixups(rec) {
        return None;
    }
    let rec: &[u8] = rec;
    let flags = le16(rec, 0x16)?;
    let attrs_off = le16(rec, 0x14)? as usize;
    let used = (le32(rec, 0x18)? as usize).min(rec.len());
    let mut r = Record {
        number: match number {
            Some(n) => n,
            None => le32(rec, 0x2C)? as u64,
        },
        seq: le16(rec, 0x10)?,
        in_use: flags & 1 != 0,
        is_dir: flags & 2 != 0,
        base: le64(rec, 0x20)? & 0xFFFF_FFFF_FFFF,
        runs_ok: true,
        ..Default::default()
    };
    let mut name_rank = 0u8;
    let mut name_times = (0i64, 0i64);
    let mut off = attrs_off;
    let mut guard = 0;
    while off + 16 <= used && guard < 256 {
        guard += 1;
        let ty = le32(rec, off)?;
        if ty == 0xFFFF_FFFF {
            break;
        }
        let len = le32(rec, off + 4)? as usize;
        if len < 16 || !len.is_multiple_of(8) || off + len > used {
            break;
        }
        let a = &rec[off..off + len];
        off += len;
        let non_resident = a[8] != 0;
        let name_len = a[9];
        let aflags = le16(a, 12)?;
        let content = if non_resident {
            None
        } else {
            let clen = le32(a, 16).unwrap_or(0) as usize;
            let coff = le16(a, 20).unwrap_or(0) as usize;
            a.get(coff..coff.checked_add(clen)?)
        };
        match ty {
            0x10 => {
                if let Some(c) = content {
                    r.created = filetime_to_unix(le64(c, 0).unwrap_or(0));
                    r.modified = filetime_to_unix(le64(c, 8).unwrap_or(0));
                }
            }
            0x30 => {
                let Some(c) = content else { continue };
                if c.len() < 66 {
                    continue;
                }
                let chars = c[64] as usize;
                let Some(raw) = c.get(66..66 + chars * 2) else {
                    continue;
                };
                // Namespace 2 is the short 8.3 alias: only used if nothing better exists.
                let rank = if c[65] == 2 { 1 } else { 2 };
                if rank > name_rank {
                    name_rank = rank;
                    r.name = utf16le(raw);
                    let p = le64(c, 0).unwrap_or(0);
                    r.parent = p & 0xFFFF_FFFF_FFFF;
                    r.parent_seq = (p >> 48) as u16;
                    name_times = (
                        filetime_to_unix(le64(c, 8).unwrap_or(0)),
                        filetime_to_unix(le64(c, 16).unwrap_or(0)),
                    );
                    if r.size == 0 && r.resident.is_none() && r.parts.is_empty() {
                        r.size = le64(c, 48).unwrap_or(0);
                    }
                }
            }
            0x80 if name_len == 0 => {
                if non_resident {
                    let start_vcn = le64(a, 16).unwrap_or(0);
                    let runs_off = le16(a, 32).unwrap_or(0) as usize;
                    let (runs, ok) = parse_runs(a.get(runs_off..).unwrap_or(&[]));
                    r.runs_ok &= ok;
                    if start_vcn == 0 {
                        r.size = le64(a, 48).unwrap_or(0);
                        r.compressed = aflags & 0x00FF != 0;
                        r.encrypted = aflags & 0x4000 != 0;
                    }
                    r.parts.push((start_vcn, runs));
                } else if let Some(c) = content {
                    r.size = c.len() as u64;
                    r.resident = Some(c.to_vec());
                }
            }
            _ => {}
        }
    }
    if r.created == 0 {
        r.created = name_times.0;
    }
    if r.modified == 0 {
        r.modified = name_times.1;
    }
    Some(r)
}

/// Where the clusters of a volume are on the source.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    /// Absolute offset of cluster 0.
    pub vol_base: u64,
    pub cluster: u64,
    /// Number of clusters in the volume.
    pub clusters: u64,
}

struct DirEnt {
    name: String,
    parent: u64,
    seq: u16,
    in_use: bool,
}

/// Gathers records, then turns them into files with full paths.
#[derive(Default)]
pub struct Collector {
    dirs: HashMap<u64, DirEnt>,
    files: Vec<Record>,
    extra: HashMap<u64, Vec<(u64, Vec<Run>)>>,
}

pub const MAX_RECORDS: usize = 4_000_000;
const ROOT: u64 = 5;

impl Collector {
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// `keep_existing`: also keep files whose record is still in use.
    pub fn add(&mut self, r: Record, keep_existing: bool) {
        if r.base != 0 {
            if !r.parts.is_empty() && (keep_existing || !r.in_use) && self.extra.len() < MAX_RECORDS
            {
                self.extra.entry(r.base).or_default().extend(r.parts);
            }
            return;
        }
        if r.name.is_empty() {
            return;
        }
        if r.is_dir {
            if self.dirs.len() < MAX_RECORDS {
                self.dirs.insert(
                    r.number,
                    DirEnt {
                        name: r.name,
                        parent: r.parent,
                        seq: r.seq,
                        in_use: r.in_use,
                    },
                );
            }
            return;
        }
        if (r.in_use && !keep_existing) || r.size == 0 || self.files.len() >= MAX_RECORDS {
            return;
        }
        // The file system's own files ($MFT, $Bitmap, ...).
        if r.number < 16 || (r.parent == ROOT && r.name.starts_with('$')) {
            return;
        }
        self.files.push(r);
    }

    fn folder(&self, parent: u64, parent_seq: u16) -> String {
        let mut parts: Vec<&str> = Vec::new();
        let mut cur = parent;
        let mut known = false;
        for depth in 0..64 {
            if cur == ROOT {
                known = true;
                break;
            }
            let Some(d) = self.dirs.get(&cur) else { break };
            if depth == 0 && parent_seq != 0 {
                // The folder's record may have been reused by something else.
                let same =
                    d.seq == parent_seq || (!d.in_use && d.seq == parent_seq.wrapping_add(1));
                if !same {
                    break;
                }
            }
            parts.push(&d.name);
            if d.parent == cur {
                break;
            }
            cur = d.parent;
        }
        parts.reverse();
        let path = parts.join("/");
        if known {
            path
        } else {
            join("(Folder unknown)", &path)
        }
    }

    /// Builds the final list. `status` maps "record in use" to a status.
    pub fn finish(
        mut self,
        geo: &Geometry,
        alloc: Option<&AllocMap>,
        prefix: &str,
        origin: Origin,
        status: &dyn Fn(bool) -> Status,
    ) -> Vec<FoundFile> {
        let mut out = Vec::with_capacity(self.files.len());
        let files = std::mem::take(&mut self.files);
        for mut r in files {
            let folder = self.folder(r.parent, r.parent_seq);
            if folder.starts_with('$') {
                continue;
            }
            if let Some(more) = self.extra.remove(&r.number) {
                r.parts.extend(more);
            }
            let st = status(r.in_use);
            let mut note = String::new();
            let (data, mut chance) = if let Some(bytes) = r.resident.take() {
                (FileData::Resident(bytes), Chance::Good)
            } else {
                r.parts.sort_by_key(|p| p.0);
                let mut extents = Vec::new();
                let mut covered = 0u64;
                let mut next_vcn = 0u64;
                let mut complete = r.runs_ok;
                'parts: for (vcn, runs) in &r.parts {
                    if *vcn != next_vcn {
                        complete = false;
                        break;
                    }
                    for run in runs {
                        if covered >= r.size {
                            break 'parts;
                        }
                        let Some(bytes) = run.len.checked_mul(geo.cluster) else {
                            complete = false;
                            break 'parts;
                        };
                        let bytes = bytes.min(r.size - covered);
                        match run.lcn {
                            None => extents.push(Extent {
                                offset: Extent::SPARSE,
                                len: bytes,
                            }),
                            Some(lcn) => {
                                if lcn.saturating_add(run.len) > geo.clusters {
                                    complete = false;
                                    break 'parts;
                                }
                                extents.push(Extent {
                                    offset: geo.vol_base + lcn * geo.cluster,
                                    len: bytes,
                                });
                            }
                        }
                        covered += bytes;
                        next_vcn += run.len;
                    }
                }
                let chance = if extents.is_empty() {
                    note =
                        "The file record no longer says where the content was stored.".to_string();
                    Chance::Poor
                } else if !complete || covered < r.size {
                    note = "Only part of the content could be located.".to_string();
                    Chance::Partial
                } else {
                    match st {
                        Status::Existing => Chance::Good,
                        Status::Lost if alloc.is_none() => Chance::Good,
                        _ => chance_of(alloc, &extents),
                    }
                };
                (FileData::Extents(extents), chance)
            };
            if r.compressed {
                note = "Stored with NTFS compression. The recovered copy holds the compressed data and will not open directly.".to_string();
                chance = Chance::Poor;
            } else if r.encrypted {
                note = "Encrypted by Windows (EFS). The recovered copy is still encrypted."
                    .to_string();
                if chance == Chance::Good {
                    chance = Chance::Partial;
                }
            }
            out.push(FoundFile {
                name: r.name,
                path: join(prefix, &folder),
                size: r.size,
                modified: r.modified,
                created: r.created,
                status: st,
                origin,
                chance,
                data,
                note,
            });
        }
        out
    }
}

/// A file made of several pieces of the volume (the MFT itself, `$Bitmap`).
struct Pieces {
    /// `(offset in the volume, length)`, in file order.
    ext: Vec<(u64, u64)>,
    total: u64,
}

impl Pieces {
    fn from_runs(runs: &[Run], boot: &Boot, vol_len: u64) -> Pieces {
        let mut ext = Vec::new();
        let mut total = 0u64;
        for r in runs {
            let Some(lcn) = r.lcn else { break };
            let (Some(off), Some(len)) = (
                lcn.checked_mul(boot.cluster),
                r.len.checked_mul(boot.cluster),
            ) else {
                break;
            };
            if off.saturating_add(len) > vol_len {
                break;
            }
            ext.push((off, len));
            total += len;
        }
        Pieces { ext, total }
    }

    fn read(&self, vol: &Source, mut offset: u64, buf: &mut [u8]) {
        let mut done = 0usize;
        for (start, len) in &self.ext {
            if done == buf.len() {
                break;
            }
            if offset >= *len {
                offset -= len;
                continue;
            }
            let n = ((len - offset) as usize).min(buf.len() - done);
            vol.read_at(start + offset, &mut buf[done..done + n]);
            done += n;
            offset = 0;
        }
        buf[done..].fill(0);
    }
}

fn first_data_runs(rec: &Record) -> Vec<Run> {
    let mut parts = rec.parts.clone();
    parts.sort_by_key(|p| p.0);
    parts.into_iter().flat_map(|p| p.1).collect()
}

/// Lists the files of an NTFS volume.
pub fn scan(vol: &Source, boot: &Boot, cx: &mut Ctx) -> Result<VolumeInfo, String> {
    let rs = boot.record_size;
    let vol_len = vol.len();
    let clusters = (boot.total_sectors * boot.bps / boot.cluster).min(vol_len / boot.cluster);
    cx.progress.set_phase("Reading the NTFS file table");

    // Record 0 describes the MFT itself. If it is damaged, the mirror copy
    // (first four records) is tried.
    let mut mft_record = None;
    for lcn in [boot.mft_lcn, boot.mirr_lcn] {
        let Some(off) = lcn.checked_mul(boot.cluster) else {
            continue;
        };
        if let Some(mut buf) = vol.read_exact_vec(off, rs) {
            if let Some(r) = parse_record(&mut buf, Some(0)) {
                if !r.parts.is_empty() {
                    mft_record = Some(r);
                    break;
                }
            }
        }
    }
    let (pieces, mft_bytes) = match &mft_record {
        Some(r) => {
            let p = Pieces::from_runs(&first_data_runs(r), boot, vol_len);
            let bytes = if r.size > 0 {
                r.size.min(p.total)
            } else {
                p.total
            };
            (p, bytes)
        }
        None => {
            (cx.log)(
                "The first record of the NTFS file table is damaged. Reading the table as one continuous block instead.".to_string(),
            );
            let off = boot.mft_lcn * boot.cluster;
            if off >= vol_len {
                return Err("The NTFS file table lies outside this source.".to_string());
            }
            let len = (vol_len - off).min(1 << 30);
            (
                Pieces {
                    ext: vec![(off, len)],
                    total: len,
                },
                len,
            )
        }
    };
    if pieces.total == 0 {
        return Err("The NTFS file table could not be located.".to_string());
    }
    let total_records = mft_bytes / rs as u64;

    // $Bitmap (record 6): one bit per cluster.
    let mut alloc = None;
    let mut rec6 = vec![0u8; rs];
    pieces.read(vol, 6 * rs as u64, &mut rec6);
    if let Some(r) = parse_record(&mut rec6, Some(6)) {
        if r.name == "$Bitmap" {
            let p = Pieces::from_runs(&first_data_runs(&r), boot, vol_len);
            let want = clusters.div_ceil(8).min(p.total).min(1 << 30) as usize;
            if want > 0 {
                let mut bits = vec![0u8; want];
                let mut pos = 0usize;
                while pos < want {
                    let n = (want - pos).min(4 << 20);
                    p.read(vol, pos as u64, &mut bits[pos..pos + n]);
                    pos += n;
                    if cx.ctl.checkpoint() {
                        return Err("Stopped.".to_string());
                    }
                }
                alloc = Some(AllocMap::new(
                    vol.base(),
                    vol.base() + vol_len,
                    vol.base(),
                    boot.cluster,
                    clusters,
                    bits,
                ));
            }
        }
    }
    if alloc.is_none() {
        (cx.log)(
            "The NTFS cluster bitmap could not be read, so recoverability cannot be rated."
                .to_string(),
        );
    }

    let keep_existing = cx.include_existing || cx.lost;
    let mut col = Collector::default();
    let per_batch = ((4usize << 20) / rs).max(1);
    let mut buf = vec![0u8; per_batch * rs];
    let mut index = 0u64;
    let mut blank_run = 0u64;
    while index < total_records {
        if cx.ctl.checkpoint() {
            break;
        }
        let n = (total_records - index).min(per_batch as u64) as usize;
        pieces.read(vol, index * rs as u64, &mut buf[..n * rs]);
        for i in 0..n {
            let rec = &mut buf[i * rs..(i + 1) * rs];
            match parse_record(rec, Some(index + i as u64)) {
                Some(r) => {
                    blank_run = 0;
                    col.add(r, keep_existing);
                }
                None => blank_run += 1,
            }
        }
        index += n as u64;
        cx.progress.set(index * rs as u64, mft_bytes);
        cx.progress
            .found
            .store(col.len() as u64, std::sync::atomic::Ordering::Relaxed);
        // Without a trustworthy size, stop after a long stretch without records.
        if mft_record.is_none() && blank_run > 65_536 {
            break;
        }
    }

    let geo = Geometry {
        vol_base: vol.base(),
        cluster: boot.cluster,
        clusters,
    };
    let lost = cx.lost;
    let status = move |in_use: bool| match (in_use, lost) {
        (false, _) => Status::Deleted,
        (true, true) => Status::Lost,
        (true, false) => Status::Existing,
    };
    let files = col.finish(&geo, alloc.as_ref(), cx.prefix, Origin::Ntfs, &status);
    let listed = files.len() as u64;
    let mut iter = files.into_iter().peekable();
    while iter.peek().is_some() {
        (cx.emit)(iter.by_ref().take(5000).collect());
    }
    let reserved = pieces
        .ext
        .iter()
        .map(|(o, l)| Extent {
            offset: vol.base() + o,
            len: *l,
        })
        .collect();
    Ok(VolumeInfo {
        kind: FsKind::Ntfs,
        label: String::new(),
        cluster: boot.cluster,
        alloc,
        reserved,
        listed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::Rng;

    #[test]
    fn run_lists() {
        // 0x21: 1-byte length, 2-byte offset. Then a negative delta, then a hole.
        let b = [0x21, 0x10, 0x00, 0x01, 0x11, 0x08, 0xF0, 0x01, 0x04, 0x00];
        let (runs, ok) = parse_runs(&b);
        assert!(ok);
        assert_eq!(
            runs,
            vec![
                Run {
                    lcn: Some(256),
                    len: 16
                },
                Run {
                    lcn: Some(240),
                    len: 8
                },
                Run { lcn: None, len: 4 },
            ]
        );
        // Missing terminator, oversized fields, negative cluster: flagged, never a panic.
        assert!(!parse_runs(&[0x21, 0x10, 0x00]).1);
        assert!(!parse_runs(&[0x9F, 1, 2, 3]).1);
        assert!(!parse_runs(&[0x11, 0x08, 0x80, 0x00]).1);
        assert!(!parse_runs(&[]).1);
        assert_eq!(parse_runs(&[0x00]), (vec![], true));
    }

    #[test]
    fn boot_sector_validation() {
        let img = crate::testimg::ntfs::build(&crate::testimg::sample_tree(), 16 << 20);
        let boot = parse_boot(&img[..512]).expect("valid boot sector");
        assert_eq!(boot.cluster, 4096);
        assert_eq!(boot.record_size, 1024);
        let mut bad = img[..512].to_vec();
        bad[13] = 3; // sectors per cluster must be a power of two
        assert!(parse_boot(&bad).is_none());
        let mut bad = img[..512].to_vec();
        bad[0x30..0x38].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(parse_boot(&bad).is_none());
        let mut bad = img[..512].to_vec();
        bad[0x40] = 0x80; // record size 2^128
        assert!(parse_boot(&bad).is_none());
        assert!(parse_boot(&img[..100]).is_none());
    }

    #[test]
    fn garbage_records_never_panic() {
        let mut rng = Rng(0x5EED);
        for round in 0..4000 {
            let mut rec = rng.bytes(1024);
            rec[..4].copy_from_slice(b"FILE");
            if round % 2 == 0 {
                // Plausible header so that the attribute walk is reached.
                rec[4..6].copy_from_slice(&0x30u16.to_le_bytes());
                rec[6..8].copy_from_slice(&3u16.to_le_bytes());
                let usn = [rec[0x30], rec[0x31]];
                rec[510..512].copy_from_slice(&usn);
                rec[1022..1024].copy_from_slice(&usn);
                rec[0x14..0x16].copy_from_slice(&0x38u16.to_le_bytes());
                rec[0x18..0x1C].copy_from_slice(&1024u32.to_le_bytes());
                if round % 4 == 0 {
                    // A plausible first attribute header with random content.
                    let ty = [0x10u32, 0x30, 0x80][(round / 4) % 3];
                    rec[0x38..0x3C].copy_from_slice(&ty.to_le_bytes());
                    rec[0x3C..0x40]
                        .copy_from_slice(&((rng.below(60) + 2) as u32 * 8).to_le_bytes());
                }
            }
            let _ = parse_record(&mut rec, None);
            let _ = looks_like_record(&rec);
        }
        assert!(parse_record(&mut [], None).is_none());
        assert!(parse_record(&mut b"FILE".to_vec(), None).is_none());
    }

    #[test]
    fn torn_records_are_rejected() {
        let img = crate::testimg::ntfs::build(&crate::testimg::sample_tree(), 16 << 20);
        let boot = parse_boot(&img[..512]).unwrap();
        let off = (boot.mft_lcn * boot.cluster) as usize + 5 * 1024;
        let mut rec = img[off..off + 1024].to_vec();
        assert!(parse_record(&mut rec.clone(), None).is_some());
        assert_eq!(looks_like_record(&rec), Some(1024));
        rec[510] ^= 0xFF; // the first sector's check bytes no longer match
        assert!(parse_record(&mut rec, None).is_none());
    }
}
