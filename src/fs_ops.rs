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

/// A name that does not collide inside `dir`, using Explorer-style ` (2)` suffixes.
pub fn unique_in(dir: &Path, name: &str) -> String {
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

pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf> {
    refuse_mtp(parent)?;
    let name = unique_in(parent, name);
    let target = parent.join(&name);
    std::fs::write(&target, b"")?;
    Ok(target)
}

/// Copy entries into `dest`, returning the new paths. Never overwrites.
pub fn copy_entries(sources: &[PathBuf], dest: &Path) -> Result<Vec<PathBuf>> {
    refuse_mtp(dest)?;
    if sources.iter().any(|p| crate::mtp::is_mtp(p)) {
        bail!("Copy from a portable device is not supported.");
    }
    let mut out = Vec::new();
    for src in sources {
        let Some(name) = src.file_name() else {
            continue;
        };
        if src == dest || dest.starts_with(src) {
            bail!("Cannot copy a folder into itself.");
        }
        let target = dest.join(unique_in(dest, &name.to_string_lossy()));
        copy_recursive(src, &target)?;
        out.push(target);
    }
    Ok(out)
}

/// Move entries into `dest`. Same-folder cut is a no-op.
pub fn move_entries(sources: &[PathBuf], dest: &Path) -> Result<Vec<PathBuf>> {
    refuse_mtp(dest)?;
    if sources.iter().any(|p| crate::mtp::is_mtp(p)) {
        bail!("Move from a portable device is not supported.");
    }
    let mut out = Vec::new();
    for src in sources {
        let Some(name) = src.file_name() else {
            continue;
        };
        let Some(parent) = src.parent() else { continue };
        if parent == dest {
            out.push(src.clone());
            continue;
        }
        if dest.starts_with(src) {
            bail!("Cannot move a folder into itself.");
        }
        let target = dest.join(unique_in(dest, &name.to_string_lossy()));
        match std::fs::rename(src, &target) {
            Ok(()) => out.push(target),
            Err(_) => {
                copy_recursive(src, &target)?;
                trash::delete(src)?;
                out.push(target);
            }
        }
    }
    Ok(out)
}

fn copy_recursive(src: &Path, dst: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(src)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, dst)?;
        #[cfg(windows)]
        {
            if target.is_dir() {
                std::os::windows::fs::symlink_dir(target, dst)?;
            } else {
                std::os::windows::fs::symlink_file(target, dst)?;
            }
        }
        return Ok(());
    }
    if meta.is_dir() {
        std::fs::create_dir_all(dst)?;
        for ent in std::fs::read_dir(src)? {
            let ent = ent?;
            copy_recursive(&ent.path(), &dst.join(ent.file_name()))?;
        }
        return Ok(());
    }
    std::fs::copy(src, dst)?;
    Ok(())
}

fn refuse_mtp(path: &Path) -> Result<()> {
    if crate::mtp::is_mtp(path) {
        bail!("This write is not supported on a portable device.");
    }
    Ok(())
}

/// Open a terminal with `dir` as the working directory.
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
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["-a", "Terminal", &dir.display().to_string()])
            .spawn()?;
        return Ok(());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(term) = std::env::var("TERMINAL")
            && std::process::Command::new(&term)
                .current_dir(dir)
                .spawn()
                .is_ok()
        {
            return Ok(());
        }
        for (bin, extra) in [
            ("x-terminal-emulator", None),
            ("gnome-terminal", Some("--working-directory")),
            ("konsole", Some("--workdir")),
            ("xfce4-terminal", Some("--working-directory")),
            ("kitty", None),
            ("alacritty", None),
            ("xterm", None),
        ] {
            let mut cmd = std::process::Command::new(bin);
            cmd.current_dir(dir);
            if let Some(flag) = extra {
                cmd.arg(flag).arg(dir);
            }
            if cmd.spawn().is_ok() {
                return Ok(());
            }
        }
        bail!("No terminal emulator found.");
    }
}

/// Elevate and run. Windows uses `runas`; Unix tries `pkexec`.
pub fn run_as_admin(path: &Path) -> Result<()> {
    refuse_mtp(path)?;
    #[cfg(windows)]
    {
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("Start-Process -FilePath '{}' -Verb RunAs", path.display()),
            ])
            .spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "do shell script \"open -a '{}'\" with administrator privileges",
            path.display()
        );
        std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn()?;
        return Ok(());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if std::process::Command::new("pkexec")
            .arg(path)
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
        bail!("Could not elevate. pkexec is not available.");
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
    fn unique_in_adds_a_numeric_suffix() {
        let dir = tmp().join("uniq");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Note.txt"), b"a").unwrap();
        assert_eq!(unique_in(&dir, "Note.txt"), "Note (2).txt");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copy_entries_does_not_clobber() {
        let dir = tmp().join("copy");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.txt");
        std::fs::write(&src, b"hello").unwrap();
        let dest = dir.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        let copied = copy_entries(&[src.clone()], &dest).unwrap();
        assert_eq!(std::fs::read(&copied[0]).unwrap(), b"hello");
        assert!(src.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_folder_and_file() {
        let dir = tmp().join("new");
        std::fs::create_dir_all(&dir).unwrap();
        let folder = create_folder(&dir, "New folder").unwrap();
        assert!(folder.is_dir());
        let file = create_file(&dir, "New Text Document.txt").unwrap();
        assert!(file.is_file());
        std::fs::remove_dir_all(&dir).ok();
    }
}
