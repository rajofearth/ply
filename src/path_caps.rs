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
    if is_portable(path) || crate::recycle_bin::is_recycle_bin(path) {
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

    #[test]
    fn recycle_bin_path_gets_no_caps() {
        // Browse-only: the Recycle Bin must offer no mutating or openable caps.
        let path = PathBuf::from(crate::recycle_bin::ROOT_STR);
        assert!(crate::recycle_bin::is_recycle_bin(&path));
        let caps = for_path(&path);
        assert!(!caps.watch && !caps.rename && !caps.trash && !caps.reveal && !caps.open_direct);
    }

    const FULL: Caps = Caps {
        watch: true,
        rename: true,
        trash: true,
        reveal: true,
        open_direct: true,
    };
    const NONE: Caps = Caps {
        watch: false,
        rename: false,
        trash: false,
        reveal: false,
        open_direct: false,
    };

    #[test]
    fn capability_truth_table_covers_every_path_kind() {
        // Local paths are fully capable on every platform.
        for local in [r"C:\Users", r"C:\Users\me\notes.txt", "/home/me", "/home"] {
            let path = Path::new(local);
            assert_eq!(for_path(path), FULL, "local {local:?}");
            assert!(!is_portable(path), "local {local:?}");
        }

        // The synthetic Recycle Bin root parses identically on every platform.
        assert_eq!(
            for_path(Path::new(crate::recycle_bin::ROOT_STR)),
            NONE,
            "recycle bin"
        );

        // Portable-device paths only resolve where Windows UNC parsing exists.
        #[cfg(windows)]
        for mtp in [r"\\MTP\DEVICE", r"\\MTP\DEVICE\o1", r"\\MTP\dev\s1\o2"] {
            let path = Path::new(mtp);
            assert!(is_portable(path), "portable {mtp:?}");
            assert_eq!(for_path(path), NONE, "portable {mtp:?}");
        }
    }

    #[test]
    fn caps_are_granted_all_or_nothing() {
        // No path may land in a half-capable state: five capabilities, all or
        // none. Guards against a future match arm enabling one flag only.
        let mut paths = vec![
            PathBuf::from(r"C:\Users"),
            PathBuf::from("/home/me"),
            PathBuf::from(crate::recycle_bin::ROOT_STR),
        ];
        #[cfg(windows)]
        paths.push(PathBuf::from(r"\\MTP\DEVICE\o1"));
        for path in &paths {
            let c = for_path(path);
            let all = c.watch && c.rename && c.trash && c.reveal && c.open_direct;
            let none = !c.watch && !c.rename && !c.trash && !c.reveal && !c.open_direct;
            assert!(all || none, "half-capable caps for {path:?}: {c:?}");
        }
    }
}
