//! PathCaps: omit the impossible, grey out the temporarily unavailable.

use std::path::Path;

use crate::mtp;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cap {
    /// Show and enable.
    Yes,
    /// Show disabled (temporarily unavailable).
    Soft,
    /// Omit from the menu.
    No,
}

impl Cap {
    pub fn show(self) -> bool {
        !matches!(self, Cap::No)
    }

    pub fn enable(self) -> bool {
        matches!(self, Cap::Yes)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PathCaps {
    pub open: Cap,
    pub open_with: Cap,
    pub run_as_admin: Cap,
    pub terminal: Cap,
    pub new_tab: Cap,
    pub new_window: Cap,
    pub rename: Cap,
    pub cut: Cap,
    pub copy: Cap,
    pub paste: Cap,
    pub copy_path: Cap,
    pub delete: Cap,
    pub pin: Cap,
    pub unpin: Cap,
    pub properties: Cap,
    pub new_item: Cap,
    pub refresh: Cap,
    pub view: Cap,
    pub sort: Cap,
}

#[derive(Clone, Copy, Debug)]
pub struct CapsCtx<'a> {
    pub clipboard_empty: bool,
    pub is_volume: bool,
    pub pinned: bool,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_multi: bool,
    pub run_as_admin: bool,
    pub folder: Option<&'a Path>,
}

impl PathCaps {
    pub fn for_entry(path: &Path, ctx: CapsCtx<'_>) -> Self {
        let mtp = mtp::is_mtp(path);
        let volume = ctx.is_volume;
        let writable = !mtp && !volume;
        let paste = paste_cap(ctx.clipboard_empty, writable && ctx.folder.is_some());
        let folderish = ctx.is_dir && !ctx.is_multi;

        Self {
            open: Cap::Yes,
            open_with: if ctx.is_file && !ctx.is_multi && !mtp {
                Cap::Yes
            } else {
                Cap::No
            },
            run_as_admin: if ctx.run_as_admin && !ctx.is_multi && !mtp {
                Cap::Yes
            } else {
                Cap::No
            },
            terminal: if folderish && !mtp { Cap::Yes } else { Cap::No },
            new_tab: if folderish { Cap::Yes } else { Cap::No },
            new_window: if folderish { Cap::Yes } else { Cap::No },
            rename: if writable && !ctx.is_multi {
                Cap::Yes
            } else {
                Cap::No
            },
            cut: if writable { Cap::Yes } else { Cap::No },
            copy: if !mtp { Cap::Yes } else { Cap::No },
            paste,
            copy_path: Cap::Yes,
            delete: if writable { Cap::Yes } else { Cap::No },
            pin: if folderish && !ctx.pinned {
                Cap::Yes
            } else {
                Cap::No
            },
            unpin: if folderish && ctx.pinned {
                Cap::Yes
            } else {
                Cap::No
            },
            properties: Cap::Yes,
            new_item: Cap::No,
            refresh: Cap::No,
            view: Cap::No,
            sort: Cap::No,
        }
    }

