//! Thin wrappers over the Win32 calls RescueForge needs. Everything here
//! either queries information or opens something for reading.

use std::ffi::c_void;
use std::fs::File;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const IOCTL_DISK_GET_DRIVE_GEOMETRY_EX: u32 = 0x0007_00A0;
const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405C;
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
const FSCTL_ALLOW_EXTENDED_DASD_IO: u32 = 0x0009_0083;
const SHARE_ALL: u32 = 7;

/// UTF-16, NUL-terminated.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Is this process running with administrator rights?
pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

fn shell_execute(verb: &str, file: &str, params: &str) -> Result<(), String> {
    let (v, f, p) = (wide(verb), wide(file), wide(params));
    let r = unsafe {
        ShellExecuteW(
            HWND::default(),
            PCWSTR(v.as_ptr()),
            PCWSTR(f.as_ptr()),
            if params.is_empty() {
                PCWSTR::null()
            } else {
                PCWSTR(p.as_ptr())
            },
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if r.0 as usize > 32 {
        Ok(())
    } else {
        Err(format!(
            "Windows could not open \"{file}\" (code {}).",
            r.0 as usize
        ))
    }
}

/// Opens a folder, file or web address with its default program.
pub fn open_path(target: &str) -> Result<(), String> {
    shell_execute("open", target, "")
}

/// Starts a new elevated copy of RescueForge. The caller should then close this one.
pub fn restart_as_admin() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    shell_execute("runas", &exe.to_string_lossy(), "")
}

/// Text in the system's own "ANSI" code page (what Notepad used to save
/// before UTF-8 became the default), as a string.
pub fn ansi_to_string(bytes: &[u8]) -> String {
    use windows::Win32::Globalization::{
        MultiByteToWideChar, CP_ACP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS,
    };
    if bytes.is_empty() {
        return String::new();
    }
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    // SAFETY: both calls get valid slices; the second buffer has the size the first call asked for.
    unsafe {
        let n = MultiByteToWideChar(CP_ACP, flags, bytes, None);
        if n <= 0 {
            return bytes.iter().map(|b| *b as char).collect();
        }
        let mut wide = vec![0u16; n as usize];
        let n = MultiByteToWideChar(CP_ACP, flags, bytes, Some(&mut wide));
        String::from_utf16_lossy(&wide[..n.max(0) as usize])
    }
}

/// The local offset from UTC in seconds (east is positive).
pub fn tz_offset_secs() -> i32 {
    let mut info = TIME_ZONE_INFORMATION::default();
    let id = unsafe { GetTimeZoneInformation(&mut info) };
    let bias = match id {
        1 => info.Bias + info.StandardBias,
        2 => info.Bias + info.DaylightBias,
        0 => info.Bias,
        _ => 0,
    };
    -bias * 60
}

fn ioctl(file: &File, code: u32, input: &[u8], output: &mut [u8]) -> Option<usize> {
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            HANDLE(file.as_raw_handle()),
            code,
            if input.is_empty() {
                None
            } else {
                Some(input.as_ptr() as *const c_void)
            },
            input.len() as u32,
            if output.is_empty() {
                None
            } else {
                Some(output.as_mut_ptr() as *mut c_void)
            },
            output.len() as u32,
            Some(&mut returned),
            None,
        )
    };
    ok.ok().map(|_| returned as usize)
}

/// Opens a device only to ask questions about it (no read or write access).
fn open_for_query(path: &str) -> Option<File> {
    std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(SHARE_ALL)
        .open(path)
        .ok()
}

/// `(size in bytes, bytes per sector)` of an open volume or disk.
pub fn device_geometry(file: &File) -> Option<(u64, u32)> {
    let mut geo = [0u8; 64];
    let mut sector = 512u32;
    let mut size = 0u64;
    if ioctl(file, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &[], &mut geo).is_some() {
        let s = u32::from_le_bytes([geo[20], geo[21], geo[22], geo[23]]);
        if s.is_power_of_two() && (512..=4096).contains(&s) {
            sector = s;
        }
        size = crate::util::le64(&geo, 24).unwrap_or(0);
    }
    let mut len = [0u8; 8];
    if ioctl(file, IOCTL_DISK_GET_LENGTH_INFO, &[], &mut len).is_some() {
        size = u64::from_le_bytes(len);
    }
    if size == 0 || size > (1u64 << 60) {
        None
    } else {
        Some((size, sector))
    }
}

