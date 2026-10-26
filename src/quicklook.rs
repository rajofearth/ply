//! Bounded, on-demand Quick Look data preparation.
//!
//! This first slice deliberately handles plain text and Windows Imaging
//! Component (WIC) images only. It never persists rich preview data, follows
//! symlinks, starts helpers, or decodes on the UI thread. Unsupported input is
//! a normal fallback to the existing shell icon, metadata, and Open actions.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

pub const MAX_READ_BYTES: usize = 256 * 1024;
pub const MAX_RENDER_BYTES: usize = 64 * 1024;
pub const MAX_RENDER_LINES: usize = 4_000;
pub const MAX_IMAGE_RGBA_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RICH_INPUT_BYTES: u64 = 64 * 1024 * 1024;
pub const IMAGE_RENDER_SIDE: u32 = 2_048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug)]
pub enum PreviewData {
    Text {
        text: String,
        truncated: bool,
    },
    Image {
        pixels: Vec<u8>,
        width: u32,
        height: u32,
    },
    Fallback,
}

#[derive(Debug)]
pub enum LoadError {
    Changed,
    Io,
}

impl From<io::Error> for LoadError {
    fn from(error: io::Error) -> Self {
        let _ = error;
        Self::Io
    }
}

#[derive(Debug)]
pub struct LoadedPreview {
    pub stamp: FileStamp,
    pub data: PreviewData,
}

/// A changed source invalidates the request. Ordinary I/O failure still gets
/// the safe shell/metadata fallback, but a result from a different file must
/// never replace the request with fallback content.
pub(crate) fn load_result_is_stale(
    result: &Result<LoadedPreview, LoadError>,
    requested: &FileStamp,
) -> bool {
    match result {
        Err(LoadError::Changed) => true,
        Ok(loaded) => loaded.stamp != *requested,
        Err(LoadError::Io) => false,
    }
}

pub(crate) fn entry_allows_preview(kind: &crate::listing::EntryKind) -> bool {
    matches!(kind, crate::listing::EntryKind::File)
}

fn rich_image_admitted(size: u64) -> bool {
    size <= MAX_RICH_INPUT_BYTES
}

pub fn stamp(path: &Path) -> io::Result<FileStamp> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || is_link_like_metadata(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Quick Look requires a regular non-link file",
        ));
    }
    Ok(FileStamp {
        size: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

pub(crate) fn is_link_like(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| is_link_like_metadata(&metadata))
        .unwrap_or(false)
}

fn is_link_like_metadata(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

pub fn load(path: &Path, cancelled: &AtomicBool) -> Result<LoadedPreview, LoadError> {
    let before = match stamp(path) {
        Ok(stamp) => stamp,
        Err(_) if is_link_like(path) => return Err(LoadError::Changed),
        Err(_) => return Err(LoadError::Io),
    };
    if cancelled.load(Ordering::Relaxed) {
        return Err(LoadError::Changed);
    }

    let data = if is_likely_image(path) {
        if !rich_image_admitted(before.size) {
            PreviewData::Fallback
        } else {
            #[cfg(windows)]
            {
                decode_wic(path, cancelled)
                    .map(|(pixels, width, height)| PreviewData::Image {
                        pixels,
                        width,
                        height,
                    })
                    .unwrap_or(PreviewData::Fallback)
            }
            #[cfg(not(windows))]
            {
                let _ = cancelled;
                PreviewData::Fallback
            }
        }
    } else if is_likely_text(path) {
        match read_text(path, before.size, cancelled) {
            Ok(Some((text, truncated))) => PreviewData::Text { text, truncated },
            Ok(None) => PreviewData::Fallback,
            Err(error) => return Err(error),
        }
    } else {
        PreviewData::Fallback
    };

    if cancelled.load(Ordering::Relaxed) {
        return Err(LoadError::Changed);
    }
    let after = match stamp(path) {
        Ok(stamp) => stamp,
        Err(_) if is_link_like(path) => return Err(LoadError::Changed),
        Err(_) => return Err(LoadError::Io),
    };
    if before != after {
        return Err(LoadError::Changed);
    }
    Ok(LoadedPreview { stamp: after, data })
}

fn read_text(
    path: &Path,
    expected_size: u64,
    cancelled: &AtomicBool,
) -> Result<Option<(String, bool)>, LoadError> {
    let mut file = File::open(path)?;
    let mut bytes = Vec::with_capacity(MAX_READ_BYTES);
    file.by_ref()
        .take(MAX_READ_BYTES as u64)
        .read_to_end(&mut bytes)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(LoadError::Changed);
    }
    let truncated = expected_size > MAX_READ_BYTES as u64;

    let text = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        String::from_utf8(bytes[3..].to_vec()).ok()
    } else if bytes.starts_with(&[0xff, 0xfe]) {
        decode_utf16(&bytes[2..], false)
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        decode_utf16(&bytes[2..], true)
    } else if looks_binary(&bytes) {
        None
    } else {
        String::from_utf8(bytes).ok()
    };

    let Some(text) = text else {
        return Ok(None);
    };
    if looks_binary(text.as_bytes()) {
        return Ok(None);
    }
    let (text, render_truncated) = bound_text(&text);
    Ok(Some((text, truncated || render_truncated)))
}

