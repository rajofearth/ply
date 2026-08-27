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
    for p in paths {
        refuse_volume_root(p)?;
    }
    if paths.iter().any(|p| !crate::path_caps::for_path(p).trash) {
        bail!("A portable device has no Recycle Bin to move to.");
    }
    trash::delete_all(paths)?;
    Ok(())
}

/// Whether a path sits on a volume that can hold a Recycle Bin. Removable,
/// CD, network and portable-device volumes cannot, so delete there needs a
/// permanent-delete step; fixed drives recycle normally. Mirrors how Windows
/// Explorer decides when to ask for permanent deletion.
pub fn volume_supports_recycle_bin(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetDriveTypeW(root: *const u16) -> u32;
        }
        const DRIVE_FIXED: u32 = 3;

        let wide: Vec<u16> = volume_root(path)
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        // SAFETY: NUL-terminated drive root path.
        unsafe { GetDriveTypeW(wide.as_ptr()) == DRIVE_FIXED }
    }
    #[cfg(not(windows))]
    {
        // No cross-platform recycle bins on Unix either, but deletion there has
        // always gone to trash when available; keep that behaviour fallible.
        true
    }
}

/// The drive root to ask Windows about: `C:\` for a lettered path, or the
/// `\\server\share` root for a UNC path. Anything unmappable falls back to a
/// fixed-looking local root so it is treated as recyclable rather than
/// destructively permanent.
#[cfg(windows)]
fn volume_root(path: &Path) -> PathBuf {
    let s = path.to_string_lossy().into_owned();
    if s.len() >= 3 && s.as_bytes()[1] == b':' {
        // "C:\..." -> "C:\"
        return PathBuf::from(format!("{}:\\", &s[..1]));
    }
    if let Some(rest) = s.strip_prefix("\\\\") {
        let parts: Vec<&str> = rest.split('\\').filter(|x| !x.is_empty()).collect();
        if parts.len() >= 2 {
            return PathBuf::from(format!("\\\\{}\\{}", parts[0], parts[1]));
        }
    }
    PathBuf::from("C:\\")
}

/// Permanently delete entries without any Recycle Bin, recursing into folders.
///
/// Every path is screened for drive/device roots and MTP before any mutation,
/// so an unhealthy batch fails closed with nothing touched — never a partial
/// permanent delete.
pub fn delete_permanently(paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        refuse_volume_root(path)?;
        refuse_mtp(path)?;
    }
    for path in paths {
        // `symlink_metadata` so a symlink is removed, not its target.
        let meta = std::fs::symlink_metadata(path)?;
        if meta.file_type().is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn refuse_mtp(path: &Path) -> Result<()> {
    if crate::mtp::is_mtp(path) {
        bail!("This is not available on a portable device.");
    }
    Ok(())
}

/// Refuse to mutate a drive/device root: `path.parent().is_none()` is the
/// documented std predicate for "terminates in a root or prefix", covering
/// `D:\`, `\\server\share`, `\\MTP\dev` and non-normalized junk. The recycle
/// bin's synthetic root (`\\RecycleBin`) parses with a `\` parent on Windows,
/// so it is refused explicitly too.
fn refuse_volume_root(path: &Path) -> Result<()> {
    if path.parent().is_none() || crate::recycle_bin::is_recycle_bin(path) {
        bail!("A drive or device root cannot be deleted.");
    }
    Ok(())
}

/// How a single path should be deleted, fail-closed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeleteFlow {
    Refuse,
    Trash,
    Permanent,
}

pub fn classify_delete(path: &Path) -> DeleteFlow {
    if path.parent().is_none() || crate::recycle_bin::is_recycle_bin(path) {
        return DeleteFlow::Refuse; // drive/UNC/MTP/recycle-bin root or prefix
    }
    if crate::path_caps::is_portable(path) {
        return DeleteFlow::Refuse; // MTP objects are virtual, never deleted
    }
    if volume_supports_recycle_bin(path) {
        DeleteFlow::Trash
    } else {
        DeleteFlow::Permanent
    }
}

/// Split paths into trashable and permanent-delete sets, or refuse the whole
/// batch if any is a drive/device root. Never escalates a trashable path.
pub fn plan_delete(paths: &[PathBuf]) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let (mut trash, mut permanent) = (Vec::new(), Vec::new());
    for p in paths {
        match classify_delete(p) {
            DeleteFlow::Refuse => bail!("Deleting a drive or device root isn't supported."),
            DeleteFlow::Trash => trash.push(p.clone()),
            DeleteFlow::Permanent => permanent.push(p.clone()),
        }
    }
    Ok((trash, permanent))
}

