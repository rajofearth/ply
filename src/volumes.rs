//! Volume discovery for This PC. [`discover`] / [`discover_lettered`] may block
//! on network drives (`GetDiskFreeSpaceExW` / `GetVolumeInformationW`) — call
//! off the UI thread. MTP is separate so drive polls stay cheap.

use std::path::{Path, PathBuf};

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

/// Re-query free/total for local `Drive` volumes, returning the ones whose
/// numbers changed (with the fresh numbers). Network drives and MTP are never
/// touched, so this never blocks on a hung share.
#[cfg(windows)]
pub fn refresh_local_sizes(volumes: &[Volume]) -> Vec<Volume> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(
            root: *const u16,
            free: *mut u64,
            total: *mut u64,
            free_tot: *mut u64,
        ) -> i32;
    }

    volumes
        .iter()
        .filter(|v| v.kind == VolumeKind::Drive)
        .filter_map(|v| {
            let wide: Vec<u16> = std::ffi::OsStr::new(&v.path)
                .encode_wide()
                .chain(Some(0))
                .collect();
            let (mut free, mut total, mut free_tot) = (0u64, 0u64, 0u64);
            // SAFETY: valid locals.
            let ok =
                unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut free_tot) };
            (ok != 0 && total != 0).then_some((v, free, total))
        })
        .filter_map(|(v, free, total)| {
            (v.free != free || v.total != total).then_some(Volume {
                free,
                total,
                ..v.clone()
            })
        })
        .collect()
}

#[cfg(not(windows))]
pub fn refresh_local_sizes(_volumes: &[Volume]) -> Vec<Volume> {
    Vec::new()
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

/// Cap for pinned Quick Access folders, in memory and on disk.
pub const MAX_QUICK_ACCESS: usize = 64;

/// Persisted pins: `<config_dir>/ply/quick_access.txt`, falling back to
/// `<data_dir>/ply/quick_access.txt` when no config dir exists.
pub fn quick_access_path() -> PathBuf {
    dirs::config_dir()
        .or_else(dirs::data_dir)
        .map(|d| d.join("ply").join("quick_access.txt"))
        .unwrap_or_else(|| std::env::temp_dir().join("ply").join("quick_access.txt"))
}

/// Parse persisted pins, one path per line. Trims whitespace, skips empties,
/// dedupes (first wins), caps at [`MAX_QUICK_ACCESS`]. Pure, no filesystem
/// checks; `is_dir` / MTP / Recycle Bin filtering happens in `load_or_seed`.
pub fn parse_quick_access_text(text: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let path = PathBuf::from(line);
        if !out.contains(&path) {
            out.push(path);
        }
        if out.len() >= MAX_QUICK_ACCESS {
            break;
        }
    }
    out
}

/// Serialize pins as one display string per line. Capped at
/// [`MAX_QUICK_ACCESS`]; inverse of [`parse_quick_access_text`].
pub fn serialize_quick_access(pins: &[PathBuf]) -> String {
    let mut text = String::new();
    for pin in pins.iter().take(MAX_QUICK_ACCESS) {
        text.push_str(&pin.display().to_string());
        text.push('\n');
    }
    text
}

/// Persist pins. Best-effort: creates parent dirs, ignores all IO errors so a
/// disk failure never changes app behavior.
pub fn save_quick_access(pins: &[PathBuf]) {
    save_quick_access_at(&quick_access_path(), pins);
}

fn save_quick_access_at(path: &Path, pins: &[PathBuf]) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, serialize_quick_access(pins));
}

/// Load persisted pins, or seed from shell folders on first run / corrupt
/// file. A present file wins exactly as written (trimmed, deduped, capped at
/// [`MAX_QUICK_ACCESS`], MTP / Recycle Bin / missing paths dropped), so an
/// unpinned seed stays unpinned. Absent or unreadable files fall back to
/// [`default_quick_access`] plus a best-effort save.
pub fn load_or_seed() -> Vec<PathBuf> {
    load_or_seed_at(&quick_access_path())
}

fn load_or_seed_at(path: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(path) else {
        let seed = default_quick_access();
        save_quick_access_at(path, &seed);
        return seed;
    };
    let mut pins: Vec<PathBuf> = parse_quick_access_text(&text)
        .into_iter()
        .filter(|p| !crate::mtp::is_mtp(p) && !crate::recycle_bin::is_recycle_bin(p) && p.is_dir())
        .collect();
    pins.truncate(MAX_QUICK_ACCESS);
    pins
}