fn decode_utf16(bytes: &[u8], big_endian: bool) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units = bytes.as_chunks::<2>().0.iter().map(|pair| {
        if big_endian {
            u16::from_be_bytes([pair[0], pair[1]])
        } else {
            u16::from_le_bytes([pair[0], pair[1]])
        }
    });
    String::from_utf16(&units.collect::<Vec<_>>()).ok()
}

fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return true;
    }
    let controls = bytes
        .iter()
        .filter(|&&byte| byte < 0x20 && !matches!(byte, b'\n' | b'\r' | b'\t' | 0x0c))
        .count();
    controls.saturating_mul(10) > bytes.len()
}

fn bound_text(input: &str) -> (String, bool) {
    const MARKER: &str = "[Preview truncated]";
    let needs_truncation =
        input.len() > MAX_RENDER_BYTES || input.lines().count() > MAX_RENDER_LINES;
    if !needs_truncation {
        return (input.to_owned(), false);
    }

    let room = MAX_RENDER_BYTES.saturating_sub(MARKER.len() + 1);
    let mut end = input.len().min(room);
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    let mut output = input[..end].to_owned();
    if output.lines().count() > MAX_RENDER_LINES.saturating_sub(1) {
        output = output
            .split_inclusive('\n')
            .take(MAX_RENDER_LINES.saturating_sub(1))
            .collect();
    }
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(MARKER);
    (output, true)
}

fn is_likely_image(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "bmp" | "gif" | "ico" | "jpg" | "jpeg" | "jpe" | "png" | "tif" | "tiff" | "webp"
    )
}

fn is_likely_text(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "c" | "cfg"
            | "cpp"
            | "csv"
            | "go"
            | "h"
            | "ini"
            | "java"
            | "js"
            | "jsx"
            | "json"
            | "log"
            | "md"
            | "py"
            | "rs"
            | "sh"
            | "toml"
            | "ts"
            | "tsx"
            | "txt"
            | "xml"
            | "yaml"
            | "yml"
    )
}

fn fit_dimensions(width: u32, height: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let max_pixels = (MAX_IMAGE_RGBA_BYTES / 4) as u64;
    let max_side = IMAGE_RENDER_SIDE as u64;
    let mut target_width = width as u64;
    let mut target_height = height as u64;

    if target_width.max(target_height) > max_side {
        if target_width >= target_height {
            target_height = (target_height * max_side / target_width).max(1);
            target_width = max_side;
        } else {
            target_width = (target_width * max_side / target_height).max(1);
            target_height = max_side;
        }
    }
    while target_width.saturating_mul(target_height) > max_pixels {
        if target_width >= target_height {
            target_width = (max_pixels / target_height).max(1);
        } else {
            target_height = (max_pixels / target_width).max(1);
        }
    }
    Some((target_width as u32, target_height as u32))
}

