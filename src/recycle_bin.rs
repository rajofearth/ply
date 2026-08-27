//! Browsing the Recycle Bin. Read-only for now.
//!
//! The Recycle Bin is a virtual shell namespace with no filesystem path, so Ply
//! addresses it with a synthetic root path that `list_sorted` dispatches on,
//! mirroring how MTP devices travel through the rest of the app. Opening one
//! location shows a flat listing of everything in the bin; restore and empty
//! are out of scope.

use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use crate::listing::{Entry, EntryKind};

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
pub fn list() -> anyhow::Result<Vec<Entry>> {
    Ok(trash::os_limited::list()?
        .into_iter()
        .map(item_to_entry)
        .collect())
}

/// Each recycled item gets the shell parsing name as its path — the only stable
/// key the shell exposes, and not a filesystem path that no longer exists.
fn item_to_entry(item: trash::TrashItem) -> Entry {
    let name = item.name.to_string_lossy().into_owned();
    let modified = (item.time_deleted >= 0)
        .then(|| UNIX_EPOCH + Duration::from_secs(item.time_deleted as u64));
    Entry {
        path: PathBuf::from(item.id.to_string_lossy().into_owned()),
        name,
        kind: EntryKind::File,
        size: 0,
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
}
