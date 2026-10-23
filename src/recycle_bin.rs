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
        // Metadata is a shell query and may enumerate a directory, so keep it
        // inside the existing background listing task. If a recycled item has
        // disappeared or cannot be queried, omit it rather than inventing a
        // file with a size of zero.
        .filter_map(|item| {
            let metadata = trash::os_limited::metadata(&item).ok();
            let (kind, size) = metadata_to_entry_fields(metadata.map(|m| m.size))?;
            Some(item_to_entry(item, kind, size))
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

#[cfg(not(target_os = "macos"))]
fn metadata_to_entry_fields(size: Option<trash::TrashItemSize>) -> Option<(EntryKind, u64)> {
    match size? {
        trash::TrashItemSize::Bytes(bytes) => Some((EntryKind::File, bytes)),
        // Entry.size is a byte count, so the directory's child count is not
        // copied into it. The list UI already renders directories as "—".
        trash::TrashItemSize::Entries(_) => Some((EntryKind::Directory, 0)),
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

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn metadata_maps_file_and_directory_items() {
        assert_eq!(
            metadata_to_entry_fields(Some(trash::TrashItemSize::Bytes(1234))),
            Some((EntryKind::File, 1234))
        );
        assert_eq!(
            metadata_to_entry_fields(Some(trash::TrashItemSize::Entries(7))),
            Some((EntryKind::Directory, 0))
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unavailable_metadata_is_not_a_fake_empty_file() {
        assert_eq!(metadata_to_entry_fields(None), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn listing_is_unavailable_on_macos() {
        assert!(list().is_err());
    }
}
