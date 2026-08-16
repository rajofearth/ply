use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(not(windows))]
use std::collections::HashSet;

/// A discoverable mount / drive shown on Home and under This PC.
#[derive(Clone, Debug)]
pub struct Volume {
    /// Stable id (typically the mount path string).
    pub id: String,
    /// Display name (e.g. "Local Disk (C:)" or "Home").
    pub name: String,
    /// Mount root to open as Current Folder / Workspace.
    pub path: PathBuf,
    pub kind: VolumeKind,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeKind {
    Drive,
    Device,
    Network,
}

impl Volume {
    /// Percent of capacity in use, 0–100. Returns 0 when total is unknown.
    pub fn pct_used(&self) -> u32 {
        if self.total_bytes == 0 {
            return 0;
        }
        let used = self.total_bytes.saturating_sub(self.free_bytes);
        ((used as u128 * 100) / self.total_bytes as u128) as u32
    }

    /// e.g. `"199 GB free"`.
    pub fn free_label(&self) -> String {
        format!("{} free", format_capacity(self.free_bytes))
    }

    /// e.g. `"618 GB"`.
    pub fn total_label(&self) -> String {
        format_capacity(self.total_bytes)
    }
}

/// Human-readable capacity for Home drive cards (whole units, Explorer-style).
fn format_capacity(n: u64) -> String {
    const KB: f64 = 1024.0;
    let n = n as f64;
    if n < KB {
        format!("{n:.0} B")
    } else if n < KB * KB {
        format!("{:.0} KB", n / KB)
    } else if n < KB * KB * KB {
        format!("{:.0} MB", n / (KB * KB))
    } else if n < KB * KB * KB * KB {
        format!("{:.0} GB", n / (KB * KB * KB))
    } else {
        format!("{:.0} TB", n / (KB * KB * KB * KB))
    }
}

/// Discover local volumes for Home / This PC.
pub fn discover_volumes() -> Vec<Volume> {
    #[cfg(windows)]
    {
        discover_volumes_windows()
    }
    #[cfg(not(windows))]
    {
        discover_volumes_unix()
    }
}

/// Quick-access folders that exist under the user home.
pub fn default_quick_access() -> Vec<PathBuf> {
    let candidates: [Option<PathBuf>; 6] = [
        dirs::desktop_dir(),
        dirs::download_dir(),
        dirs::document_dir(),
        dirs::picture_dir(),
        dirs::audio_dir(),
        dirs::video_dir(),
    ];
    candidates
        .into_iter()
        .flatten()
        .filter(|p| p.is_dir())
        .collect()
}

fn space_for(path: &Path) -> (u64, u64) {
    #[cfg(unix)]
    {
        if let Some(pair) = space_statvfs(path) {
            return pair;
        }
    }
    space_via_df(path).unwrap_or((0, 0))
}

#[cfg(unix)]
fn space_statvfs(path: &Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut st) != 0 {
            return None;
        }
        let frsize = st.f_frsize as u64;
        let total = (st.f_blocks as u64).saturating_mul(frsize);
        let free = (st.f_bavail as u64).saturating_mul(frsize);
        Some((free, total))
    }
}

/// Fallback when `statvfs` is unavailable or fails: `df -B1`.
fn space_via_df(path: &Path) -> Option<(u64, u64)> {
    let output = Command::new("df")
        .args(["-B1", "--output=size,avail"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // Skip header; take first data line.
    let line = text.lines().nth(1)?.trim();
    let mut parts = line.split_whitespace();
    let size: u64 = parts.next()?.parse().ok()?;
    let avail: u64 = parts.next()?.parse().ok()?;
    Some((avail, size))
}

#[cfg(unix)]
fn fs_dev(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

#[cfg(not(windows))]
fn discover_volumes_unix() -> Vec<Volume> {
    let mut volumes = Vec::new();
    let mut seen_devs: HashSet<u64> = HashSet::new();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();

    if let Some(home) = dirs::home_dir() {
        let name = format!(
            "Home ({})",
            home.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| home.display().to_string())
        );
        push_volume(
            &mut volumes,
            &mut seen_devs,
            &mut seen_paths,
            home,
            name,
            VolumeKind::Drive,
        );
    }

    let root = PathBuf::from("/");
    let root_dev = fs_dev(&root);
    let home_dev = dirs::home_dir().and_then(|h| fs_dev(&h));
    if volumes.is_empty() || (root_dev.is_some() && root_dev != home_dev) {
        push_volume(
            &mut volumes,
            &mut seen_devs,
            &mut seen_paths,
            root,
            "System (/)".into(),
            VolumeKind::Drive,
        );
    }

    for mount in read_proc_mounts() {
        let under_media = mount.starts_with("/media/")
            || mount.starts_with("/run/media/")
            || mount.starts_with("/mnt/");
        if !under_media {
            continue;
        }
        let kind = classify_mount(&mount);
        let name = mount
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| mount.display().to_string());
        push_volume(
            &mut volumes,
            &mut seen_devs,
            &mut seen_paths,
            mount,
            name,
            kind,
        );
    }

    volumes
}

#[cfg(not(windows))]
fn push_volume(
    volumes: &mut Vec<Volume>,
    seen_devs: &mut HashSet<u64>,
    seen_paths: &mut HashSet<PathBuf>,
    path: PathBuf,
    name: String,
    kind: VolumeKind,
) {
    if !path.is_dir() || !seen_paths.insert(path.clone()) {
        return;
    }
    if let Some(dev) = fs_dev(&path) {
        if !seen_devs.insert(dev) {
            return;
        }
    }
    let (free_bytes, total_bytes) = space_for(&path);
    let id = path.to_string_lossy().into_owned();
    volumes.push(Volume {
        id,
        name,
        path,
        kind,
        free_bytes,
        total_bytes,
    });
}

#[cfg(not(windows))]
fn read_proc_mounts() -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string("/proc/mounts") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let _source = parts.next();
        let Some(target) = parts.next() else {
            continue;
        };
        let path = PathBuf::from(unescape_mount(target));
        if path.is_dir() {
            out.push(path);
        }
    }
    out
}

