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
    if !crate::path_caps::for_path(path).reveal {
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
    if !crate::path_caps::for_path(path).rename {
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
    if paths.iter().any(|p| !crate::path_caps::for_path(p).trash) {
        bail!("A portable device has no Recycle Bin to move to.");
    }
    trash::delete_all(paths)?;
    Ok(())
}

fn refuse_mtp(path: &Path) -> Result<()> {
    if crate::mtp::is_mtp(path) {
        bail!("This is not available on a portable device.");
    }
    Ok(())
}

/// Extensions that promote Run as administrator on Windows.
pub fn is_admin_target(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            ["exe", "msi", "bat", "cmd", "ps1"]
                .iter()
                .any(|ext| e.eq_ignore_ascii_case(ext))
        })
}

/// Explorer-style ` (2)` suffix when `name` already exists in `dir`.
fn unique_in(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = path.extension().and_then(|s| s.to_str());
    for i in 2..10_000 {
        let candidate = match ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        if !dir.join(&candidate).exists() {
            return candidate;
        }
    }
    name.to_string()
}

pub fn create_folder(parent: &Path, name: &str) -> Result<PathBuf> {
    refuse_mtp(parent)?;
    let name = unique_in(parent, name);
    let target = parent.join(&name);
    std::fs::create_dir(&target)?;
    Ok(target)
}

pub fn open_terminal(dir: &Path) -> Result<()> {
    refuse_mtp(dir)?;
    let dir = if dir.is_file() {
        dir.parent().unwrap_or(dir)
    } else {
        dir
    };
    #[cfg(windows)]
    {
        if std::process::Command::new("wt")
            .args(["-d", &dir.display().to_string()])
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
        std::process::Command::new("cmd")
            .args(["/k", "cd", "/d", &dir.display().to_string()])
            .spawn()?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        bail!("Open in Terminal is Windows-only for now.");
    }
}

pub fn run_as_admin(path: &Path) -> Result<()> {
    refuse_mtp(path)?;
    #[cfg(windows)]
    {
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Start-Process -LiteralPath $env:PLY_RUNAS -Verb RunAs",
            ])
            .env("PLY_RUNAS", path.as_os_str())
            .spawn()?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        bail!("Run as administrator is Windows-only for now.");
    }
}

/// Native OS "Open with" picker.
pub fn choose_another(path: &Path) -> Result<()> {
    refuse_mtp(path)?;
    #[cfg(windows)]
    {
        std::process::Command::new("rundll32")
            .arg("shell32.dll,OpenAs_RunDLL")
            .arg(path)
            .spawn()?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        bail!("Choose another app is Windows-only for now.");
    }
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

    #[test]
    fn unique_in_adds_a_suffix() {
        let dir = tmp();
        std::fs::write(dir.join("New folder"), b"").unwrap();
        assert_eq!(unique_in(&dir, "New folder"), "New folder (2)");
        std::fs::remove_file(dir.join("New folder")).ok();
    }

    #[test]
    fn admin_target_is_exe_or_script() {
        assert!(is_admin_target(Path::new("setup.exe")));
        assert!(is_admin_target(Path::new("run.ps1")));
        assert!(!is_admin_target(Path::new("run.sh")));
        assert!(!is_admin_target(Path::new("notes.txt")));
    }
}