/// Lets reads reach the last sectors of a volume, which lie outside the file
/// system. This changes how this one handle behaves, nothing on the disk.
pub fn allow_extended_io(file: &File) {
    let _ = ioctl(file, FSCTL_ALLOW_EXTENDED_DASD_IO, &[], &mut []);
}

fn storage_query(file: &File, property: u32, out: &mut [u8]) -> Option<usize> {
    let mut query = [0u8; 12];
    query[..4].copy_from_slice(&property.to_le_bytes());
    ioctl(file, IOCTL_STORAGE_QUERY_PROPERTY, &query, out)
}

fn c_string_at(buf: &[u8], offset: usize) -> String {
    if offset == 0 || offset >= buf.len() {
        return String::new();
    }
    let end = buf[offset..]
        .iter()
        .position(|b| *b == 0)
        .map(|p| offset + p)
        .unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[offset..end])
        .trim()
        .to_string()
}

/// What the hardware says about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hardware {
    pub model: String,
    pub bus: String,
    pub removable: bool,
    /// No seek penalty: a solid-state drive.
    pub solid_state: bool,
    /// The drive erases deleted blocks on its own (TRIM).
    pub trim: bool,
}

fn hardware(file: &File) -> Hardware {
    let mut hw = Hardware::default();
    let mut buf = [0u8; 1024];
    if let Some(n) = storage_query(file, 0, &mut buf) {
        let b = &buf[..n.min(buf.len())];
        if b.len() >= 36 {
            let vendor = c_string_at(b, crate::util::le32(b, 12).unwrap_or(0) as usize);
            let product = c_string_at(b, crate::util::le32(b, 16).unwrap_or(0) as usize);
            hw.model = format!("{vendor} {product}").trim().to_string();
            hw.removable = b[10] != 0;
            hw.bus = match crate::util::le32(b, 28).unwrap_or(0) {
                1 => "SCSI",
                2 => "ATAPI",
                3 => "ATA",
                7 => "USB",
                8 => "RAID",
                10 => "SAS",
                11 => "SATA",
                12 => "SD card",
                13 => "MMC",
                14 | 15 => "Virtual",
                17 => "NVMe",
                _ => "",
            }
            .to_string();
        }
    }
    let mut small = [0u8; 16];
    if storage_query(file, 7, &mut small).is_some() {
        hw.solid_state = small[8] == 0;
    }
    let mut small = [0u8; 16];
    if storage_query(file, 8, &mut small).is_some() {
        hw.trim = small[8] != 0;
    }
    hw
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveInfo {
    /// "D"
    pub letter: String,
    pub label: String,
    /// "NTFS", "FAT32", "exFAT", or "" when Windows cannot read the volume.
    pub fs: String,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
    /// `\\?\Volume{...}\`, used to tell volumes apart reliably.
    pub guid: String,
    /// Numbers of the physical disks this volume lives on.
    pub disks: Vec<u32>,
    pub hw: Hardware,
}

impl DriveInfo {
    /// The path that opens the raw volume.
    pub fn device_path(&self) -> String {
        format!("\\\\.\\{}:", self.letter)
    }
}

fn volume_guid(root: &str) -> String {
    let mut buf = [0u16; 80];
    let w = wide(root);
    if unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(w.as_ptr()), &mut buf) }.is_ok() {
        from_wide(&buf)
    } else {
        String::new()
    }
}

fn volume_disks(file: &File) -> Vec<u32> {
    let mut buf = [0u8; 8 + 24 * 32];
    let mut disks = Vec::new();
    if ioctl(file, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, &[], &mut buf).is_some() {
        let n = (crate::util::le32(&buf, 0).unwrap_or(0) as usize).min(32);
        for i in 0..n {
            if let Some(d) = crate::util::le32(&buf, 8 + i * 24) {
                if !disks.contains(&d) {
                    disks.push(d);
                }
            }
        }
    }
    disks
}

