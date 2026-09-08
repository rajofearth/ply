//! Filesystem side effects. Everything here touches the disk or the shell, so
//! each call is fallible and reports a message the status line can show.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Recommended Open-with handlers for a file, as display names for the
/// flyout. Sync registry reads via `SHAssocEnumHandlers` + `IAssocHandler`;
/// capped at [`OPEN_WITH_CAP`] and never blocking menu open: any failure
/// (including MTP) returns empty so the caller falls back to the single
/// Choose-app child. `Invoke` is deferred, so clicking a row currently opens
/// the picker.
pub const OPEN_WITH_CAP: usize = 6;

#[cfg(windows)]
fn assoc_handler_display_name(
    handler: &windows::Win32::UI::Shell::IAssocHandler,
) -> Option<String> {
    use windows::Win32::System::Com::CoTaskMemFree;

    let pwstr = unsafe { handler.GetUIName().or_else(|_| handler.GetName()).ok()? };
    let raw = pwstr.as_ptr();
    if raw.is_null() {
        return None;
    }
    let len = unsafe {
        let mut n = 0usize;
        while *raw.add(n) != 0 {
            n += 1;
            if n > 4096 {
                break;
            }
        }
        n
    };
    let slice = unsafe { std::slice::from_raw_parts(raw, len) };
    let s = String::from_utf16_lossy(slice).trim().to_string();
    unsafe {
        CoTaskMemFree(Some(raw as *const std::ffi::c_void));
    }
    if s.is_empty() { None } else { Some(s) }
}

/// Recommended Open-with app: the handler display name plus its shell icon
/// source. `icon` is the handler's `GetIconLocation` path when it resolves
/// to a real file on disk, else empty.
///
/// The app renders `icon` via the existing `path_icon_probe` path
/// (`thumbs.rs:1201`), which resolves a real file's own shell icon through
/// `path_icon` (`thumbs.rs:2396`) and `path_icon_index` (`thumbs.rs:1999`);
/// no worker change is needed. Note that probe takes no dll icon index
/// (`GetIconLocation` also returns one); the exe/dll path is returned anyway
/// because the file's own icon is the right art in nearly all cases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenWithApp {
    pub name: String,
    pub icon: PathBuf,
}

/// Recommended Open-with apps for a file, as display names plus icon sources
/// for the flyout. Same enumeration as [`list_open_with_handlers`] (capped
/// at [`OPEN_WITH_CAP`], MTP/Recycle Bin refused, empty on any failure), but
/// per handler also reads `IAssocHandler::GetIconLocation`; the icon path is
/// kept only when it resolves to a real file on disk, else empty.
pub fn list_open_with_apps(path: &Path) -> Vec<OpenWithApp> {
    if crate::mtp::is_mtp(path) || crate::recycle_bin::is_recycle_bin(path) {
        return Vec::new();
    }
    #[cfg(windows)]
    {
        list_open_with_apps_windows(path).unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Vec::new()
    }
}

#[cfg(windows)]
fn list_open_with_apps_windows(path: &Path) -> Option<Vec<OpenWithApp>> {
    use windows::Win32::UI::Shell::{ASSOC_FILTER_RECOMMENDED, IAssocHandler, SHAssocEnumHandlers};
    use windows::core::PCWSTR;

    let ext = path.extension()?.to_str()?;
    if ext.is_empty() {
        return Some(Vec::new());
    }
    let assoc = format!(".{ext}");
    let wide: Vec<u16> = assoc.encode_utf16().chain(Some(0)).collect();
    let enum_handlers =
        unsafe { SHAssocEnumHandlers(PCWSTR(wide.as_ptr()), ASSOC_FILTER_RECOMMENDED).ok()? };
    let mut out: Vec<OpenWithApp> = Vec::new();
    for _ in 0..OPEN_WITH_CAP {
        let mut fetched = 0u32;
        let mut handler: [Option<IAssocHandler>; 1] = [None];
        if unsafe {
            enum_handlers
                .Next(&mut handler, Some(&mut fetched))
                .is_err()
        } || fetched == 0
        {
            break;
        }
        let Some(h) = handler[0].take() else {
            break;
        };
        let name = assoc_handler_display_name(&h).unwrap_or_default();
        let name = name.trim().to_string();
        if name.is_empty() || out.iter().any(|a| a.name == name) {
            continue;
        }
        let icon = assoc_handler_icon_path(&h).unwrap_or_default();
        out.push(OpenWithApp { name, icon });
        if out.len() >= OPEN_WITH_CAP {
            break;
        }
    }
    Some(out)
}