    pub fn for_background(folder: &Path, ctx: CapsCtx<'_>) -> Self {
        let mtp = mtp::is_mtp(folder);
        let writable = !mtp && !ctx.is_volume;
        let paste = paste_cap(ctx.clipboard_empty, writable);
        Self {
            open: Cap::No,
            open_with: Cap::No,
            run_as_admin: Cap::No,
            terminal: if mtp { Cap::No } else { Cap::Yes },
            new_tab: Cap::No,
            new_window: Cap::No,
            rename: Cap::No,
            cut: Cap::No,
            copy: Cap::No,
            paste,
            copy_path: Cap::No,
            delete: Cap::No,
            pin: Cap::No,
            unpin: Cap::No,
            properties: Cap::Yes,
            new_item: if writable { Cap::Yes } else { Cap::No },
            refresh: Cap::Yes,
            view: Cap::Yes,
            sort: Cap::Yes,
        }
    }
}

fn paste_cap(clipboard_empty: bool, writable: bool) -> Cap {
    if !writable {
        Cap::No
    } else if clipboard_empty {
        Cap::Soft
    } else {
        Cap::Yes
    }
}

/// Extensions that promote "Run as administrator" (or pkexec on Unix).
pub fn is_admin_target(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "exe" | "msi" | "bat" | "cmd" | "ps1" | "sh" | "bash" | "appimage"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx<'a>(folder: Option<&'a Path>) -> CapsCtx<'a> {
        CapsCtx {
            clipboard_empty: true,
            is_volume: false,
            pinned: false,
            is_dir: false,
            is_file: true,
            is_multi: false,
            run_as_admin: false,
            folder,
        }
    }

    #[test]
    fn empty_paste_is_soft_on_a_writable_folder() {
        let folder = PathBuf::from("/tmp");
        let caps = PathCaps::for_background(&folder, ctx(Some(&folder)));
        assert_eq!(caps.paste, Cap::Soft);
        assert!(caps.paste.show());
        assert!(!caps.paste.enable());
        assert_eq!(caps.new_item, Cap::Yes);
    }

    #[test]
    fn paste_enables_when_clipboard_has_files() {
        let folder = PathBuf::from("/tmp");
        let mut c = ctx(Some(&folder));
        c.clipboard_empty = false;
        let caps = PathCaps::for_background(&folder, c);
        assert_eq!(caps.paste, Cap::Yes);
    }

    #[test]
    fn volume_root_omits_writes() {
        let path = PathBuf::from("C:\\");
        let mut c = ctx(Some(&path));
        c.is_volume = true;
        c.is_dir = true;
        c.is_file = false;
        let caps = PathCaps::for_entry(&path, c);
        assert_eq!(caps.rename, Cap::No);
        assert_eq!(caps.delete, Cap::No);
        assert_eq!(caps.cut, Cap::No);
    }

    #[cfg(windows)]
    #[test]
    fn mtp_omits_writes_and_open_with() {
        let path = PathBuf::from(r"\\MTP\deadbeef\obj");
        let mut c = ctx(Some(&path));
        c.is_file = true;
        let caps = PathCaps::for_entry(&path, c);
        assert_eq!(caps.rename, Cap::No);
        assert_eq!(caps.delete, Cap::No);
        assert_eq!(caps.cut, Cap::No);
        assert_eq!(caps.copy, Cap::No);
        assert_eq!(caps.open_with, Cap::No);
        assert_eq!(caps.open, Cap::Yes);
    }

    #[test]
    fn multi_select_omits_rename_and_promote() {
        let path = PathBuf::from("/tmp/a.txt");
        let mut c = ctx(Some(Path::new("/tmp")));
        c.is_multi = true;
        c.is_file = true;
        let caps = PathCaps::for_entry(&path, c);
        assert_eq!(caps.rename, Cap::No);
        assert_eq!(caps.open_with, Cap::No);
        assert_eq!(caps.run_as_admin, Cap::No);
        assert_eq!(caps.cut, Cap::Yes);
        assert_eq!(caps.delete, Cap::Yes);
    }

    #[test]
    fn folder_exposes_terminal_and_pin() {
        let path = PathBuf::from("/tmp/docs");
        let mut c = ctx(Some(Path::new("/tmp")));
        c.is_dir = true;
        c.is_file = false;
        let caps = PathCaps::for_entry(&path, c);
        assert_eq!(caps.terminal, Cap::Yes);
        assert_eq!(caps.new_tab, Cap::Yes);
        assert_eq!(caps.pin, Cap::Yes);
        assert_eq!(caps.open_with, Cap::No);
    }

    #[test]
    fn exe_promotes_run_as_admin() {
        assert!(is_admin_target(Path::new("setup.exe")));
        assert!(is_admin_target(Path::new("run.sh")));
        assert!(!is_admin_target(Path::new("notes.txt")));
    }
}
