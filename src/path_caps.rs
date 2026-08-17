//! Capability flags for a path: what Ply is willing to offer, not what the OS
//! might eventually allow. Portable-device paths currently support none of the
//! mutating or watchable operations.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub watch: bool,
    pub rename: bool,
    pub trash: bool,
    pub reveal: bool,
    /// When false, Open must copy the object out before handing it to the OS.
    pub open_direct: bool,
}

pub fn is_portable(path: &Path) -> bool {
    crate::mtp::is_mtp(path)
}

pub fn for_path(path: &Path) -> Caps {
    if is_portable(path) {
        Caps {
            watch: false,
            rename: false,
            trash: false,
            reveal: false,
            open_direct: false,
        }
    } else {
        Caps {
            watch: true,
            rename: true,
            trash: true,
            reveal: true,
            open_direct: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn local_paths_get_full_caps() {
        let caps = for_path(Path::new(r"C:\Users"));
        assert!(caps.watch && caps.rename && caps.trash && caps.reveal && caps.open_direct);
        assert!(!is_portable(Path::new(r"C:\Users")));
    }

    #[test]
    fn portable_paths_get_no_caps() {
        let path = PathBuf::from(r"\\MTP\DEVICE\o1");
        assert!(is_portable(&path));
        let caps = for_path(&path);
        assert!(!caps.watch && !caps.rename && !caps.trash && !caps.reveal && !caps.open_direct);
    }
}