/// The handler's `GetIconLocation` path when it is absolute and a real file
/// on disk; `None` when unresolvable, non-absolute, or not a file. The icon
/// index out-param is intentionally dropped (see [`OpenWithApp`]).
#[cfg(windows)]
fn assoc_handler_icon_path(handler: &windows::Win32::UI::Shell::IAssocHandler) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::core::PWSTR;

    let mut icon_ptr = PWSTR::null();
    let mut _index: i32 = 0;
    unsafe {
        handler
            .GetIconLocation(&mut icon_ptr as *mut PWSTR, &mut _index as *mut i32)
            .ok()?
    };
    let raw = icon_ptr.as_ptr();
    if raw.is_null() {
        return None;
    }
    let len = unsafe {
        let mut n = 0usize;
        while *raw.add(n) != 0 {
            n += 1;
            if n > 4096 {
                break;
            }
        }
        n
    };
    let slice = unsafe { std::slice::from_raw_parts(raw, len) };
    let s = String::from_utf16_lossy(slice);
    unsafe {
        CoTaskMemFree(Some(raw as *const std::ffi::c_void));
    }
    let s = expand_env_vars(s.trim().trim_matches(['"', '\'']).trim());
    if s.is_empty() {
        return None;
    }
    let icon = PathBuf::from(&s);
    if !icon.is_absolute() || !icon.is_file() {
        return None;
    }
    Some(icon)
}

