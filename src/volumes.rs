//! Volume discovery for This PC. [`discover`] / [`discover_lettered`] may block
//! on network drives (`GetDiskFreeSpaceExW` / `GetVolumeInformationW`) — call
//! off the UI thread. MTP is separate so drive polls stay cheap.

use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VolumeKind {
    Drive,
    Device,
    Network,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// e.g. `"Windows-SSD (C:)"`; unlabeled → `"Local Disk (D:)"` etc.
    pub name: String,
    pub path: PathBuf,
    pub kind: VolumeKind,
    pub free: u64,
    pub total: u64,
}

impl Volume {
    /// Percent used, `0.0..=100.0`. Returns `0.0` when `total` is 0.
    pub fn pct_used(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        let used = self.total.saturating_sub(self.free);
        (used as f64 / self.total as f64 * 100.0) as f32
    }

    /// Sidebar / home icon for this volume's kind.
    pub fn ico(&self) -> crate::icons::Ico {
        volume_icon(self.kind)
    }
}

/// Icon for a [`VolumeKind`] (Drive / Device / Network).
pub fn volume_icon(kind: VolumeKind) -> crate::icons::Ico {
    use crate::icons::Ico;
    match kind {
        VolumeKind::Drive => Ico::HardDrive,
        VolumeKind::Device => Ico::Usb,
        VolumeKind::Network => Ico::Network,
    }
}

/// Partition into lettered drives vs devices & network (home + sidebar sections).
pub fn partition_drives_devices(volumes: &[Volume]) -> (Vec<&Volume>, Vec<&Volume>) {
    let mut drives = Vec::new();
    let mut devices = Vec::new();
    for v in volumes {
        if v.kind == VolumeKind::Drive {
            drives.push(v);
        } else {
            devices.push(v);
        }
    }
    (drives, devices)
}

/// Bitmask of present drive letters (`GetLogicalDrives`). `0` off Windows.
pub fn logical_drives_mask() -> u32 {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetLogicalDrives() -> u32;
        }
        // SAFETY: kernel32 drive bitmask; no pointers.
        unsafe { GetLogicalDrives() }
    }
    #[cfg(not(windows))]
    {
        0
    }
}

/// Lettered / mounted volumes only — no MTP. May block on network shares.
pub fn discover_lettered() -> Vec<Volume> {
    #[cfg(windows)]
    {
        discover_windows_lettered()
    }
    #[cfg(not(windows))]
    {
        discover_unix()
    }
}

/// Portable devices (phones, cameras) with no drive letter.
pub fn discover_mtp_devices() -> Vec<Volume> {
    crate::mtp::devices()
        .into_iter()
        .map(|d| Volume {
            path: d.root(),
            name: d.name,
            kind: VolumeKind::Device,
            free: d.free,
            total: d.total,
        })
        .collect()
}

/// Merge lettered volumes with MTP devices, keeping Drive → Device → Network
/// order and inserting MTP ahead of network shares.
pub fn merge_lettered_and_mtp(mut lettered: Vec<Volume>, mtp: Vec<Volume>) -> Vec<Volume> {
    let network_start = lettered
        .iter()
        .position(|v| v.kind == VolumeKind::Network)
        .unwrap_or(lettered.len());
    lettered.splice(network_start..network_start, mtp);
    lettered
}

/// Mounted volumes: lettered drives plus MTP. May block on network shares.
pub fn discover() -> Vec<Volume> {
    merge_lettered_and_mtp(discover_lettered(), discover_mtp_devices())
}

/// Desktop, Downloads, Documents, Pictures, Music, Videos (existing only).
pub fn default_quick_access() -> Vec<PathBuf> {
    [
        dirs::desktop_dir(),
        dirs::download_dir(),
        dirs::document_dir(),
        dirs::picture_dir(),
        dirs::audio_dir(),
        dirs::video_dir(),
    ]
    .into_iter()
    .flatten()
    .filter(|p| p.is_dir())
    .collect()
}

