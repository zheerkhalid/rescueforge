//! Partition tables: classic MBR (with extended partitions) and GPT.

use crate::source::Source;
use crate::util::{le32, le64, utf16le};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition {
    /// 1-based position in the table.
    pub index: usize,
    /// Byte offset and length on the disk.
    pub offset: u64,
    pub len: u64,
    /// The name from the GPT, or the MBR type as text.
    pub name: String,
}

fn mbr_type_name(t: u8) -> &'static str {
    match t {
        0x01 => "FAT12",
        0x04 | 0x06 | 0x0E => "FAT16",
        0x07 => "NTFS / exFAT",
        0x0B | 0x0C => "FAT32",
        0x27 => "Windows recovery",
        0x82 => "Linux swap",
        0x83 => "Linux",
        0xEF => "EFI system",
        _ => "Partition",
    }
}

/// Reads the partition table at the start of `disk`. Returns nothing when
/// there is none (or when it is too damaged to trust).
pub fn read_partitions(disk: &Source) -> Vec<Partition> {
    let sector = disk.read_vec(0, 512);
    if sector.len() < 512 || sector[510] != 0x55 || sector[511] != 0xAA {
        return Vec::new();
    }
    let entries: Vec<(u8, u64, u64)> = (0..4)
        .map(|i| {
            let e = &sector[446 + i * 16..462 + i * 16];
            (
                e[4],
                le32(e, 8).unwrap_or(0) as u64,
                le32(e, 12).unwrap_or(0) as u64,
            )
        })
        .collect();
    if entries.iter().any(|e| e.0 == 0xEE) {
        for sector_size in [512u64, 4096] {
            let parts = read_gpt(disk, sector_size);
            if !parts.is_empty() {
                return parts;
            }
        }
    }
    let mut out = Vec::new();
    let add = |out: &mut Vec<Partition>, ty: u8, start: u64, count: u64| {
        let (offset, len) = (start * 512, count * 512);
        if count == 0 || offset >= disk.len() || out.len() >= 128 {
            return;
        }
        out.push(Partition {
            index: out.len() + 1,
            offset,
            len: len.min(disk.len() - offset),
            name: mbr_type_name(ty).to_string(),
        });
    };
    for (ty, start, count) in &entries {
        match *ty {
            0 | 0xEE => {}
            0x05 | 0x0F | 0x85 => {
                // Extended partition: a linked list of small tables.
                let ext_start = *start;
                let mut ebr = ext_start;
                for _ in 0..128 {
                    let s = disk.read_vec(ebr * 512, 512);
                    if s.len() < 512 || s[510] != 0x55 || s[511] != 0xAA {
                        break;
                    }
                    let e0 = &s[446..462];
                    let e1 = &s[462..478];
                    if e0[4] != 0 {
                        add(
                            &mut out,
                            e0[4],
                            ebr + le32(e0, 8).unwrap_or(0) as u64,
                            le32(e0, 12).unwrap_or(0) as u64,
                        );
                    }
                    let next = le32(e1, 8).unwrap_or(0) as u64;
                    if e1[4] == 0 || next == 0 || ext_start + next <= ebr {
                        break;
                    }
                    ebr = ext_start + next;
                }
            }
            _ => add(&mut out, *ty, *start, *count),
        }
    }
    out
}

fn read_gpt(disk: &Source, sector_size: u64) -> Vec<Partition> {
    let h = disk.read_vec(sector_size, 512);
    if h.len() < 92 || &h[..8] != b"EFI PART" {
        return Vec::new();
    }
    let entries_lba = le64(&h, 72).unwrap_or(0);
    let count = le32(&h, 80).unwrap_or(0).min(1024) as u64;
    let size = le32(&h, 84).unwrap_or(0) as u64;
    if !(128..=1024).contains(&size) {
        return Vec::new();
    }
    let Some(table_off) = entries_lba.checked_mul(sector_size) else {
        return Vec::new();
    };
    let table = disk.read_vec(table_off, (count * size) as usize);
    let mut out = Vec::new();
    for (i, e) in table.chunks_exact(size as usize).enumerate() {
        if e[..16].iter().all(|b| *b == 0) {
            continue;
        }
        let first = le64(e, 32).unwrap_or(0);
        let last = le64(e, 40).unwrap_or(0);
        let (Some(offset), Some(end)) = (
            first.checked_mul(sector_size),
            last.checked_add(1).and_then(|l| l.checked_mul(sector_size)),
        ) else {
            continue;
        };
        if first == 0 || end <= offset || offset >= disk.len() {
            continue;
        }
        let name = utf16le(&e[56..128]);
        let name = name.trim_end_matches('\0').trim();
        out.push(Partition {
            index: i + 1,
            offset,
            len: (end - offset).min(disk.len() - offset),
            name: if name.is_empty() {
                "Partition".to_string()
            } else {
                name.to_string()
            },
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimg;
    use crate::util::Rng;

    #[test]
    fn mbr_with_primary_and_logical_partitions() {
        let a = vec![0xAAu8; 64 * 512];
        let b = vec![0xBBu8; 32 * 512];
        let c = vec![0xCCu8; 16 * 512];
        let disk = testimg::disk::mbr(&[(0x07, &a), (0x0C, &b)], &[(0x06, &c)]);
        let src = Source::from_bytes(disk);
        let parts = read_partitions(&src);
        assert_eq!(parts.len(), 3, "{parts:?}");
        assert_eq!(parts[0].name, "NTFS / exFAT");
        assert_eq!(parts[0].len, 64 * 512);
        assert_eq!(src.read_vec(parts[0].offset, 2), [0xAA, 0xAA]);
        assert_eq!(src.read_vec(parts[1].offset, 2), [0xBB, 0xBB]);
        assert_eq!(parts[2].name, "FAT16");
        assert_eq!(src.read_vec(parts[2].offset, 2), [0xCC, 0xCC]);
        assert_eq!(parts[2].len, 16 * 512);
    }

    #[test]
    fn gpt() {
        let a = vec![0xAAu8; 64 * 512];
        let b = vec![0xBBu8; 32 * 512];
        let disk = testimg::disk::gpt(&[("Basic data partition", &a), ("Photos", &b)]);
        let src = Source::from_bytes(disk);
        let parts = read_partitions(&src);
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert_eq!(parts[0].name, "Basic data partition");
        assert_eq!(parts[1].name, "Photos");
        assert_eq!(parts[1].len, 32 * 512);
        assert_eq!(src.read_vec(parts[1].offset, 1), [0xBB]);
    }

    #[test]
    fn damaged_tables_never_panic_or_loop() {
        assert!(read_partitions(&Source::from_bytes(vec![])).is_empty());
        assert!(read_partitions(&Source::from_bytes(vec![0; 100])).is_empty());
        let mut rng = Rng(99);
        for round in 0..3000 {
            let mut disk = rng.bytes(8192);
            disk[510] = 0x55;
            disk[511] = 0xAA;
            match round % 3 {
                // An extended partition whose chain points at itself.
                0 => {
                    disk[446 + 4] = 0x05;
                    disk[446 + 8..446 + 12].copy_from_slice(&2u32.to_le_bytes());
                    disk[1024 + 510] = 0x55;
                    disk[1024 + 511] = 0xAA;
                }
                // A GPT header with random geometry.
                1 => {
                    disk[446 + 4] = 0xEE;
                    disk[512..520].copy_from_slice(b"EFI PART");
                }
                _ => {}
            }
            let src = Source::from_bytes(disk);
            for p in read_partitions(&src) {
                assert!(p.offset < src.len() && p.offset + p.len <= src.len());
            }
        }
    }
}
