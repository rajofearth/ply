//! Windows file clipboard publishing for local filesystem Entries.
//!
//! This module only publishes paths. A destination owns the later paste, so a
//! Cut never moves or removes its sources. Text clipboard operations stay in
//! `field.rs` and the existing Copy-as-path shortcut stays in `ui`.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use gpui::Window;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    Copy,
    Cut,
}

/// Whether a complete selection can be published as local filesystem paths.
/// `all` is deliberate: a mixed selection must not publish a partial payload.
pub(crate) fn eligible(paths: &[PathBuf]) -> bool {
    !paths.is_empty() && paths.iter().all(|path| is_eligible_path(path))
}

#[cfg(windows)]
fn is_eligible_path(path: &Path) -> bool {
    path.is_absolute() && !is_virtual_path(path)
}

#[cfg(not(windows))]
fn is_eligible_path(_path: &Path) -> bool {
    false
}

#[cfg(windows)]
fn is_virtual_path(path: &Path) -> bool {
    crate::mtp::is_mtp(path)
        || path == Path::new(crate::recycle_bin::ROOT_STR)
        || path
            .to_string_lossy()
            .starts_with(concat!(r"\\RecycleBin", "\\"))
}

#[cfg(windows)]
pub(crate) fn write(operation: Operation, paths: Vec<PathBuf>, window: &Window) -> Result<()> {
    if !eligible(&paths) {
        bail!("the selection contains a non-local filesystem path")
    }

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;

    let handle = <Window as HasWindowHandle>::window_handle(window)
        .map_err(|err| anyhow::anyhow!("could not get the Ply window handle: {err}"))?;
    let hwnd = match handle.as_raw() {
        RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as *mut c_void),
        _ => bail!("Ply did not provide a Win32 window handle"),
    };

    write_windows(operation, &paths, hwnd)
}

#[cfg(not(windows))]
pub(crate) fn write(_operation: Operation, _paths: Vec<PathBuf>, _window: &Window) -> Result<()> {
    bail!("Windows file clipboard is unavailable on this platform")
}

#[cfg(windows)]
fn write_windows(
    operation: Operation,
    paths: &[PathBuf],
    hwnd: windows::Win32::Foundation::HWND,
) -> Result<()> {
    use windows::Win32::System::DataExchange::{EmptyClipboard, RegisterClipboardFormatW};
    use windows::Win32::System::Ole::CF_HDROP;

    let preferred_format =
        unsafe { RegisterClipboardFormatW(windows::core::w!("Preferred DropEffect")) };
    if preferred_format == 0 {
        bail!("could not register the Preferred DropEffect clipboard format")
    }

    let _clipboard = ClipboardGuard::open(hwnd)?;
    unsafe { EmptyClipboard()? };

    set_clipboard_bytes(&hdrop_payload(paths), CF_HDROP.0 as u32)?;
    let effect = preferred_drop_effect(operation).to_le_bytes();
    if let Err(error) = set_clipboard_bytes(&effect, preferred_format) {
        // SetClipboardData transfers ownership only for the format it accepts.
        // If the second write fails, remove the already-published CF_HDROP
        // while this process still owns the open clipboard. Otherwise a Cut
        // could be left behind without its advisory move effect.
        return match unsafe { EmptyClipboard() } {
            Ok(()) => Err(anyhow::anyhow!(
                "could not publish Preferred DropEffect; clipboard was cleared: {error}"
            )),
            Err(clear_error) => Err(anyhow::anyhow!(
                "could not publish Preferred DropEffect: {error}; could not clear partial clipboard data: {clear_error}"
            )),
        };
    }
    Ok(())
}

#[cfg(windows)]
struct ClipboardGuard;

#[cfg(windows)]
impl ClipboardGuard {
    fn open(hwnd: windows::Win32::Foundation::HWND) -> Result<Self> {
        use windows::Win32::System::DataExchange::OpenClipboard;

        unsafe { OpenClipboard(Some(hwnd)) }
            .map_err(|err| anyhow::anyhow!("Windows clipboard is busy or unavailable: {err}"))?;
        Ok(Self)
    }
}

#[cfg(windows)]
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        use windows::Win32::System::DataExchange::CloseClipboard;

        let _ = unsafe { CloseClipboard() };
    }
}