/// Fixed and removable drives that have a drive letter.
pub fn list_drives() -> Vec<DriveInfo> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let w = wide(&root);
        let kind = unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) };
        // 2 removable, 3 fixed, 6 RAM disk. Network and optical drives are left out.
        if !matches!(kind, 2 | 3 | 6) {
            continue;
        }
        let mut label = [0u16; 261];
        let mut fs = [0u16; 261];
        let readable = unsafe {
            GetVolumeInformationW(
                PCWSTR(w.as_ptr()),
                Some(&mut label),
                None,
                None,
                None,
                Some(&mut fs),
            )
        }
        .is_ok();
        let (mut total, mut free) = (0u64, 0u64);
        if readable {
            let _ = unsafe {
                GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), None, Some(&mut total), Some(&mut free))
            };
        }
        let query = open_for_query(&format!("\\\\.\\{letter}:"));
        let (disks, hw) = match &query {
            Some(f) => (volume_disks(f), hardware(f)),
            None => (Vec::new(), Hardware::default()),
        };
        if !readable && query.is_none() {
            // An empty card reader slot: nothing to scan.
            continue;
        }
        out.push(DriveInfo {
            letter: letter.to_string(),
            label: if readable {
                from_wide(&label)
            } else {
                String::new()
            },
            fs: if readable {
                from_wide(&fs)
            } else {
                String::new()
            },
            total,
            free,
            removable: kind == 2,
            guid: volume_guid(&root),
            disks,
            hw,
        });
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskInfo {
    pub number: u32,
    pub size: u64,
    pub sector: u32,
    pub hw: Hardware,
}

impl DiskInfo {
    pub fn device_path(&self) -> String {
        format!("\\\\.\\PhysicalDrive{}", self.number)
    }
}

/// Physical disks 0..=31 that are present.
pub fn list_disks() -> Vec<DiskInfo> {
    let mut out = Vec::new();
    for number in 0..32u32 {
        let Some(file) = open_for_query(&format!("\\\\.\\PhysicalDrive{number}")) else {
            continue;
        };
        let mut geo = [0u8; 64];
        if ioctl(&file, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &[], &mut geo).is_none() {
            continue;
        }
        let sector = crate::util::le32(&geo, 20).unwrap_or(512);
        let size = crate::util::le64(&geo, 24).unwrap_or(0);
        if size == 0 {
            continue;
        }
        out.push(DiskInfo {
            number,
            size,
            sector: if sector.is_power_of_two() && (512..=4096).contains(&sector) {
                sector
            } else {
                512
            },
            hw: hardware(&file),
        });
    }
    out
}

/// Where a folder or file lives: used to keep recovered files off the drive
/// they are being recovered from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DestInfo {
    /// "E:\\" (or a mount point folder).
    pub root: String,
    pub guid: String,
    pub disks: Vec<u32>,
    pub free: Option<u64>,
}

pub fn dest_info(path: &Path) -> DestInfo {
    let mut info = DestInfo::default();
    // The folder may not exist yet: walk up to the nearest ancestor that does.
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        if !probe.pop() {
            break;
        }
    }
    let w = wide(&probe.to_string_lossy());
    let mut root = [0u16; 300];
    if unsafe { GetVolumePathNameW(PCWSTR(w.as_ptr()), &mut root) }.is_err() {
        return info;
    }
    info.root = from_wide(&root);
    info.guid = volume_guid(&info.root);
    let rw = wide(&info.root);
    let mut free = 0u64;
    if unsafe { GetDiskFreeSpaceExW(PCWSTR(rw.as_ptr()), Some(&mut free), None, None) }.is_ok() {
        info.free = Some(free);
    }
    // `\\?\Volume{guid}\` without the trailing slash opens the volume device.
    let device = if info.guid.is_empty() {
        let letter: String = info.root.chars().take(2).collect();
        format!("\\\\.\\{letter}")
    } else {
        info.guid.trim_end_matches('\\').to_string()
    };
    if let Some(f) = open_for_query(&device) {
        info.disks = volume_disks(&f);
    }
    info
}

/// The folder for settings: `%LOCALAPPDATA%\RescueForge`.
pub fn app_dir() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RescueForge")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_drives_is_safe_and_sane() {
        // Read-only smoke test: asks Windows which drives exist.
        let drives = list_drives();
        for d in &drives {
            assert_eq!(d.letter.len(), 1);
            assert!(d.free <= d.total);
            assert!(d.device_path().starts_with("\\\\.\\"));
        }
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        assert!(
            drives.iter().any(|d| system.starts_with(&d.letter)),
            "the system drive is listed"
        );
        for d in list_disks() {
            assert!(d.size > 0);
            assert!(d.sector >= 512);
        }
    }

    #[test]
    fn destination_info() {
        let tmp = std::env::temp_dir();
        let info = dest_info(&tmp.join("does-not-exist-yet").join("deeper"));
        assert!(info.root.ends_with('\\'), "{info:?}");
        assert!(info.free.is_some());
        let tz = tz_offset_secs();
        assert!((-14 * 3600..=14 * 3600).contains(&tz));
        assert!(c_string_at(b"abc\0def\0", 4) == "def");
        assert!(c_string_at(b"abc", 9).is_empty());
    }
}
