//! Real file Cut/Copy/Paste via the OS clipboard.
//!
//! Ply also keeps an in-process snapshot so Paste works even when the platform
//! clipboard cannot advertise file lists (some GPUI/Linux setups). Copy path
//! does not go through this module.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Result, bail};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClipOp {
    Copy,
    Cut,
}

#[derive(Clone, Debug)]
pub struct FileClip {
    pub paths: Vec<PathBuf>,
    pub op: ClipOp,
}

static INTERNAL: Mutex<Option<FileClip>> = Mutex::new(None);

pub fn set(paths: Vec<PathBuf>, op: ClipOp) -> Result<()> {
    if paths.is_empty() {
        bail!("Nothing to put on the clipboard.");
    }
    if paths.iter().any(|p| crate::mtp::is_mtp(p)) {
        bail!("A portable device has no file clipboard.");
    }
    let _ = write_os(&paths, op);
    *INTERNAL.lock().expect("file clipboard") = Some(FileClip { paths, op });
    Ok(())
}

pub fn clear() {
    *INTERNAL.lock().expect("file clipboard") = None;
}

pub fn get() -> Option<FileClip> {
    if let Some(clip) = INTERNAL.lock().expect("file clipboard").clone() {
        return Some(clip);
    }
    read_os()
}

pub fn is_empty() -> bool {
    get().map(|c| c.paths.is_empty()).unwrap_or(true)
}

/// After a successful cut-paste, the clipboard is spent.
pub fn take_if_cut() -> Option<FileClip> {
    let mut guard = INTERNAL.lock().expect("file clipboard");
    match guard.as_ref() {
        Some(c) if c.op == ClipOp::Cut => guard.take(),
        _ => None,
    }
}

fn write_os(paths: &[PathBuf], op: ClipOp) -> Result<()> {
    #[cfg(windows)]
    {
        windows::write(paths, op)
    }
    #[cfg(target_os = "macos")]
    {
        macos::write(paths, op)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux::write(paths, op)
    }
}

fn read_os() -> Option<FileClip> {
    #[cfg(windows)]
    {
        windows::read()
    }
    #[cfg(target_os = "macos")]
    {
        macos::read()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux::read()
    }
}