#[cfg(windows)]
fn discover_windows_lettered() -> Vec<Volume> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
        fn GetDiskFreeSpaceExW(root: *const u16, free: *mut u64, total: *mut u64, free_tot: *mut u64) -> i32;
        fn GetVolumeInformationW(
            root: *const u16, name: *mut u16, name_len: u32, serial: *mut u32, max_comp: *mut u32,
            flags: *mut u32, fs_name: *mut u16, fs_name_len: u32,
        ) -> i32;
    }

    const UNKNOWN: u32 = 0;
    const NO_ROOT: u32 = 1;
    const REMOVABLE: u32 = 2;
    const REMOTE: u32 = 4;
    const CDROM: u32 = 5;

    let mut by_kind: [Vec<Volume>; 3] = Default::default();
    let mask = logical_drives_mask();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = std::ffi::OsStr::new(&root).encode_wide().chain(Some(0)).collect();

        // SAFETY: NUL-terminated root path.
        let dtype = unsafe { GetDriveTypeW(wide.as_ptr()) };
        if dtype == UNKNOWN || dtype == NO_ROOT {
            continue;
        }
        let kind = match dtype {
            REMOVABLE | CDROM => VolumeKind::Device,
            REMOTE => VolumeKind::Network,
            _ => VolumeKind::Drive,
        };

        let (mut free, mut total, mut free_tot) = (0u64, 0u64, 0u64);
        // SAFETY: valid locals; may block on network drives.
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut free_tot) };
        // Skip empty CD/card readers and inaccessible volumes.
        if ok == 0 || total == 0 {
            continue;
        }

        let mut buf = [0u16; 261];
        // SAFETY: MAX_PATH+1 buffer; unused out-params null.
        let label = if unsafe {
            GetVolumeInformationW(
                wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32, std::ptr::null_mut(),
                std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0,
            )
        } != 0
        {
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..n])
        } else {
            String::new()
        };
        let base = if !label.is_empty() {
            label
        } else {
            match (kind, dtype) {
                (_, CDROM) => "CD Drive",
                (VolumeKind::Device, _) => "Removable Disk",
                (VolumeKind::Network, _) => "Network Drive",
                _ => "Local Disk",
            }
            .into()
        };

        by_kind[kind as usize].push(Volume {
            name: format!("{base} ({letter}:)"),
            path: PathBuf::from(root),
            kind,
            free,
            total,
        });
    }
    let [mut drives, mut devices, mut network] = by_kind;
    drives.append(&mut devices);
    drives.append(&mut network);
    drives
}

/// Minimal Unix fallback: `/`, plus home when on a different device.
#[cfg(not(windows))]
fn discover_unix() -> Vec<Volume> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    fn space(path: &std::path::Path) -> Option<(u64, u64)> {
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: NUL-terminated path; `st` is a valid out-param.
        unsafe {
            let mut st: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c.as_ptr(), &mut st) != 0 {
                return None;
            }
            let b = st.f_frsize as u64;
            let total = (st.f_blocks as u64).saturating_mul(b);
            let free = (st.f_bavail as u64).saturating_mul(b);
            (total > 0).then_some((free, total))
        }
    }

    let mut out = Vec::new();
    let root = PathBuf::from("/");
    if let Some((free, total)) = space(&root) {
        out.push(Volume {
            name: "System (/)".into(),
            path: root.clone(),
            kind: VolumeKind::Drive,
            free,
            total,
        });
    }
    if let Some(home) = dirs::home_dir() {
        let same = matches!(
            (std::fs::metadata(&root), std::fs::metadata(&home)),
            (Ok(r), Ok(h)) if r.dev() == h.dev()
        );
        if !same {
            if let Some((free, total)) = space(&home) {
                let leaf = home
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| home.display().to_string());
                out.push(Volume {
                    name: format!("Home ({leaf})"),
                    path: home,
                    kind: VolumeKind::Drive,
                    free,
                    total,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(free: u64, total: u64) -> Volume {
        Volume { name: "t".into(), path: PathBuf::from("/"), kind: VolumeKind::Drive, free, total }
    }

    #[test]
    fn pct_used_basic() {
        assert!((vol(25, 100).pct_used() - 75.0).abs() < f32::EPSILON);
    }

    #[test]
    fn pct_used_zero_total() {
        assert_eq!(vol(0, 0).pct_used(), 0.0);
    }

    #[test]
    fn discover_smoke() {
        let volumes = discover();
        assert!(!volumes.is_empty());
        for v in &volumes {
            // Portable devices may decline to report capacity.
            if v.kind != VolumeKind::Device {
                assert!(v.total > 0, "{:?} total == 0", v.path);
            }
        }
    }

    #[test]
    fn merge_keeps_mtp_before_network() {
        let lettered = vec![
            Volume {
                name: "C".into(),
                path: PathBuf::from(r"C:\"),
                kind: VolumeKind::Drive,
                free: 1,
                total: 2,
            },
            Volume {
                name: "N".into(),
                path: PathBuf::from(r"Z:\"),
                kind: VolumeKind::Network,
                free: 1,
                total: 2,
            },
        ];
        let mtp = vec![Volume {
            name: "Phone".into(),
            path: PathBuf::from(r"\\MTP\abc"),
            kind: VolumeKind::Device,
            free: 0,
            total: 0,
        }];
        let merged = merge_lettered_and_mtp(lettered, mtp);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[1].name, "Phone");
        assert_eq!(merged[2].kind, VolumeKind::Network);
    }

    #[test]
    fn quick_access_existing_dirs() {
        for p in default_quick_access() {
            assert!(p.is_dir(), "{p:?}");
        }
    }
}
