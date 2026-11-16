//! Read-only access to whatever is being scanned: a volume, a physical disk,
//! a disk image file, or a buffer in memory (tests and the demo).
//!
//! Nothing in this module can write. Devices and images are opened with read
//! access only, and the [`BlockSource`] trait has no write method.
//!
//! [`Source`] adds the robustness layer on top: reads never fail. A range that
//! cannot be read is retried, then split down to single sectors; sectors that
//! still fail are returned as zeros and written to the read log.

use std::fs::File;
use std::io;
use std::os::windows::fs::{FileExt, OpenOptionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub trait BlockSource: Send + Sync {
    /// Size in bytes.
    fn len(&self) -> u64;
    /// Fills `buf` from `offset`. The caller keeps the range inside `len()`.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
    /// The smallest unit worth retrying on its own.
    fn sector(&self) -> u32 {
        512
    }
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Unreadable areas met so far.
#[derive(Default)]
pub struct ReadLog {
    ranges: Mutex<Vec<(u64, u64)>>,
    bad_bytes: AtomicU64,
    retries: AtomicU64,
}

impl ReadLog {
    const MAX_RANGES: usize = 20_000;

    fn add(&self, offset: u64, len: u64) {
        self.bad_bytes.fetch_add(len, Ordering::Relaxed);
        let mut r = self.ranges.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(last) = r.last_mut() {
            if last.0 + last.1 == offset {
                last.1 += len;
                return;
            }
        }
        if r.len() < Self::MAX_RANGES {
            r.push((offset, len));
        }
    }

    /// Bytes that could not be read and were replaced by zeros.
    pub fn bad_bytes(&self) -> u64 {
        self.bad_bytes.load(Ordering::Relaxed)
    }

    pub fn retries(&self) -> u64 {
        self.retries.load(Ordering::Relaxed)
    }

    /// `(offset, length)` of each unreadable area, in the order they were met.
    pub fn ranges(&self) -> Vec<(u64, u64)> {
        self.ranges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// A window onto a block source. Cheap to clone; clones share the read log.
#[derive(Clone)]
pub struct Source {
    inner: Arc<dyn BlockSource>,
    base: u64,
    len: u64,
    log: Arc<ReadLog>,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Source({} bytes at {})", self.len, self.base)
    }
}

impl Source {
    pub fn new(inner: Arc<dyn BlockSource>) -> Source {
        let len = inner.len();
        Source {
            inner,
            base: 0,
            len,
            log: Arc::new(ReadLog::default()),
        }
    }

    pub fn from_bytes(data: Vec<u8>) -> Source {
        Source::new(Arc::new(MemSource(data)))
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Where this window starts on the underlying source.
    pub fn base(&self) -> u64 {
        self.base
    }

    pub fn log(&self) -> &Arc<ReadLog> {
        &self.log
    }

    /// A sub-window (clamped to this one), for example one partition of a disk.
    pub fn slice(&self, offset: u64, len: u64) -> Source {
        let offset = offset.min(self.len);
        Source {
            inner: self.inner.clone(),
            base: self.base + offset,
            len: len.min(self.len - offset),
            log: self.log.clone(),
        }
    }

    /// The whole underlying source.
    pub fn root(&self) -> Source {
        Source {
            inner: self.inner.clone(),
            base: 0,
            len: self.inner.len(),
            log: self.log.clone(),
        }
    }

    /// Reads into `buf`. Bytes past the end are zero; unreadable sectors are
    /// zero and logged. Returns how many bytes lie inside the source.
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> usize {
        let avail = self.len.saturating_sub(offset).min(buf.len() as u64) as usize;
        buf[avail..].fill(0);
        if avail > 0 {
            self.read_robust(self.base + offset, &mut buf[..avail]);
        }
        avail
    }

    /// Up to `len` bytes from `offset` (shorter at the end of the source).
    pub fn read_vec(&self, offset: u64, len: usize) -> Vec<u8> {
        let avail = self.len.saturating_sub(offset).min(len as u64) as usize;
        let mut v = vec![0u8; avail];
        if avail > 0 {
            self.read_robust(self.base + offset, &mut v);
        }
        v
    }

    /// Exactly `len` bytes, or `None` when the range leaves the source.
    pub fn read_exact_vec(&self, offset: u64, len: usize) -> Option<Vec<u8>> {
        if offset.checked_add(len as u64)? > self.len {
            return None;
        }
        Some(self.read_vec(offset, len))
    }

    fn read_robust(&self, abs: u64, buf: &mut [u8]) {
        if self.inner.read_at(abs, buf).is_ok() {
            return;
        }
        self.log.retries.fetch_add(1, Ordering::Relaxed);
        if self.inner.read_at(abs, buf).is_ok() {
            return;
        }
        const BLOCK: u64 = 64 * 1024;
        let sector = (self.inner.sector() as u64).clamp(512, 4096);
        let total = buf.len();
        let mut pos = 0usize;
        while pos < total {
            let to_boundary = BLOCK - ((abs + pos as u64) % BLOCK);
            let block_end = total.min(pos + to_boundary as usize);
            if total as u64 > BLOCK
                && self
                    .inner
                    .read_at(abs + pos as u64, &mut buf[pos..block_end])
                    .is_ok()
            {
                pos = block_end;
                continue;
            }
            // Sector by sector. After eight failures in a row the rest of the
            // 64 KB block is given up, so a dead area does not stall the scan.
            let mut p = pos;
            let mut failures = 0;
            while p < block_end {
                if failures >= 8 {
                    buf[p..block_end].fill(0);
                    self.log.add(abs + p as u64, (block_end - p) as u64);
                    break;
                }
                let to_sector = sector - ((abs + p as u64) % sector);
                let e = block_end.min(p + to_sector as usize);
                let piece = &mut buf[p..e];
                let ok = self.inner.read_at(abs + p as u64, piece).is_ok()
                    || self.inner.read_at(abs + p as u64, piece).is_ok();
                if ok {
                    failures = 0;
                } else {
                    piece.fill(0);
                    self.log.add(abs + p as u64, (e - p) as u64);
                    failures += 1;
                }
                p = e;
            }
            pos = block_end;
        }
    }
}

/// A buffer in memory.
pub struct MemSource(pub Vec<u8>);

impl BlockSource for MemSource {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let start = usize::try_from(offset).map_err(|_| io::ErrorKind::UnexpectedEof)?;
        let end = start
            .checked_add(buf.len())
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        let src = self.0.get(start..end).ok_or(io::ErrorKind::UnexpectedEof)?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

/// A buffer with unreadable areas: stands in for a failing disk in tests and
/// in the self-test.
pub struct FlakySource {
    pub data: Vec<u8>,
    /// `(offset, length)` of areas whose reads always fail.
    pub bad: Vec<(u64, u64)>,
    /// Every read fails the first time it is tried (exercises the retry).
    pub fail_first: bool,
    pub attempts: AtomicU64,
}

impl BlockSource for FlakySource {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let n = self.attempts.fetch_add(1, Ordering::Relaxed);
        if self.fail_first && n.is_multiple_of(2) {
            return Err(io::Error::other("transient read error"));
        }
        let end = offset + buf.len() as u64;
        if self.bad.iter().any(|(o, l)| offset < o + l && *o < end) {
            return Err(io::Error::other("unreadable sector"));
        }
        MemSource::read_at_slice(&self.data, offset, buf)
    }
}

impl MemSource {
    fn read_at_slice(data: &[u8], offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let start = usize::try_from(offset).map_err(|_| io::ErrorKind::UnexpectedEof)?;
        let end = start
            .checked_add(buf.len())
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        buf.copy_from_slice(data.get(start..end).ok_or(io::ErrorKind::UnexpectedEof)?);
        Ok(())
    }
}

const SHARE_ALL: u32 = 7; // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE

fn read_full(file: &File, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let n = file.seek_read(buf, offset)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        offset += n as u64;
        buf = &mut buf[n..];
    }
    Ok(())
}

/// A disk image file (raw, or a fixed-size VHD whose 512-byte footer is left out).
pub struct ImageFile {
    file: File,
    len: u64,
}

impl BlockSource for ImageFile {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        read_full(&self.file, offset, buf)
    }
}

/// Opens a disk image for reading. Dynamic and differencing VHD files are
/// refused with an explanation: their blocks are not stored in disk order.
pub fn open_image(path: &Path) -> Result<Source, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_ALL)
        .open(path)
        .map_err(|e| format!("Could not open \"{}\": {e}", path.display()))?;
    let mut len = file
        .metadata()
        .map_err(|e| format!("Could not read the size of \"{}\": {e}", path.display()))?
        .len();
    if len == 0 {
        return Err(format!("\"{}\" is empty.", path.display()));
    }
    if len > 512 {
        let mut footer = [0u8; 512];
        if read_full(&file, len - 512, &mut footer).is_ok() && &footer[..8] == b"conectix" {
            match u32::from_be_bytes([footer[60], footer[61], footer[62], footer[63]]) {
                2 => len -= 512,
                3 | 4 => {
                    return Err(
                        "This is a dynamically expanding or differencing VHD. Only fixed-size VHD files can be scanned; convert it to a fixed-size VHD or a raw image first."
                            .to_string(),
                    )
                }
                _ => {}
            }
        }
    }
    Ok(Source::new(Arc::new(ImageFile { file, len })))
}