pub fn file_uri(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut out = String::from("file://");
    if cfg!(windows) {
        out.push('/');
    }
    for ch in raw.chars() {
        match ch {
            '\\' if cfg!(windows) => out.push('/'),
            ' ' => out.push_str("%20"),
            '%' => out.push_str("%25"),
            '#' => out.push_str("%23"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn parse_file_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.trim().strip_prefix("file://")?;
    let decoded = percent_decode(rest);
    let path = if cfg!(windows) {
        decoded.trim_start_matches('/').replace('/', "\\")
    } else {
        decoded
    };
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(all(unix, not(target_os = "macos")))]
mod linux {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    pub fn write(paths: &[PathBuf], op: ClipOp) -> Result<()> {
        let verb = match op {
            ClipOp::Copy => "copy",
            ClipOp::Cut => "cut",
        };
        let mut body = String::from(verb);
        body.push('\n');
        for p in paths {
            body.push_str(&file_uri(p));
            body.push('\n');
        }
        // GNOME/Nautilus. xclip serves one target; this is the one paste cares about.
        pipe_xclip("x-special/gnome-copied-files", body.as_bytes())?;
        Ok(())
    }

    pub fn read() -> Option<FileClip> {
        if let Some(clip) = parse_gnome(&xclip_out("x-special/gnome-copied-files")?) {
            return Some(clip);
        }
        parse_uri_list(&xclip_out("text/uri-list")?)
    }

    fn parse_gnome(text: &str) -> Option<FileClip> {
        let mut lines = text.lines();
        let op = match lines.next()?.trim() {
            "cut" => ClipOp::Cut,
            "copy" => ClipOp::Copy,
            _ => return None,
        };
        let paths: Vec<PathBuf> = lines.filter_map(parse_file_uri).collect();
        if paths.is_empty() {
            None
        } else {
            Some(FileClip { paths, op })
        }
    }

    fn parse_uri_list(text: &str) -> Option<FileClip> {
        let paths: Vec<PathBuf> = text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .filter_map(parse_file_uri)
            .collect();
        if paths.is_empty() {
            None
        } else {
            Some(FileClip {
                paths,
                op: ClipOp::Copy,
            })
        }
    }

    fn pipe_xclip(target: &str, bytes: &[u8]) -> Result<()> {
        if which("xclip").is_none() {
            return Ok(());
        }
        let mut child = Command::new("xclip")
            .args(["-selection", "clipboard", "-t", target])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(bytes)?;
        }
        Ok(())
    }

    fn xclip_out(target: &str) -> Option<String> {
        let out = Command::new("xclip")
            .args(["-selection", "clipboard", "-o", "-t", target])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8(out.stdout).ok()
    }

    fn which(name: &str) -> Option<PathBuf> {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths).find_map(|p| {
                let bin = p.join(name);
                bin.is_file().then_some(bin)
            })
        })
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::process::Command;

    pub fn write(paths: &[PathBuf], op: ClipOp) -> Result<()> {
        let list = paths
            .iter()
            .map(|p| format!("POSIX file \"{}\"", p.display()))
            .collect::<Vec<_>>()
            .join(", ");
        let script = format!("set the clipboard to {{{list}}}");
        Command::new("osascript").args(["-e", &script]).status()?;
        let _ = op;
        Ok(())
    }

    pub fn read() -> Option<FileClip> {
        let out = Command::new("osascript")
            .args(["-e", "POSIX path of (the clipboard as «class furl»)"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8(out.stdout).ok()?;
        let paths: Vec<PathBuf> = text
            .lines()
            .map(|l| PathBuf::from(l.trim()))
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
        if paths.is_empty() {
            None
        } else {
            Some(FileClip {
                paths,
                op: ClipOp::Copy,
            })
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::os::windows::ffi::OsStrExt;

    const CF_HDROP: u32 = 15;
    const GMEM_MOVEABLE: u32 = 0x0002;
    const DROPEFFECT_COPY: u32 = 1;
    const DROPEFFECT_MOVE: u32 = 2;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn OpenClipboard(hwnd: *mut core::ffi::c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(format: u32, mem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
        fn GetClipboardData(format: u32) -> *mut core::ffi::c_void;
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut core::ffi::c_void;
        fn GlobalLock(mem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
        fn GlobalUnlock(mem: *mut core::ffi::c_void) -> i32;
        fn GlobalSize(mem: *mut core::ffi::c_void) -> usize;
    }

    #[repr(C)]
    struct Dropfiles {
        p_files: u32,
        x: i32,
        y: i32,
        nc: i32,
        wide: i32,
    }

    pub fn write(paths: &[PathBuf], op: ClipOp) -> Result<()> {
        let mut wide: Vec<u16> = Vec::new();
        for p in paths {
            wide.extend(p.as_os_str().encode_wide());
            wide.push(0);
        }
        wide.push(0);
        let header = std::mem::size_of::<Dropfiles>();
        let bytes = header + wide.len() * 2;
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                bail!("OpenClipboard failed");
            }
            EmptyClipboard();
            let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if mem.is_null() {
                CloseClipboard();
                bail!("GlobalAlloc failed");
            }
            let ptr = GlobalLock(mem) as *mut u8;
            let drop = Dropfiles {
                p_files: header as u32,
                x: 0,
                y: 0,
                nc: 0,
                wide: 1,
            };
            std::ptr::write(ptr as *mut Dropfiles, drop);
            std::ptr::copy_nonoverlapping(
                wide.as_ptr() as *const u8,
                ptr.add(header),
                wide.len() * 2,
            );
            GlobalUnlock(mem);
            SetClipboardData(CF_HDROP, mem);

            let name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
            let fmt = RegisterClipboardFormatW(name.as_ptr());
            if fmt != 0 {
                let effect = if op == ClipOp::Cut {
                    DROPEFFECT_MOVE
                } else {
                    DROPEFFECT_COPY
                };
                let emem = GlobalAlloc(GMEM_MOVEABLE, 4);
                if !emem.is_null() {
                    let ep = GlobalLock(emem) as *mut u32;
                    *ep = effect;
                    GlobalUnlock(emem);
                    SetClipboardData(fmt, emem);
                }
            }
            CloseClipboard();
        }
        Ok(())
    }

    pub fn read() -> Option<FileClip> {
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return None;
            }
            let mem = GetClipboardData(CF_HDROP);
            if mem.is_null() {
                CloseClipboard();
                return None;
            }
            let ptr = GlobalLock(mem) as *const u8;
            let drop = &*(ptr as *const Dropfiles);
            let list = ptr.add(drop.p_files as usize);
            let mut paths = Vec::new();
            if drop.wide != 0 {
                let mut offset = 0;
                loop {
                    let s = list.add(offset) as *const u16;
                    if *s == 0 {
                        break;
                    }
                    let mut len = 0;
                    while *s.add(len) != 0 {
                        len += 1;
                    }
                    let slice = std::slice::from_raw_parts(s, len);
                    paths.push(PathBuf::from(String::from_utf16_lossy(slice)));
                    offset += (len + 1) * 2;
                }
            }
            GlobalUnlock(mem);

            let mut op = ClipOp::Copy;
            let name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
            let fmt = RegisterClipboardFormatW(name.as_ptr());
            if fmt != 0 {
                let emem = GetClipboardData(fmt);
                if !emem.is_null() {
                    let ep = GlobalLock(emem) as *const u32;
                    if *ep == DROPEFFECT_MOVE {
                        op = ClipOp::Cut;
                    }
                    GlobalUnlock(emem);
                }
            }
            CloseClipboard();
            if paths.is_empty() {
                None
            } else {
                Some(FileClip { paths, op })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uri_roundtrip_unix_style() {
        if cfg!(windows) {
            return;
        }
        let p = PathBuf::from("/tmp/Some File.txt");
        let uri = file_uri(&p);
        assert_eq!(uri, "file:///tmp/Some%20File.txt");
        assert_eq!(parse_file_uri(&uri), Some(p));
    }

    #[test]
    fn internal_clipboard_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ply-clip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        set(vec![file.clone()], ClipOp::Copy).unwrap();
        let got = get().unwrap();
        assert_eq!(got.paths, vec![file]);
        assert_eq!(got.op, ClipOp::Copy);
        assert!(!is_empty());
        clear();
        std::fs::remove_dir_all(&dir).ok();
    }
}