/// Expand `%VAR%` segments via the process environment, without extra Win32
/// imports. Unknown variables expand to empty, matching `ExpandEnvironmentStringsW`.
#[cfg(windows)]
fn expand_env_vars(raw: &str) -> String {
    if !raw.contains('%') {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let key = &after[..end];
                if let Ok(val) = std::env::var(key) {
                    out.push_str(&val);
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('%');
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Friendly app name for the Opens-with row via `AssocQueryString`
/// (`ASSOCSTR_FRIENDLYAPPNAME`), or `None` when the shell has nothing.
/// MTP and bin paths always return `None` so callers fall back to the kind
/// label. Sync registry read, microsecond rank.
pub fn friendly_app_name(path: &Path) -> Option<String> {
    if crate::mtp::is_mtp(path) || crate::recycle_bin::is_recycle_bin(path) {
        return None;
    }
    #[cfg(windows)]
    {
        friendly_app_name_windows(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

#[cfg(windows)]
fn friendly_app_name_windows(path: &Path) -> Option<String> {
    use windows::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_FRIENDLYAPPNAME, AssocQueryStringW};
    use windows::core::PCWSTR;

    if path.is_dir() {
        return None;
    }
    let ext = path.extension()?.to_str()?;
    if ext.is_empty() {
        return None;
    }
    let assoc = format!(".{ext}");
    let wide_assoc: Vec<u16> = assoc.encode_utf16().chain(Some(0)).collect();
    let mut out = vec![0u16; 1024];
    let mut len = out.len() as u32;
    let hr = unsafe {
        AssocQueryStringW(
            ASSOCF_NONE,
            ASSOCSTR_FRIENDLYAPPNAME,
            PCWSTR(wide_assoc.as_ptr()),
            PCWSTR::null(),
            Some(windows::core::PWSTR(out.as_mut_ptr())),
            &mut len,
        )
    };
    if hr.is_err() {
        return None;
    }
    let end = out.iter().position(|&c| c == 0).unwrap_or(out.len());
    if end == 0 {
        return None;
    }
    let s = String::from_utf16_lossy(&out[..end]).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Executable whose shell icon represents the terminal row. `open_terminal`
/// prefers `wt` then falls back to `cmd`; the icon uses the `cmd` path since
/// it always exists on Windows and keeps menu open off the PATH probe.
pub fn terminal_exe_path() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(r"C:\Windows\System32\cmd.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/bin/sh")
    }
}

/// Quote a path for Copy-as-path, Explorer-style: always wrapped in double
/// quotes. Idempotent: an already quoted value is returned unchanged.
pub fn quote_path_for_copy(path: &Path) -> String {
    let s = path.to_string_lossy().into_owned();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s
    } else {
        format!("\"{s}\"")
    }
}

/// Newline-joined quoted paths for a multi copy. Single path degrades to
/// [`quote_path_for_copy`].
pub fn join_paths_for_copy(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| quote_path_for_copy(p))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Raw file-attribute bits shared by [`attr_bits`] and [`set_attr_bits`].
/// Values match Windows `FILE_ATTRIBUTE_*`, so the Windows path is a mask
/// rather than a mapping and the fallback path synthesises the same bits.
pub const ATTR_READONLY: u32 = 0x1;
/// See [`ATTR_READONLY`].
pub const ATTR_HIDDEN: u32 = 0x2;

/// Bits [`set_attr_bits`] understands; anything else in a mask is ignored.
const ATTR_KNOWN: u32 = ATTR_READONLY | ATTR_HIDDEN;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn SetFileAttributesW(path: *const u16, attrs: u32) -> i32;
    fn GetCompressedFileSizeW(path: *const u16, high: *mut u32) -> u32;
    fn GetDiskFreeSpaceW(
        root: *const u16,
        sectors_per_cluster: *mut u32,
        bytes_per_sector: *mut u32,
        free_clusters: *mut u32,
        total_clusters: *mut u32,
    ) -> i32;
    fn GetLastError() -> u32;
}

#[cfg(windows)]
const INVALID_FILE_ATTRIBUTES: u32 = 0xFFFF_FFFF;
#[cfg(windows)]
const INVALID_FILE_SIZE: u32 = 0xFFFF_FFFF;
#[cfg(windows)]
const NO_ERROR: u32 = 0;

#[cfg(windows)]
fn wide_nul(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Recursive total for a folder: logical bytes across all reachable files plus
/// how many files and subfolders were seen. See [`walk_folder`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FolderSummary {
    pub bytes: u64,
    pub files: u64,
    pub folders: u64,
}

/// Recursively total `path` without opening any file: sizes come from
/// `symlink_metadata` lengths only.
///
/// Iterative stack over `std::fs`, so deep trees cannot overflow the call
/// stack. Returns `None` (fail-closed) for MTP / portable / Recycle Bin
/// locations and when `cancel` is set; permission errors skip that subtree
/// and keep going.
///
/// Never follows symlinks or reparse points (only
/// `file_type().is_symlink`, never a following `metadata().is_dir`), skips
/// `$Recycle.Bin` and `System Volume Information` at every level, and stops
/// descending past depth 64. `folders` counts descendant directories, not
/// `path` itself; a file `path` totals just that file.
pub fn walk_folder(path: &Path, cancel: &AtomicBool) -> Option<FolderSummary> {
    if crate::mtp::is_mtp(path)
        || crate::path_caps::is_portable(path)
        || crate::recycle_bin::is_recycle_bin(path)
    {
        return None;
    }
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    const MAX_DEPTH: u32 = 64;
    const CANCEL_EVERY: u64 = 256;

    let root_meta = std::fs::symlink_metadata(path).ok()?;
    if root_meta.file_type().is_symlink() {
        return Some(FolderSummary::default());
    }
    if root_meta.file_type().is_file() {
        return Some(FolderSummary {
            bytes: root_meta.len(),
            files: 1,
            folders: 0,
        });
    }
    if !root_meta.file_type().is_dir() {
        return Some(FolderSummary::default());
    }

    let mut summary = FolderSummary::default();
    let mut stack: Vec<(PathBuf, u32)> = vec![(path.to_path_buf(), 0)];
    let mut seen: u64 = 0;
    while let Some((dir, depth)) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue, // e.g. denied: skip the subtree, keep going
        };
        for entry in entries {
            seen += 1;
            if seen.is_multiple_of(CANCEL_EVERY) && cancel.load(Ordering::Relaxed) {
                return None;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            // `DirEntry::file_type` never follows links; `metadata()` would.
            let file_type = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            if is_skipped_system_name(&entry.file_name()) {
                continue;
            }
            if file_type.is_dir() {
                summary.folders += 1;
                if depth < MAX_DEPTH {
                    stack.push((entry.path(), depth + 1));
                }
            } else if file_type.is_file() {
                // Re-stat without following, so a symlink swapped in by a
                // race still cannot leak its target's size in.
                let meta = match std::fs::symlink_metadata(entry.path()) {
                    Ok(meta) if !meta.file_type().is_symlink() => meta,
                    _ => continue,
                };
                summary.files += 1;
                summary.bytes = summary.bytes.saturating_add(meta.len());
            }
        }
    }
    Some(summary)
}

/// Volume roots no walk may enter: per-volume recycle bins and the NTFS
/// system directory. Case-insensitive so a differently-cased variant cannot
/// sneak a system tree into a total.
fn is_skipped_system_name(name: &std::ffi::OsStr) -> bool {
    let s = name.to_string_lossy();
    s.eq_ignore_ascii_case("$Recycle.Bin") || s.eq_ignore_ascii_case("System Volume Information")
}

/// Read-only / hidden bits for `path`, or `None` on MTP-style locations and
/// when the attributes cannot be read. Read-only query, no mutation.
pub fn attr_bits(path: &Path) -> Option<u32> {
    if crate::mtp::is_mtp(path)
        || crate::path_caps::is_portable(path)
        || crate::recycle_bin::is_recycle_bin(path)
    {
        return None;
    }
    #[cfg(windows)]
    {
        // SAFETY: NUL-terminated path; read-only query with no side effects.
        let attrs = unsafe { GetFileAttributesW(wide_nul(path).as_ptr()) };
        if attrs == INVALID_FILE_ATTRIBUTES {
            return None;
        }
        Some(attrs & ATTR_KNOWN)
    }
    #[cfg(not(windows))]
    {
        let meta = std::fs::symlink_metadata(path).ok()?;
        let mut bits = 0;
        if meta.permissions().readonly() {
            bits |= ATTR_READONLY;
        }
        Some(bits)
    }
}

/// Flip attribute bits with one read-modify-write on Windows; elsewhere flips
/// the std readonly bit for [`ATTR_READONLY`] and bails on [`ATTR_HIDDEN`].
/// Refuses MTP locations like the other mutating calls.
pub fn set_attr_bits(path: &Path, set_mask: u32, clear_mask: u32) -> Result<()> {
    refuse_mtp(path)?;
    #[cfg(windows)]
    {
        let wide = wide_nul(path);
        // SAFETY: NUL-terminated path for a single read-modify-write.
        let current = unsafe { GetFileAttributesW(wide.as_ptr()) };
        if current == INVALID_FILE_ATTRIBUTES {
            bail!("Cannot read attributes for \"{}\".", path.display());
        }
        let next = (current | (set_mask & ATTR_KNOWN)) & !(clear_mask & ATTR_KNOWN);
        // SAFETY: same NUL-terminated path; the single write call.
        if unsafe { SetFileAttributesW(wide.as_ptr(), next) } == 0 {
            bail!("Cannot set attributes for \"{}\".", path.display());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        if (set_mask | clear_mask) & ATTR_HIDDEN != 0 {
            bail!("The hidden attribute is Windows-only.");
        }
        if (set_mask | clear_mask) & ATTR_READONLY == 0 {
            return Ok(()); // nothing this platform can change
        }
        let meta = std::fs::symlink_metadata(path)?;
        let mut perms = meta.permissions();
        // Set wins when both masks name the bit.
        perms.set_readonly(
            (set_mask & ATTR_READONLY) != 0
                || (perms.readonly() && (clear_mask & ATTR_READONLY) == 0),
        );
        std::fs::set_permissions(path, perms)?;
        Ok(())
    }
}

/// Explorer's "Size on disk": the compressed size rounded up to whole
/// clusters. `logical` (e.g. a [`walk_folder`] total) is what gets rounded
/// for directories, which have no meaningful compressed size of their own.
/// `None` on MTP-style locations, unreadable paths, and off Windows.
pub fn size_on_disk(path: &Path, logical: u64) -> Option<u64> {
    if crate::mtp::is_mtp(path)
        || crate::path_caps::is_portable(path)
        || crate::recycle_bin::is_recycle_bin(path)
    {
        return None;
    }
    #[cfg(windows)]
    {
        // `symlink_metadata` so a link is recognised without following it.
        if let Ok(meta) = std::fs::symlink_metadata(path)
            && meta.file_type().is_dir()
        {
            return Some(crate::thumbs::round_to_cluster(
                logical,
                cluster_bytes(path),
            ));
        }
        let wide = wide_nul(path);
        let mut high: u32 = 0;
        // SAFETY: NUL-terminated path; `high` is a valid out-param.
        let low = unsafe { GetCompressedFileSizeW(wide.as_ptr(), &mut high as *mut u32) };
        // 0xFFFFFFFF is also a valid low half; only a real error fails.
        if low == INVALID_FILE_SIZE && unsafe { GetLastError() } != NO_ERROR {
            return None;
        }
        let compressed = ((high as u64) << 32) | low as u64;
        Some(crate::thumbs::round_to_cluster(
            compressed,
            cluster_bytes(path),
        ))
    }
    #[cfg(not(windows))]
    {
        let _ = logical;
        None
    }
}

/// Bytes per cluster on the volume holding `path`, via the volume root's
/// `GetDiskFreeSpaceW`. Zero when unknown, so `round_to_cluster` degrades to
/// identity (see [`crate::thumbs::round_to_cluster`]).
#[cfg(windows)]
fn cluster_bytes(path: &Path) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    let mut root = volume_root(path).into_os_string();
    if !root.to_string_lossy().ends_with('\\') {
        root.push("\\");
    }
    let wide: Vec<u16> = root.encode_wide().chain(Some(0)).collect();
    let (mut sectors, mut bytes) = (0u32, 0u32);
    // SAFETY: NUL-terminated root; out-params valid; nulls ask to skip.
    let ok = unsafe {
        GetDiskFreeSpaceW(
            wide.as_ptr(),
            &mut sectors as *mut u32,
            &mut bytes as *mut u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        0
    } else {
        (sectors as u64).checked_mul(bytes as u64).unwrap_or(0)
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

    static TEST_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// Collision-free scratch dir per test; the shared `tmp()` above races
    /// when tests run in parallel.
    fn unique_tmp(tag: &str) -> PathBuf {
        let n = TEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ply-fsops-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Incompressible bytes so a compressed volume cannot shrink the file
    /// under its logical size mid-test.
    fn noisy_bytes(n: usize) -> Vec<u8> {
        let mut x: u32 = 0x1234_5678;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    #[test]
    fn attr_consts_match_windows_bits() {
        assert_eq!(ATTR_READONLY, 0x1);
        assert_eq!(ATTR_HIDDEN, 0x2);
    }

    #[test]
    fn attr_bits_refuses_portable() {
        let mtp = Path::new(r"\\MTP\DEVICE\o1");
        assert!(attr_bits(mtp).is_none());
        assert!(set_attr_bits(mtp, ATTR_READONLY, 0).is_err());
    }

    #[test]
    fn walker_counts_nested_tree() {
        let dir = unique_tmp("walktree");
        std::fs::write(dir.join("a.txt"), vec![0u8; 10]).unwrap();
        let sub = dir.join("sub");
        std::fs::create_dir_all(sub.join("deep")).unwrap();
        std::fs::write(sub.join("b.txt"), vec![0u8; 20]).unwrap();
        std::fs::write(sub.join("deep/c.txt"), vec![0u8; 30]).unwrap();
        std::fs::create_dir(dir.join("emptydir")).unwrap();

        let summary = walk_folder(&dir, &AtomicBool::new(false)).expect("real dir must walk");
        assert_eq!(summary.files, 3);
        assert_eq!(summary.folders, 3); // sub, deep, emptydir — not the root
        assert_eq!(summary.bytes, 60);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn walker_skips_system_names() {
        let dir = unique_tmp("walksys");
        std::fs::write(dir.join("keep.txt"), vec![0u8; 5]).unwrap();
        let bin = dir.join("$Recycle.Bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("junk.bin"), vec![0u8; 100]).unwrap();
        let svi = dir.join("System Volume Information");
        std::fs::create_dir_all(&svi).unwrap();
        std::fs::write(svi.join("tracking.log"), vec![0u8; 50]).unwrap();

        let summary = walk_folder(&dir, &AtomicBool::new(false)).expect("real dir must walk");
        assert_eq!(summary.files, 1);
        assert_eq!(summary.bytes, 5);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn walker_does_not_follow_symlinks_outside() {
        let dir = unique_tmp("walksym");
        let outside = unique_tmp("walksym-out");
        std::fs::write(outside.join("secret.bin"), vec![0u8; 100]).unwrap();
        std::fs::write(dir.join("inner.txt"), vec![0u8; 7]).unwrap();
        let file_link = dir.join("escape.txt");
        let dir_link = dir.join("escape-dir");

        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_file(outside.join("secret.bin"), &file_link)
            .is_ok()
            && std::os::windows::fs::symlink_dir(&outside, &dir_link).is_ok();
        #[cfg(unix)]
        let created = std::os::unix::fs::symlink(outside.join("secret.bin"), &file_link).is_ok()
            && std::os::unix::fs::symlink(&outside, &dir_link).is_ok();
        #[cfg(not(any(windows, unix)))]
        let created = false;

        if !created {
            // Symlink creation needs privileges it can be denied; skip
            // rather than fail the whole run on such machines.
            std::fs::remove_dir_all(&dir).ok();
            std::fs::remove_dir_all(&outside).ok();
            return;
        }

        let summary = walk_folder(&dir, &AtomicBool::new(false)).expect("real dir must walk");
        assert_eq!(
            summary.files, 1,
            "the linked outside file must stay uncounted"
        );
        assert_eq!(summary.bytes, 7);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn walker_cancel_upfront_returns_none() {
        let dir = unique_tmp("walkcancel");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        assert!(walk_folder(&dir, &AtomicBool::new(true)).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn walker_refuses_virtual_locations() {
        let cancel = AtomicBool::new(false);
        assert!(walk_folder(Path::new(r"\\MTP\DEVICE\o1"), &cancel).is_none());
        assert!(walk_folder(Path::new(crate::recycle_bin::ROOT_STR), &cancel).is_none());
    }

    #[test]
    fn attr_readonly_round_trip() {
        let dir = unique_tmp("attrro");
        let file = dir.join("probe.txt");
        std::fs::write(&file, b"x").unwrap();

        set_attr_bits(&file, ATTR_READONLY, 0).unwrap();
        assert!(
            attr_bits(&file).expect("attrs must read back") & ATTR_READONLY != 0,
            "readonly bit must read back after set"
        );

        set_attr_bits(&file, 0, ATTR_READONLY).unwrap();
        assert!(
            attr_bits(&file).expect("attrs must read back") & ATTR_READONLY == 0,
            "readonly bit must clear"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn attr_hidden_round_trip_windows() {
        let dir = unique_tmp("attrhid");
        let file = dir.join("probe.txt");
        std::fs::write(&file, b"x").unwrap();

        set_attr_bits(&file, ATTR_HIDDEN, 0).unwrap();
        assert!(
            attr_bits(&file).expect("attrs must read back") & ATTR_HIDDEN != 0,
            "hidden bit must read back after set"
        );

        set_attr_bits(&file, 0, ATTR_HIDDEN).unwrap();
        assert!(
            attr_bits(&file).expect("attrs must read back") & ATTR_HIDDEN == 0,
            "hidden bit must clear"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(not(windows))]
    #[test]
    fn attr_hidden_bails_off_windows() {
        let dir = unique_tmp("attrhid");
        let file = dir.join("probe.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(set_attr_bits(&file, ATTR_HIDDEN, 0).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn size_on_disk_none_or_rounded() {
        let dir = unique_tmp("sizedisk");
        let file = dir.join("data.bin");
        std::fs::write(&file, noisy_bytes(12345)).unwrap();

        match size_on_disk(&file, 12345) {
            None => {} // off Windows: always None
            Some(v) => assert!(v >= 12345, "rounded size {v} must cover logical 12345"),
        }
        let empty = dir.join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        match size_on_disk(&empty, 0) {
            None => {}
            Some(v) => assert_eq!(v, 0, "empty file takes no clusters"),
        }
        assert!(size_on_disk(&dir.join("missing.bin"), 10).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn size_on_disk_directory_rounds_logical() {
        let dir = unique_tmp("sizedir");
        match size_on_disk(&dir, 5000) {
            None => {} // off Windows: always None
            Some(v) => assert!(v >= 5000, "rounded dir size {v} must cover logical 5000"),
        }
        std::fs::remove_dir_all(&dir).ok();
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
            assert_eq!(
                trash.len(),
                paths.len(),
                "every trashable path lands in trash"
            );
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
        assert!(
            res.is_err(),
            "a batch touching a portable root must be refused whole"
        );
        assert!(
            file.exists(),
            "nothing may be planned/deleted when a batch is refused"
        );
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
        assert!(
            sentinel.exists(),
            "a refused root must not touch anything else"
        );
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
        assert!(
            file.exists(),
            "the root guard must abort before any deletion"
        );
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
        assert!(
            res.is_err(),
            "a root trailing the batch must refuse the whole batch"
        );
        assert!(
            file.exists(),
            "nothing may be deleted when any batch member is a root"
        );
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

        delete_permanently(std::slice::from_ref(&link)).unwrap();
        assert!(!link.exists(), "the symlink itself must be removed");
        assert!(target.exists(), "the symlink target must survive deletion");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn quote_path_always_quotes_explorer_style() {
        assert_eq!(
            quote_path_for_copy(Path::new(r"C:\Users\me\notes.txt")),
            r#""C:\Users\me\notes.txt""#
        );
        assert_eq!(
            quote_path_for_copy(Path::new(r"C:\Users\me\my notes.txt")),
            r#""C:\Users\me\my notes.txt""#
        );
        assert_eq!(
            quote_path_for_copy(Path::new(r#""C:\already quoted.txt""#)),
            r#""C:\already quoted.txt""#
        );
    }

    #[test]
    fn join_paths_quotes_each_and_newline_joins() {
        let paths = vec![
            PathBuf::from(r"C:\a.txt"),
            PathBuf::from(r"C:\my docs\b.txt"),
        ];
        assert_eq!(
            join_paths_for_copy(&paths),
            "\"C:\\a.txt\"\n\"C:\\my docs\\b.txt\""
        );
        assert!(join_paths_for_copy(&[]).is_empty());
        assert_eq!(
            join_paths_for_copy(&[PathBuf::from(r"C:\a.txt")]),
            quote_path_for_copy(Path::new(r"C:\a.txt"))
        );
    }

    #[test]
    fn friendly_app_name_refuses_portable_and_bin() {
        assert!(friendly_app_name(Path::new(r"\\MTP\DEVICE\o1")).is_none());
        assert!(friendly_app_name(Path::new(crate::recycle_bin::ROOT_STR)).is_none());
    }

    #[test]
    fn open_with_apps_empty_on_mtp_bin_and_missing() {
        assert!(list_open_with_apps(Path::new(r"\\MTP\DEVICE\o1")).is_empty());
        assert!(list_open_with_apps(Path::new(crate::recycle_bin::ROOT_STR)).is_empty());
        // No extension (and does not exist): nothing to enumerate.
        let dir = unique_tmp("openwith-missing");
        let missing = dir.join("missing-noext");
        assert!(!missing.exists());
        assert!(list_open_with_apps(&missing).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn open_with_apps_names_non_empty_for_txt() {
        let dir = unique_tmp("openwith-txt");
        let file = dir.join("probe.txt");
        std::fs::write(&file, b"x").unwrap();
        let apps = list_open_with_apps(&file);
        assert!(
            !apps.is_empty(),
            ".txt must have at least one recommended handler on Windows"
        );
        assert!(
            apps.len() <= OPEN_WITH_CAP,
            "app list must stay capped at {OPEN_WITH_CAP}"
        );
        for app in &apps {
            assert!(
                !app.name.trim().is_empty(),
                "every app name must be non-empty"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn open_with_apps_icons_absolute_when_present() {
        let dir = unique_tmp("openwith-icons");
        let file = dir.join("probe.txt");
        std::fs::write(&file, b"x").unwrap();
        let apps = list_open_with_apps(&file);
        assert!(
            apps.len() <= OPEN_WITH_CAP,
            "app list must stay capped at {OPEN_WITH_CAP}"
        );
        for app in &apps {
            assert!(
                !app.name.trim().is_empty(),
                "every app name must be non-empty"
            );
            if !app.icon.as_os_str().is_empty() {
                assert!(
                    app.icon.is_absolute(),
                    "icon path must be absolute, got {}",
                    app.icon.display()
                );
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn open_with_apps_expand_env_vars() {
        unsafe { std::env::set_var("PLY_TEST_EXPAND", r"C:\Windows") };
        assert_eq!(
            expand_env_vars(r"%PLY_TEST_EXPAND%\notepad.exe"),
            r"C:\Windows\notepad.exe"
        );
        assert_eq!(expand_env_vars(r"%PLY_NO_SUCH_VAR_XYZ%\a.exe"), r"\a.exe");
        assert_eq!(expand_env_vars(r"C:\plain\a.exe"), r"C:\plain\a.exe");
        unsafe { std::env::remove_var("PLY_TEST_EXPAND") };
    }
}