#[cfg(windows)]
fn set_clipboard_bytes(data: &[u8], format: u32) -> Result<()> {
    use std::mem::forget;
    use std::ptr::copy_nonoverlapping;
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::SetClipboardData;
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows::core::Owned;

    let global: Owned<HGLOBAL> = unsafe { Owned::new(GlobalAlloc(GMEM_MOVEABLE, data.len())?) };
    let locked = unsafe { GlobalLock(*global) };
    if locked.is_null() {
        bail!("GlobalLock failed while preparing clipboard data")
    }
    unsafe {
        copy_nonoverlapping(data.as_ptr(), locked.cast(), data.len());
        let _ = GlobalUnlock(*global);
        SetClipboardData(format, Some(HANDLE((*global).0)))?;
    }
    forget(global);
    Ok(())
}

#[cfg(windows)]
fn hdrop_payload(paths: &[PathBuf]) -> Vec<u8> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::copy_nonoverlapping;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::Shell::DROPFILES;
    use windows::core::BOOL;

    let header = DROPFILES {
        pFiles: size_of::<DROPFILES>() as u32,
        pt: POINT { x: 0, y: 0 },
        fNC: BOOL(0),
        fWide: BOOL(1),
    };
    let mut wide = Vec::new();
    for path in paths {
        wide.extend(path.as_os_str().encode_wide());
        wide.push(0);
    }
    wide.push(0);

    let header_size = size_of::<DROPFILES>();
    let mut bytes = vec![0u8; header_size + wide.len() * size_of::<u16>()];
    unsafe {
        copy_nonoverlapping(
            (&header as *const DROPFILES).cast::<u8>(),
            bytes.as_mut_ptr(),
            header_size,
        );
        copy_nonoverlapping(
            wide.as_ptr().cast::<u8>(),
            bytes.as_mut_ptr().add(header_size),
            wide.len() * size_of::<u16>(),
        );
    }
    bytes
}

#[cfg(windows)]
fn preferred_drop_effect(operation: Operation) -> u32 {
    match operation {
        Operation::Copy => 1,
        Operation::Cut => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn eligibility_requires_every_path_to_be_local_and_absolute() {
        assert!(eligible(&[PathBuf::from(r"C:\Users\me\one.txt")]));
        assert!(eligible(&[
            PathBuf::from(r"C:\Users\me\one.txt"),
            PathBuf::from(r"\\server\share\two.txt"),
        ]));
        assert!(!eligible(&[]));
        assert!(!eligible(&[PathBuf::from("relative.txt")]));
        assert!(!eligible(&[
            PathBuf::from(r"C:\Users\me\one.txt"),
            PathBuf::from(r"\\MTP\device\object"),
        ]));
        assert!(!eligible(&[PathBuf::from(r"\\RecycleBin\item")]));
    }

    #[cfg(windows)]
    #[test]
    fn hdrop_payload_is_wide_and_double_nul_terminated() {
        use std::mem::size_of;
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::UI::Shell::DROPFILES;

        let paths = [PathBuf::from(r"C:\one.txt"), PathBuf::from(r"C:\two.txt")];
        let payload = hdrop_payload(&paths);
        assert_eq!(
            u32::from_ne_bytes(payload[..4].try_into().unwrap()),
            size_of::<DROPFILES>() as u32
        );
        assert_eq!(payload[16..20], [1, 0, 0, 0]);

        let words = &payload[size_of::<DROPFILES>()..]
            .chunks(2)
            .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        let expected: Vec<u16> = paths
            .iter()
            .flat_map(|path| path.as_os_str().encode_wide().chain(Some(0)))
            .chain(Some(0))
            .collect();
        assert_eq!(*words, expected);
    }

    #[cfg(windows)]
    #[test]
    fn drop_effect_values_match_shell_contract() {
        assert_eq!(preferred_drop_effect(Operation::Copy), 1);
        assert_eq!(preferred_drop_effect(Operation::Cut), 2);
    }

    #[cfg(not(windows))]
    #[test]
    fn outbound_file_clipboard_is_disabled_off_windows() {
        assert!(!eligible(&[PathBuf::from("/home/me/file.txt")]));
    }
}
