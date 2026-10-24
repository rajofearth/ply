//! Browsing the Recycle Bin. Read-only for now.
//!
//! The Recycle Bin is a virtual shell namespace with no filesystem path, so Ply
//! addresses it with a synthetic root path that `list_sorted` dispatches on,
//! mirroring how MTP devices travel through the rest of the app. Opening one
//! location shows a flat listing of everything in the bin; restore and empty
//! are out of scope.

use std::path::{Path, PathBuf};
#[cfg(not(target_os = "macos"))]
use std::time::{Duration, UNIX_EPOCH};

use crate::listing::Entry;
#[cfg(not(target_os = "macos"))]
use crate::listing::EntryKind;

#[cfg(windows)]
fn shell_entry_fields(item: &trash::TrashItem) -> Option<(EntryKind, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::EnhancedStorage::PKEY_Size;
    use windows::Win32::System::SystemServices::SFGAO_FOLDER;
    use windows::Win32::UI::Shell::{IShellItem, IShellItem2, SHCreateItemFromParsingName};
    use windows::core::{Interface, PCWSTR};

    let wide: Vec<u16> = item
        .id
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let shell: IShellItem =
        unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }.ok()?;
    if unsafe { shell.GetAttributes(SFGAO_FOLDER).ok()? } == SFGAO_FOLDER {
        return Some((EntryKind::Directory, 0));
    }
    let shell: IShellItem2 = shell.cast().ok()?;
    let size = unsafe { shell.GetUInt64(&PKEY_Size).ok()? };
    Some((EntryKind::File, size))
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn trash_entry_fields(item: &trash::TrashItem) -> (EntryKind, u64) {
    match trash::os_limited::metadata(item)
        .ok()
        .map(|metadata| metadata.size)
    {
        Some(trash::TrashItemSize::Bytes(bytes)) => (EntryKind::File, bytes),
        Some(trash::TrashItemSize::Entries(_)) => (EntryKind::Directory, 0),
        None => (EntryKind::Unknown, 0),
    }
}

#[cfg(windows)]
fn trash_entry_fields(item: &trash::TrashItem) -> (EntryKind, u64) {
    shell_entry_fields(item).unwrap_or((EntryKind::Unknown, 0))
}

/// The synthetic root that stands in for the Recycle Bin. Not a real path; it
/// is used as a lone `Location::Folder` so browsing works like any other folder.
pub const ROOT_STR: &str = r"\\RecycleBin";

pub fn root() -> PathBuf {
    PathBuf::from(ROOT_STR)
}

pub fn is_recycle_bin(path: &Path) -> bool {
    path == Path::new(ROOT_STR)
}

pub fn display_name() -> &'static str {
    "Recycle Bin"
}

/// Flat, browse-only listing of everything in the Recycle Bin. Ordering is left
/// to the caller's active sort, like any other folder.
#[cfg(not(target_os = "macos"))]
pub fn list() -> anyhow::Result<Vec<Entry>> {
    Ok(trash::os_limited::list()?
        .into_iter()
        .map(|item| {
            let (kind, size) = trash_entry_fields(&item);
            item_to_entry(item, kind, size)
        })
        .collect())
}

/// `trash` does not expose its `os_limited` listing API on macOS. The Recycle
/// Bin row is Windows-only, so this path is unreachable from the UI; fail
/// clearly if it is reached anyway.
#[cfg(target_os = "macos")]
pub fn list() -> anyhow::Result<Vec<Entry>> {
    anyhow::bail!("Recycle Bin browsing is not supported on macOS")
}

/// Each recycled item gets the shell parsing name as its path — the only stable
/// key the shell exposes, and not a filesystem path that no longer exists.
#[cfg(not(target_os = "macos"))]
fn item_to_entry(item: trash::TrashItem, kind: EntryKind, size: u64) -> Entry {
    let name = item.name.to_string_lossy().into_owned();
    let modified = (item.time_deleted >= 0)
        .then(|| UNIX_EPOCH + Duration::from_secs(item.time_deleted as u64));
    Entry {
        path: PathBuf::from(item.id.to_string_lossy().into_owned()),
        name,
        kind,
        size,
        modified,
        hidden: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_round_trips() {
        assert!(is_recycle_bin(&root()));
        assert!(!is_recycle_bin(Path::new(r"C:\Users")));
        assert!(!is_recycle_bin(Path::new("/home")));
        assert_eq!(display_name(), "Recycle Bin");
        assert!(root().file_name().is_some());
    }

    #[test]
    fn child_of_the_root_is_not_the_root() {
        // A path whose parent is the bin root names an item, not the bin itself;
        // only the exact root is treated as the Recycle Bin location.
        assert!(!is_recycle_bin(&root().join("someitem")));
    }

    #[test]
    fn detection_is_exact_and_case_sensitive() {
        assert!(is_recycle_bin(Path::new(ROOT_STR)));
        assert!(is_recycle_bin(&root()));
        // Lookalikes and same-named real paths are ordinary locations.
        for other in [
            r"\\RecycleBinX",
            r"\\RecycleBins",
            r"C:\RecycleBin",
            r"C:\Users\RecycleBin",
            r"\\recyclebin",
        ] {
            assert!(!is_recycle_bin(Path::new(other)), "{other:?}");
        }
    }

    #[test]
    fn root_constant_is_stable() {
        assert_eq!(ROOT_STR, r"\\RecycleBin");
        assert_eq!(root(), PathBuf::from(ROOT_STR));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn listing_is_unavailable_on_macos() {
        assert!(list().is_err());
    }
}