#[cfg(windows)]
fn decode_wic(path: &Path, cancelled: &AtomicBool) -> Option<(Vec<u8>, u32, u32)> {
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory,
        WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
        WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::core::PCWSTR;

    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            return None;
        }
        let result = (|| {
            if cancelled.load(Ordering::Relaxed) {
                return None;
            }
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let wide: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let decoder = factory
                .CreateDecoderFromFilename(
                    PCWSTR(wide.as_ptr()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .ok()?;
            let frame = decoder.GetFrame(0).ok()?;
            let mut width = 0;
            let mut height = 0;
            frame.GetSize(&mut width, &mut height).ok()?;
            let (width, height) = fit_dimensions(width, height)?;
            let bytes = (width as usize)
                .checked_mul(height as usize)?
                .checked_mul(4)?;
            if bytes > MAX_IMAGE_RGBA_BYTES {
                return None;
            }
            let scaler = factory.CreateBitmapScaler().ok()?;
            scaler
                .Initialize(&frame, width, height, WICBitmapInterpolationModeFant)
                .ok()?;
            let converter = factory.CreateFormatConverter().ok()?;
            converter
                .Initialize(
                    &scaler,
                    &GUID_WICPixelFormat32bppRGBA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .ok()?;
            if cancelled.load(Ordering::Relaxed) {
                return None;
            }
            let stride = width.checked_mul(4)?;
            let mut pixels = vec![0; bytes];
            converter
                .CopyPixels(std::ptr::null(), stride, &mut pixels)
                .ok()?;
            Some((pixels, width, height))
        })();
        CoUninitialize();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_detection_rejects_nuls_and_control_heavy_data() {
        assert!(looks_binary(b"hello\0world"));
        assert!(looks_binary(&[1; 20]));
        assert!(!looks_binary(b"hello\nworld\t"));
    }

    #[test]
    fn text_bounds_bytes_and_lines() {
        let (text, truncated) = bound_text(&"line\n".repeat(MAX_RENDER_LINES + 20));
        assert!(truncated);
        assert!(text.len() <= MAX_RENDER_BYTES);
        assert!(text.lines().count() <= MAX_RENDER_LINES);
    }

    #[test]
    fn text_bounds_at_utf8_boundary() {
        let input = "é".repeat(MAX_RENDER_BYTES);
        let (text, _) = bound_text(&input);
        assert!(text.is_char_boundary(text.len()));
        assert!(text.ends_with("[Preview truncated]"));
    }

    #[test]
    fn odd_utf16_payload_is_rejected() {
        assert!(decode_utf16(b"a", false).is_none());
    }

    #[test]
    fn load_result_changed_or_stamp_mismatch_is_stale() {
        let stamp = FileStamp {
            size: 4,
            modified: None,
        };
        let changed = Err(LoadError::Changed);
        assert!(load_result_is_stale(&changed, &stamp));

        let mismatch = Ok(LoadedPreview {
            stamp: FileStamp {
                size: 5,
                modified: None,
            },
            data: PreviewData::Fallback,
        });
        assert!(load_result_is_stale(&mismatch, &stamp));
    }

    #[test]
    fn rich_image_admission_stops_at_64_mib() {
        assert!(rich_image_admitted(64 * 1024 * 1024));
        assert!(!rich_image_admitted(64 * 1024 * 1024 + 1));
    }

    #[test]
    fn image_dimensions_are_bounded_before_pixels_exist() {
        let (width, height) = fit_dimensions(u32::MAX, u32::MAX).unwrap();
        assert!(width <= IMAGE_RENDER_SIDE);
        assert!(height <= IMAGE_RENDER_SIDE);
        assert!((width as usize) * (height as usize) * 4 <= MAX_IMAGE_RGBA_BYTES);
    }

    #[test]
    fn only_admitted_text_extensions_are_read() {
        assert!(is_likely_text(Path::new("notes.md")));
        assert!(!is_likely_text(Path::new("report.pdf")));
        assert!(!is_likely_text(Path::new("sheet.docx")));
    }

    #[test]
    fn entry_admission_rejects_symlinks_and_non_files() {
        use crate::listing::EntryKind;

        assert!(entry_allows_preview(&EntryKind::File));
        assert!(!entry_allows_preview(&EntryKind::Directory));
        assert!(!entry_allows_preview(&EntryKind::Symlink {
            target: std::path::PathBuf::from("target"),
        }));
        assert!(!entry_allows_preview(&EntryKind::Unknown));
    }

    #[test]
    fn image_extensions_are_narrow_and_case_insensitive() {
        assert!(is_likely_image(Path::new("photo.JpEg")));
        assert!(!is_likely_image(Path::new("notes.txt")));
    }
}