/// A volume (`\\.\D:`) or a physical disk (`\\.\PhysicalDrive1`).
pub struct RawDevice {
    file: File,
    len: u64,
    sector: u32,
}

impl BlockSource for RawDevice {
    fn len(&self) -> u64 {
        self.len
    }
    fn sector(&self) -> u32 {
        self.sector
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        // Raw devices only accept whole sectors at sector-aligned offsets.
        let s = self.sector as u64;
        if offset.is_multiple_of(s) && (buf.len() as u64).is_multiple_of(s) {
            return read_full(&self.file, offset, buf);
        }
        let start = offset - offset % s;
        let end = (offset + buf.len() as u64).div_ceil(s) * s;
        let mut tmp = vec![0u8; (end - start) as usize];
        read_full(&self.file, start, &mut tmp)?;
        let skip = (offset - start) as usize;
        buf.copy_from_slice(&tmp[skip..skip + buf.len()]);
        Ok(())
    }
}

/// Opens a raw device for reading. Needs administrator rights.
pub fn open_device(device_path: &str) -> Result<Source, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_ALL)
        .open(device_path)
        .map_err(|e| {
            if e.kind() == io::ErrorKind::PermissionDenied {
                "Windows refused direct access to this drive. Restart RescueForge as administrator and try again.".to_string()
            } else {
                format!("Could not open the drive for reading: {e}")
            }
        })?;
    let (len, sector) = crate::sys::device_geometry(&file)
        .ok_or_else(|| "Could not read the size of this drive.".to_string())?;
    if len == 0 {
        return Err("This drive reports a size of zero (no media?).".to_string());
    }
    crate::sys::allow_extended_io(&file);
    Ok(Source::new(Arc::new(RawDevice {
        file,
        len: len - len % sector as u64,
        sector,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn reads_slices_and_clamps() {
        let s = Source::from_bytes(pattern(10_000));
        assert_eq!(s.len(), 10_000);
        let mut buf = [0xAAu8; 16];
        assert_eq!(s.read_at(9_990, &mut buf), 10);
        assert_eq!(&buf[..10], &pattern(10_000)[9_990..]);
        assert_eq!(&buf[10..], &[0u8; 6]);
        assert_eq!(s.read_at(20_000, &mut buf), 0);
        assert_eq!(buf, [0u8; 16]);
        assert_eq!(s.read_vec(9_998, 100).len(), 2);
        assert!(s.read_exact_vec(9_998, 100).is_none());
        assert!(s.read_exact_vec(u64::MAX, 2).is_none());

        let part = s.slice(1_000, 500);
        assert_eq!(part.base(), 1_000);
        assert_eq!(part.len(), 500);
        assert_eq!(part.read_vec(0, 4), pattern(10_000)[1_000..1_004]);
        assert_eq!(part.read_vec(498, 100).len(), 2);
        let nested = part.slice(100, 10_000);
        assert_eq!((nested.base(), nested.len()), (1_100, 400));
        assert_eq!(part.slice(9_999, 5).len(), 0);
        assert_eq!(nested.root().len(), 10_000);
    }

    #[test]
    fn bad_sectors_become_zeros_and_are_logged() {
        let data = pattern(300_000);
        let s = Source::new(Arc::new(FlakySource {
            data: data.clone(),
            bad: vec![(1024, 512), (199_680, 100_320)],
            fail_first: false,
            attempts: AtomicU64::new(0),
        }));
        let got = s.read_vec(0, 300_000);
        assert_eq!(got.len(), 300_000);
        assert!(got[..1024] == data[..1024]);
        assert!(got[1024..1536].iter().all(|b| *b == 0));
        assert!(got[1536..199_680] == data[1536..199_680]);
        assert!(got[199_680..].iter().all(|b| *b == 0));
        assert_eq!(s.log().bad_bytes(), 512 + 100_320);
        let ranges = s.log().ranges();
        assert_eq!(ranges[0], (1024, 512));
        assert_eq!(ranges[1].0, 199_680);
        assert_eq!(ranges.iter().map(|r| r.1).sum::<u64>(), 100_832);
        // A dead area is given up in blocks, not probed sector by sector.
        let src = FlakySource {
            data: vec![7u8; 4 << 20],
            bad: vec![(0, 4 << 20)],
            fail_first: false,
            attempts: AtomicU64::new(0),
        };
        let attempts = {
            let s = Source::new(Arc::new(src));
            let v = s.read_vec(0, 4 << 20);
            assert!(v.iter().all(|b| *b == 0));
            s.log().bad_bytes()
        };
        assert_eq!(attempts, 4 << 20);
    }

    #[test]
    fn raw_devices_are_read_in_whole_sectors() {
        // A plain file stands in for the device; the wrapper must turn any
        // request into sector-aligned reads and still return the right bytes.
        let dir = std::env::temp_dir().join(format!("rescueforge-raw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("device.bin");
        let data = pattern(64 * 1024);
        std::fs::write(&path, &data).unwrap();
        for sector in [512u32, 4096] {
            let dev = RawDevice {
                file: File::open(&path).unwrap(),
                len: data.len() as u64,
                sector,
            };
            let s = Source::new(Arc::new(dev));
            assert!(s.read_vec(0, 8192) == data[..8192]);
            assert!(s.read_vec(5, 3) == data[5..8]);
            assert!(s.read_vec(4095, 4098) == data[4095..8193]);
            assert!(s.read_vec(60_000, 9_999) == data[60_000..]);
            assert_eq!(s.log().bad_bytes(), 0);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opening_a_real_drive_is_read_only_and_explains_missing_rights() {
        // Read-only smoke test on the system drive: at most one sector is read.
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        match open_device(&format!("\\\\.\\{drive}")) {
            Ok(s) => {
                assert!(crate::sys::is_elevated());
                assert!(!s.is_empty());
                assert_eq!(s.read_vec(0, 512).len(), 512);
            }
            Err(e) => {
                assert!(!crate::sys::is_elevated(), "{e}");
                assert!(e.contains("administrator"), "{e}");
            }
        }
    }

    #[test]
    fn transient_errors_are_retried() {
        let data = pattern(50_000);
        let s = Source::new(Arc::new(FlakySource {
            data: data.clone(),
            bad: vec![],
            fail_first: true,
            attempts: AtomicU64::new(0),
        }));
        assert!(s.read_vec(100, 40_000) == data[100..40_100]);
        assert_eq!(s.log().bad_bytes(), 0);
        assert!(s.log().retries() >= 1);
    }

    #[test]
    fn image_files_and_vhd_footers() {
        let dir = std::env::temp_dir().join(format!("rescueforge-source-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("disk.img");
        std::fs::write(&raw, pattern(4096)).unwrap();
        let s = open_image(&raw).unwrap();
        assert_eq!(s.len(), 4096);
        assert_eq!(s.read_vec(10, 5), pattern(4096)[10..15]);

        let mut vhd = pattern(4096);
        let mut footer = vec![0u8; 512];
        footer[..8].copy_from_slice(b"conectix");
        footer[60..64].copy_from_slice(&2u32.to_be_bytes());
        vhd.extend_from_slice(&footer);
        let fixed = dir.join("disk.vhd");
        std::fs::write(&fixed, &vhd).unwrap();
        assert_eq!(open_image(&fixed).unwrap().len(), 4096);

        footer[60..64].copy_from_slice(&3u32.to_be_bytes());
        let mut dynamic = pattern(4096);
        dynamic.extend_from_slice(&footer);
        let dyn_path = dir.join("dynamic.vhd");
        std::fs::write(&dyn_path, &dynamic).unwrap();
        assert!(open_image(&dyn_path).unwrap_err().contains("fixed-size"));

        let empty = dir.join("empty.img");
        std::fs::write(&empty, b"").unwrap();
        assert!(open_image(&empty).is_err());
        assert!(open_image(&dir.join("missing.img")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