#[cfg(windows)]
fn discover_windows_lettered() -> Vec<Volume> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
        fn GetDiskFreeSpaceExW(
            root: *const u16,
            free: *mut u64,
            total: *mut u64,
            free_tot: *mut u64,
        ) -> i32;
        fn GetVolumeInformationW(
            root: *const u16,
            name: *mut u16,
            name_len: u32,
            serial: *mut u32,
            max_comp: *mut u32,
            flags: *mut u32,
            fs_name: *mut u16,
            fs_name_len: u32,
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
        let wide: Vec<u16> = std::ffi::OsStr::new(&root)
            .encode_wide()
            .chain(Some(0))
            .collect();

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
        let ok =
            unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut free_tot) };
        // Skip empty CD/card readers and inaccessible volumes.
        if ok == 0 || total == 0 {
            continue;
        }

        let mut buf = [0u16; 261];
        // SAFETY: MAX_PATH+1 buffer; unused out-params null.
        let label = if unsafe {
            GetVolumeInformationW(
                wide.as_ptr(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
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
        Volume {
            name: "t".into(),
            path: PathBuf::from("/"),
            kind: VolumeKind::Drive,
            free,
            total,
        }
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

    #[test]
    fn refresh_local_sizes_never_touches_network_or_mtp() {
        let volumes = vec![
            Volume {
                name: "C".into(),
                path: PathBuf::from(r"C:\"),
                kind: VolumeKind::Drive,
                free: 10,
                total: 100,
            },
            Volume {
                name: "N".into(),
                path: PathBuf::from(r"Z:\"),
                kind: VolumeKind::Network,
                free: 5,
                total: 10,
            },
            Volume {
                name: "Phone".into(),
                path: PathBuf::from(r"\\MTP\abc"),
                kind: VolumeKind::Device,
                free: 0,
                total: 0,
            },
        ];
        // Nothing here changes on disk, so the guaranteed outcome is that
        // unchanged local drives, plus all network/MTP, are never reported back.
        let updated = refresh_local_sizes(&volumes);
        for v in &updated {
            assert_eq!(v.kind, VolumeKind::Drive, "only local drives refresh");
        }
    }

    fn quick_access_temp(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "ply_quick_access_{label}_{}_{}",
            std::process::id(),
            n
        ))
    }

    #[test]
    fn quick_access_round_trip() {
        let pins = vec![PathBuf::from("/tmp/ply-a"), PathBuf::from("/tmp/ply-b")];
        let text = serialize_quick_access(&pins);
        assert_eq!(parse_quick_access_text(&text), pins);
    }

    #[test]
    fn quick_access_parse_dedupes_and_skips_empties() {
        let text = "/tmp/a\n\n  /tmp/b  \n/tmp/a\n/tmp/b\n";
        assert_eq!(
            parse_quick_access_text(text),
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
    }

    #[test]
    fn quick_access_parse_caps_at_64() {
        let text = (0..100)
            .map(|i| format!("/tmp/dir{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed = parse_quick_access_text(&text);
        assert_eq!(parsed.len(), MAX_QUICK_ACCESS);
        assert_eq!(parsed[0], PathBuf::from("/tmp/dir0"));
        assert_eq!(parsed[63], PathBuf::from("/tmp/dir63"));
        let serialized = serialize_quick_access(
            &(0..100)
                .map(|i| PathBuf::from(format!("/tmp/dir{i}")))
                .collect::<Vec<_>>(),
        );
        assert_eq!(parse_quick_access_text(&serialized).len(), MAX_QUICK_ACCESS);
    }

    #[test]
    fn quick_access_missing_file_falls_back_to_seed() {
        let dir = quick_access_temp("missing");
        let path = dir.join("nested").join("quick_access.txt");
        let loaded = load_or_seed_at(&path);
        assert_eq!(loaded, default_quick_access());
        // Best-effort seed save leaves a file behind when the dir is writable.
        assert_eq!(
            std::fs::read_to_string(&path).ok().as_deref(),
            Some(serialize_quick_access(&loaded).as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quick_access_corrupt_file_falls_back_to_seed() {
        let dir = quick_access_temp("corrupt");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("quick_access.txt");
        std::fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();
        assert_eq!(load_or_seed_at(&path), default_quick_access());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quick_access_drops_mtp_recycle_and_missing() {
        let dir = quick_access_temp("filter");
        let keep = dir.join("keep");
        let _ = std::fs::create_dir_all(&keep);
        let missing = dir.join("missing");
        let path = dir.join("quick_access.txt");
        let text = serialize_quick_access(&[
            keep.clone(),
            crate::mtp::root_of("deadbeefdeadbeef"),
            crate::recycle_bin::root(),
            missing,
        ]);
        std::fs::write(&path, text).unwrap();
        let loaded = load_or_seed_at(&path);
        assert!(loaded.contains(&keep));
        assert!(!loaded.iter().any(|p| crate::mtp::is_mtp(p)));
        assert!(!loaded.iter().any(|p| crate::recycle_bin::is_recycle_bin(p)));
        for seed in default_quick_access() {
            assert!(
                !loaded.contains(&seed),
                "unpinned seed stays gone: {seed:?}"
            );
        }
        assert!(loaded.len() <= MAX_QUICK_ACCESS);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quick_access_save_load_round_trip() {
        let dir = quick_access_temp("roundtrip");
        let a = dir.join("a");
        let b = dir.join("b");
        let _ = std::fs::create_dir_all(&a);
        let _ = std::fs::create_dir_all(&b);
        let path = dir.join("quick_access.txt");
        save_quick_access_at(&path, &[a.clone(), b.clone()]);
        let loaded = load_or_seed_at(&path);
        assert!(loaded.contains(&a));
        assert!(loaded.contains(&b));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
