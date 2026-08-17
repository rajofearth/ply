//! Filesystem side effects. Everything here touches the disk or the shell, so
//! each call is fallible and reports a message the status line can show.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

/// Hand a file to the OS default application.
pub fn open_with_os(path: &Path) -> Result<()> {
    open::that_detached(path)?;
    Ok(())
}

/// Show the entry selected in the platform's own file manager.
pub fn reveal(path: &Path) -> Result<()> {
    if crate::mtp::is_mtp(path) {
        bail!("A portable device has no folder to reveal.");
    }
    #[cfg(windows)]
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn()?;
    #[cfg(not(windows))]
    open::that_detached(path.parent().unwrap_or(path))?;
    Ok(())
}

/// Rename in place, returning the new path.
///
/// `new_name` is a bare file name: anything path-like is rejected rather than
/// silently moving the entry somewhere else.
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf> {
    if crate::mtp::is_mtp(path) {
        bail!("Renaming on a portable device is not supported.");
    }
    let new_name = new_name.trim();
    if new_name.is_empty() {
        bail!("Name cannot be empty.");
    }
    if new_name.contains(['/', '\\']) || Path::new(new_name).components().count() != 1 {
        bail!("Name cannot contain a path separator.");
    }
    if new_name.chars().any(|ch| "<>:\"|?*".contains(ch)) {
        bail!("Name cannot contain < > : \" | ? *");
    }
    let Some(parent) = path.parent() else {
        bail!("Cannot rename a drive root.");
    };
    let target = parent.join(new_name);
    if target == path {
        return Ok(target);
    }
    if target.exists() {
        bail!("\"{new_name}\" already exists here.");
    }
    std::fs::rename(path, &target)?;
    Ok(target)
}

/// Move entries to the platform recycle bin. Never deletes permanently.
pub fn delete_to_trash(paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    if paths.iter().any(|p| crate::mtp::is_mtp(p)) {
        bail!("A portable device has no Recycle Bin to move items to.");
    }
    trash::delete_all(paths)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ply-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rename_rejects_path_separators() {
        let file = tmp().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(rename(&file, "sub/b.txt").is_err());
        assert!(rename(&file, "").is_err());
        assert!(rename(&file, "b?.txt").is_err());
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn rename_moves_the_file() {
        let dir = tmp();
        let file = dir.join("before.txt");
        std::fs::write(&file, b"x").unwrap();
        let after = rename(&file, "after.txt").unwrap();
        assert_eq!(after, dir.join("after.txt"));
        assert!(after.exists() && !file.exists());
        std::fs::remove_file(&after).ok();
    }

    #[test]
    fn rename_refuses_to_clobber() {
        let dir = tmp();
        let (a, b) = (dir.join("one.txt"), dir.join("two.txt"));
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"y").unwrap();
        assert!(rename(&a, "two.txt").is_err());
        assert_eq!(std::fs::read(&b).unwrap(), b"y");
        std::fs::remove_file(&a).ok();
        std::fs::remove_file(&b).ok();
    }
}
