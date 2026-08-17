//! Volume discovery for This PC. [`discover`] may block on network drives
//! (`GetDiskFreeSpaceExW` / `GetVolumeInformationW`) — call off the UI thread.

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
}

/// Mounted volumes: drives, then devices, then network.
/// May block on unreachable network shares — run on a background executor.
pub fn discover() -> Vec<Volume> {
    #[cfg(windows)]
    {
        discover_windows()
    }
    #[cfg(not(windows))]
    {
        discover_unix()
    }
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
fn discover_windows() -> Vec<Volume> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
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
    // SAFETY: kernel32 drive bitmask.
    let mask = unsafe { GetLogicalDrives() };
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
    // Phones and cameras have no drive letter, so they come from WPD instead.
    devices.extend(crate::mtp::devices().into_iter().map(|d| Volume {
        path: d.root(),
        name: d.name,
        kind: VolumeKind::Device,
        free: d.free,
        total: d.total,
    }));
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
    fn quick_access_existing_dirs() {
        for p in default_quick_access() {
            assert!(p.is_dir(), "{p:?}");
        }
    }
}