/// Extensions that promote Run as administrator on Windows.
pub fn is_admin_target(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
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

    #[test]
    fn delete_permanently_removes_files_and_folders() {
        let dir = tmp().join("delperm");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file.txt"), b"x").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/nested.txt"), b"y").unwrap();

        delete_permanently(&[dir.join("file.txt"), dir.join("sub")]).unwrap();
        assert!(!dir.join("file.txt").exists());
        assert!(!dir.join("sub").exists());
        assert!(dir.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn volume_root_maps_lettered_and_unc_paths() {
        assert_eq!(volume_root(Path::new(r"C:\foo\bar")), PathBuf::from(r"C:\"));
        assert_eq!(volume_root(Path::new(r"D:\")), PathBuf::from(r"D:\"));
        assert_eq!(
            volume_root(Path::new(r"\\server\share\file.txt")),
            PathBuf::from(r"\\server\share")
        );
    }

    #[test]
    fn classify_delete_refuses_all_roots() {
        assert_eq!(
            classify_delete(Path::new(crate::recycle_bin::ROOT_STR)),
            DeleteFlow::Refuse
        );
        assert_ne!(
            classify_delete(Path::new(r"D:\Users\me\notes.txt")),
            DeleteFlow::Refuse
        );
        #[cfg(windows)]
        {
            assert_eq!(classify_delete(Path::new(r"D:\")), DeleteFlow::Refuse);
            assert_eq!(classify_delete(Path::new(r"C:\")), DeleteFlow::Refuse);
            assert_eq!(
                classify_delete(Path::new(r"\\server\share")),
                DeleteFlow::Refuse
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn permanent_delete_short_circuits_on_drive_root_in_batch() {
        let dir = tmp().join("delpermroot");
        std::fs::create_dir_all(&dir).unwrap();
        let sentinel = dir.join("sentinel.txt");
        std::fs::write(&sentinel, b"x").unwrap();

        let res = delete_permanently(&[PathBuf::from(r"D:\"), sentinel.clone()]);
        assert!(res.is_err());
        assert!(sentinel.exists(), "sentinel must survive a refused batch");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_delete_partitions_without_escalating() {
        let (a, b) = (
            PathBuf::from(r"C:\Users\me\a.txt"),
            PathBuf::from(r"C:\Users\me\b.txt"),
        );
        let (trash, permanent) = plan_delete(&[a, b]).unwrap();
        assert!(trash.is_empty() || permanent.is_empty());
        assert_eq!(trash.len() + permanent.len(), 2);
    }

    #[test]
    fn classify_delete_refuses_every_root_form() {
        let roots: &[&str] = &[
            // Cross-platform roots: the Recycle Bin's synthetic root and an MTP
            // path both parse identically on any OS.
            crate::recycle_bin::ROOT_STR,
            r"\\MTP\DEVICE\o1",
        ];
        for root in roots {
            assert_eq!(
                classify_delete(Path::new(root)),
                DeleteFlow::Refuse,
                "root {root:?} must be refused"
            );
        }
        // Drive-letter and UNC roots only make sense on Windows.
        #[cfg(windows)]
        {
            for root in [r"D:\", r"C:\", r"\\server\share"] {
                assert_eq!(
                    classify_delete(Path::new(root)),
                    DeleteFlow::Refuse,
                    "root {root:?} must be refused"
                );
            }
        }
    }

    #[test]
    fn classify_delete_mtp_sub_object_is_refused() {
        assert_eq!(
            classify_delete(Path::new(r"\\MTP\DEVICE\o1")),
            DeleteFlow::Refuse
        );
    }

    #[test]
    fn classify_delete_leaf_is_trash_or_permanent() {
        let dir = tmp().join("classify-leaf");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("leaf.txt");
        std::fs::write(&file, b"x").unwrap();

        for leaf in [&file, &dir] {
            let flow = classify_delete(leaf);
            assert!(
                flow == DeleteFlow::Trash || flow == DeleteFlow::Permanent,
                "a real leaf must be Trash or Permanent, got {flow:?}"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_delete_pure_trashable_batch_is_wholly_in_trash() {
        let dir = tmp().join("plan-trash");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"x").unwrap();

        let paths: Vec<PathBuf> = [file.clone(), dir.clone()]
            .into_iter()
            .filter(|p| classify_delete(p) == DeleteFlow::Trash)
            .collect();
        // If the machine has no fixed drive the batch is empty; nothing to plan.
        if !paths.is_empty() {
            let (trash, permanent) = plan_delete(&paths).unwrap();
            assert_eq!(trash.len(), paths.len(), "every trashable path lands in trash");
            assert!(permanent.is_empty(), "no trashable path may escalate");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_delete_mixed_batch_with_portable_root_is_refused_entirely() {
        let dir = tmp().join("plan-mixed");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("keep.txt");
        std::fs::write(&file, b"x").unwrap();

        let res = plan_delete(&[file.clone(), PathBuf::from(r"\\MTP\DEVICE\o1")]);
        assert!(res.is_err(), "a batch touching a portable root must be refused whole");
        assert!(file.exists(), "nothing may be planned/deleted when a batch is refused");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_delete_never_drops_an_entry() {
        let dir = tmp().join("plan-nodrop");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"x").unwrap();

        let (trash, permanent) = plan_delete(&[file.clone(), dir.clone()]).unwrap();
        assert_eq!(
            trash.len() + permanent.len(),
            2,
            "every planned entry lands in exactly one bucket"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_permanently_empty_batch_is_a_noop() {
        assert!(delete_permanently(&[]).is_ok());
    }

    #[test]
    fn delete_to_trash_empty_batch_is_a_noop() {
        assert!(delete_to_trash(&[]).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn delete_permanently_refuses_a_lone_drive_root() {
        let dir = tmp().join("delperm-loneroot");
        std::fs::create_dir_all(&dir).unwrap();
        let sentinel = dir.join("sentinel.txt");
        std::fs::write(&sentinel, b"x").unwrap();

        let res = delete_permanently(&[PathBuf::from(r"D:\")]);
        assert!(res.is_err());
        assert!(sentinel.exists(), "a refused root must not touch anything else");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn delete_permanently_batch_with_root_first_never_wipes_or_deletes() {
        // Regression for the original wipe bug: a removable volume root leading
        // a batch must abort before the root or any following entry is touched.
        let dir = tmp().join("delperm-batchroot");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("keep.txt");
        std::fs::write(&file, b"x").unwrap();

        let res = delete_permanently(&[PathBuf::from(r"D:\"), file.clone()]);
        assert!(res.is_err());
        assert!(file.exists(), "the root guard must abort before any deletion");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn delete_permanently_batch_with_root_after_file_aborts_without_touching_any() {
        // Fail-closed on any unhealthy batch, regardless of root position: the
        // pre-scan refuses before a single entry is mutated, so a trailing root
        // must abort the whole batch with the leading file untouched.
        let dir = tmp().join("delperm-batchroot2");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("keep.txt");
        std::fs::write(&file, b"x").unwrap();

        let res = delete_permanently(&[file.clone(), PathBuf::from(r"D:\")]);
        assert!(res.is_err(), "a root trailing the batch must refuse the whole batch");
        assert!(file.exists(), "nothing may be deleted when any batch member is a root");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn delete_to_trash_refuses_a_drive_root() {
        // Drive roots are never recyclable: the guard must balk instead of
        // handing a root to the trash layer.
        let dir = tmp().join("deltrash-root");
        std::fs::create_dir_all(&dir).unwrap();
        let sentinel = dir.join("sentinel.txt");
        std::fs::write(&sentinel, b"x").unwrap();

        let res = delete_to_trash(&[PathBuf::from(r"D:\"), sentinel.clone()]);
        assert!(res.is_err());
        assert!(sentinel.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_permanently_removes_symlink_without_touching_target() {
        let dir = tmp().join("delperm-symlink");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target.txt");
        std::fs::write(&target, b"keep").unwrap();
        let link = dir.join("link.txt");

        let created = {
            #[cfg(windows)]
            {
                std::fs::remove_file(&link).ok();
                std::os::windows::fs::symlink_file(&target, &link).is_ok()
            }
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&target, &link).is_ok()
            }
        };
        if !created {
            // Symlink creation needs Developer Mode / privileges it can be
            // denied; skip rather than fail the whole run on such machines.
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        assert!(link.is_symlink());

        delete_permanently(&[link.clone()]).unwrap();
        assert!(!link.exists(), "the symlink itself must be removed");
        assert!(target.exists(), "the symlink target must survive deletion");
        std::fs::remove_dir_all(&dir).ok();
    }
}
