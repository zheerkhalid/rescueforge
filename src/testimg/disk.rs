//! Whole-disk images: volumes placed behind an MBR or GPT partition table.

const GAP: usize = 64;

fn put_entry(sector: &mut [u8], slot: usize, kind: u8, start: u32, count: u32) {
    let e = &mut sector[446 + slot * 16..462 + slot * 16];
    e[4] = kind;
    e[8..12].copy_from_slice(&start.to_le_bytes());
    e[12..16].copy_from_slice(&count.to_le_bytes());
}

/// An MBR disk. `primary`: up to three `(type, volume)` pairs; `logical`:
/// volumes inside an extended partition.
pub fn mbr(primary: &[(u8, &Vec<u8>)], logical: &[(u8, &Vec<u8>)]) -> Vec<u8> {
    assert!(primary.len() <= 3);
    let mut disk = vec![0u8; 512];
    let mut table = [0u8; 512];
    for (slot, (kind, vol)) in primary.iter().enumerate() {
        disk.resize(disk.len().div_ceil(GAP * 512) * GAP * 512, 0);
        put_entry(
            &mut table,
            slot,
            *kind,
            (disk.len() / 512) as u32,
            (vol.len() / 512) as u32,
        );
        disk.extend_from_slice(vol);
    }
    if !logical.is_empty() {
        disk.resize(disk.len().div_ceil(GAP * 512) * GAP * 512, 0);
        let ext_start = disk.len() / 512;
        for (i, (kind, vol)) in logical.iter().enumerate() {
            let ebr = disk.len() / 512;
            let mut s = [0u8; 512];
            put_entry(&mut s, 0, *kind, GAP as u32, (vol.len() / 512) as u32);
            let next = ebr + GAP + vol.len() / 512;
            let next = next.div_ceil(GAP) * GAP;
            if i + 1 < logical.len() {
                put_entry(&mut s, 1, 0x05, (next - ext_start) as u32, 1);
            }
            s[510] = 0x55;
            s[511] = 0xAA;
            disk.extend_from_slice(&s);
            disk.resize((ebr + GAP) * 512, 0);
            disk.extend_from_slice(vol);
            disk.resize(next * 512, 0);
        }
        put_entry(
            &mut table,
            primary.len(),
            0x0F,
            ext_start as u32,
            (disk.len() / 512 - ext_start) as u32,
        );
    }
    table[510] = 0x55;
    table[511] = 0xAA;
    disk[..512].copy_from_slice(&table);
    disk.resize(disk.len() + GAP * 512, 0);
    disk
}

/// A GPT disk with a protective MBR.
pub fn gpt(parts: &[(&str, &Vec<u8>)]) -> Vec<u8> {
    let mut disk = vec![0u8; 34 * 512];
    put_entry(&mut disk[..512], 0, 0xEE, 1, u32::MAX);
    disk[510] = 0x55;
    disk[511] = 0xAA;
    let mut entries = vec![0u8; 128 * 128];
    for (i, (name, vol)) in parts.iter().enumerate() {
        disk.resize(disk.len().div_ceil(GAP * 512) * GAP * 512, 0);
        let first = (disk.len() / 512) as u64;
        disk.extend_from_slice(vol);
        let last = (disk.len() / 512) as u64 - 1;
        let e = &mut entries[i * 128..(i + 1) * 128];
        // "Basic data" partition type.
        e[..16].copy_from_slice(&[
            0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26,
            0x99, 0xC7,
        ]);
        e[16] = i as u8 + 1;
        e[32..40].copy_from_slice(&first.to_le_bytes());
        e[40..48].copy_from_slice(&last.to_le_bytes());
        for (k, u) in name.encode_utf16().take(36).enumerate() {
            e[56 + k * 2..58 + k * 2].copy_from_slice(&u.to_le_bytes());
        }
    }
    disk.resize(disk.len() + 34 * 512, 0);
    let h = &mut disk[512..1024];
    h[..8].copy_from_slice(b"EFI PART");
    h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    h[12..16].copy_from_slice(&92u32.to_le_bytes());
    h[24..32].copy_from_slice(&1u64.to_le_bytes());
    h[72..80].copy_from_slice(&2u64.to_le_bytes());
    h[80..84].copy_from_slice(&128u32.to_le_bytes());
    h[84..88].copy_from_slice(&128u32.to_le_bytes());
    disk[2 * 512..2 * 512 + entries.len()].copy_from_slice(&entries);
    disk
}