/// `/proc/mounts` escapes spaces and specials as octal (`\040`).
#[cfg(not(windows))]
fn unescape_mount(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let oct = &s[i + 1..i + 4];
            if let Ok(v) = u8::from_str_radix(oct, 8) {
                out.push(v as char);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[cfg(not(windows))]
fn classify_mount(path: &Path) -> VolumeKind {
    let Ok(text) = std::fs::read_to_string("/proc/mounts") else {
        return VolumeKind::Device;
    };
    let needle = path.to_string_lossy();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let _source = parts.next();
        let Some(target) = parts.next() else {
            continue;
        };
        let Some(fstype) = parts.next() else {
            continue;
        };
        if unescape_mount(target) != needle.as_ref() {
            continue;
        }
        let ft = fstype.to_ascii_lowercase();
        if matches!(
            ft.as_str(),
            "nfs" | "nfs4" | "cifs" | "smb" | "smb3" | "smbfs" | "fuse.sshfs" | "fuse.davfs" | "9p"
        ) || ft.contains("nfs")
            || ft.contains("cifs")
            || ft.contains("smb")
        {
            return VolumeKind::Network;
        }
        if path.starts_with("/media") || path.starts_with("/run/media") {
            return VolumeKind::Device;
        }
        return VolumeKind::Drive;
    }
    if path.starts_with("/media") || path.starts_with("/run/media") {
        VolumeKind::Device
    } else {
        VolumeKind::Drive
    }
}

#[cfg(windows)]
fn discover_volumes_windows() -> Vec<Volume> {
    let mut volumes = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = PathBuf::from(format!("{}:\\", letter as char));
        if !root.exists() {
            continue;
        }
        let (free_bytes, total_bytes) = space_windows(&root).unwrap_or((0, 0));
        let kind = volume_kind_windows(&root);
        let name = match kind {
            VolumeKind::Network => format!("Network Drive ({}:)", letter as char),
            VolumeKind::Device => format!("Removable Disk ({}:)", letter as char),
            VolumeKind::Drive => format!("Local Disk ({}:)", letter as char),
        };
        let id = root.to_string_lossy().into_owned();
        volumes.push(Volume {
            id,
            name,
            path: root,
            kind,
            free_bytes,
            total_bytes,
        });
    }

    if volumes.is_empty() {
        if let Some(home) = dirs::home_dir() {
            let (free_bytes, total_bytes) =
                space_windows(&home).unwrap_or_else(|| space_for(&home));
            let id = home.to_string_lossy().into_owned();
            volumes.push(Volume {
                id,
                name: "Home".into(),
                path: home,
                kind: VolumeKind::Drive,
                free_bytes,
                total_bytes,
            });
        }
    }

    volumes
}

#[cfg(windows)]
fn space_windows(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(
            lp_directory_name: *const u16,
            lp_free_bytes_available_to_caller: *mut u64,
            lp_total_number_of_bytes: *mut u64,
            lp_total_number_of_free_bytes: *mut u64,
        ) -> i32;
    }

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free_avail: u64 = 0;
    let mut total: u64 = 0;
    let mut free_total: u64 = 0;
    let ok =
        unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free_avail, &mut total, &mut free_total) };
    if ok == 0 {
        return None;
    }
    Some((free_avail, total))
}

#[cfg(windows)]
fn volume_kind_windows(path: &Path) -> VolumeKind {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(lp_root_path_name: *const u16) -> u32;
    }

    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_CDROM: u32 = 5;

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ty = unsafe { GetDriveTypeW(wide.as_ptr()) };
    match ty {
        DRIVE_REMOTE => VolumeKind::Network,
        DRIVE_REMOVABLE | DRIVE_CDROM => VolumeKind::Device,
        _ => VolumeKind::Drive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pct_used_and_labels() {
        let v = Volume {
            id: "/".into(),
            name: "System".into(),
            path: PathBuf::from("/"),
            kind: VolumeKind::Drive,
            free_bytes: 199 * 1024 * 1024 * 1024,
            total_bytes: 618 * 1024 * 1024 * 1024,
        };
        assert_eq!(v.pct_used(), 67);
        assert_eq!(v.kind, VolumeKind::Drive);
        assert_eq!(v.free_label(), "199 GB free");
        assert_eq!(v.total_label(), "618 GB");
        assert_eq!(
            Volume {
                free_bytes: 0,
                total_bytes: 0,
                ..v.clone()
            }
            .pct_used(),
            0
        );
    }

    #[test]
    fn discover_returns_at_least_one_volume() {
        let volumes = discover_volumes();
        assert!(!volumes.is_empty(), "expected at least home or root volume");
        for v in &volumes {
            assert!(v.path.is_dir(), "{:?} should exist", v.path);
            assert!(!v.id.is_empty());
            assert!(!v.name.is_empty());
            assert!(v.free_bytes <= v.total_bytes || v.total_bytes == 0);
        }
    }

    #[test]
    fn quick_access_paths_are_dirs() {
        for p in default_quick_access() {
            assert!(p.is_dir(), "{p:?}");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn unescape_octal_space() {
        assert_eq!(unescape_mount(r"/mnt/My\040Disk"), "/mnt/My Disk");
    }
}
