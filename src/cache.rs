//! Persistent on-disk thumbnail cache.
//!
//! Content thumbnails (the media/image path) are stored as PNGs on disk,
//! keyed by a hash of the source path bytes plus mtime stamp. On the second
//! open of a large folder, thumbnails are read from disk instead of
//! re-extracted through the shell, making the second open near-instant.
//!
//! Shell type-icons (class/path/index icons) are NOT persisted here; they
//! stay in-memory-only for now (small, cheap, OS-sourced).
//!
//! Layout follows the freedesktop Thumbnail Managing Standard:
//! `<data_dir>/ply/thumbcache/normal/<hex_key>.png`
//! `<data_dir>/ply/thumbcache/fail/<hex_key>` (empty marker)

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// FNV-1a hasher (stable, no crypto crate needed).
struct Fnv1a {
    state: u64,
}

impl Fnv1a {
    fn new() -> Self {
        Self {
            state: 0xcbf29ce484222325,
        }
    }

    fn write_u64(&mut self, v: u64) {
        self.state ^= v;
        self.state = self.state.wrapping_mul(0x100000001b3);
    }

    fn write_bytes(&mut self, b: &[u8]) {
        for &byte in b {
            self.state ^= byte as u64;
            self.state = self.state.wrapping_mul(0x100000001b3);
        }
    }

    fn finish_u64(self) -> u64 {
        self.state
    }
}

/// Hard ceiling for the on-disk cache (~128 MiB).
pub const DISK_CACHE_MAX_BYTES: u64 = 128 * 1024 * 1024;

/// Compute a stable content key from a source path and mtime stamp.
/// The same (path, stamp) always produces the same hex string; a different
/// stamp produces a different key (edited file re-extracts).
pub fn content_key(path: &Path, stamp: u64) -> String {
    let mut h = Fnv1a::new();
    h.write_bytes(path.as_os_str().to_string_lossy().as_bytes());
    h.write_u64(stamp);
    format!("{:016x}", h.finish_u64())
}

/// Base directory for the disk cache: `<data_dir>/ply/thumbcache/`.
fn cache_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("ply").join("thumbcache"))
}

/// PNG magic bytes (signature). Every file `store` writes starts with these.
const PNG_MAGIC: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// Read a cached thumbnail. Returns `(rgba_pixels, width, height)` on hit.
///
/// PNG-only by construction: `store` only ever writes PNG, so lookup decodes
/// with the `png` crate directly instead of sniffing all formats. A corrupt or
/// non-PNG file reads as a miss; the caller falls through to fresh shell
/// extraction and `store` overwrites the bad bytes, so the cache heals on
/// the next open.
pub fn lookup(key: &str) -> Option<(Vec<u8>, u32, u32)> {
    let dir = cache_dir()?.join("normal").join(format!("{key}.png"));
    let data = fs::read(&dir).ok()?;
    debug_assert!(
        data.starts_with(&PNG_MAGIC),
        "thumb cache lookup saw non-PNG bytes"
    );
    decode_png_rgba(&data)
}

/// Decode PNG bytes to raw RGBA8. Returns `None` instead of panicking on any
/// malformed input, so corrupt cache files heal via re-extraction.
fn decode_png_rgba(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if !data.starts_with(&PNG_MAGIC) {
        return None;
    }
    let decoder = png::Decoder::new(std::io::Cursor::new(data));
    let mut reader = decoder.read_info().ok()?;
    let size = reader.output_buffer_size()?;
    if size == 0 {
        return None;
    }
    let mut raw = vec![0u8; size];
    let info = reader.next_frame(&mut raw).ok()?;
    let (w, h) = (info.width, info.height);
    if w == 0 || h == 0 {
        return None;
    }
    let pixels = (w as usize)
        .checked_mul(h as usize)?
        .checked_mul(4)?;
    match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => {
            if raw.len() < pixels {
                return None;
            }
            raw.truncate(pixels);
            Some((raw, w, h))
        }
        (png::ColorType::Rgb, png::BitDepth::Eight) => {
            if raw.len() < pixels / 4 * 3 {
                return None;
            }
            let mut rgba = Vec::with_capacity(pixels);
            for px in raw[..pixels / 4 * 3].chunks_exact(3) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            Some((rgba, w, h))
        }
        (png::ColorType::Grayscale, png::BitDepth::Eight) => {
            if raw.len() < pixels / 4 {
                return None;
            }
            let mut rgba = Vec::with_capacity(pixels);
            for &g in &raw[..pixels / 4] {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
            Some((rgba, w, h))
        }
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => {
            if raw.len() < pixels / 2 {
                return None;
            }
            let mut rgba = Vec::with_capacity(pixels);
            for px in raw[..pixels / 2].chunks_exact(2) {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
            Some((rgba, w, h))
        }
        // Indexed, 16-bit, and sub-byte depths: `store` never writes these,
        // so read as a miss and let the caller heal (re-extract + overwrite).
        _ => None,
    }
}

/// Delete one cached thumbnail, e.g. after it fails to decode: the source
/// file is fine, only these bytes are corrupt, so the next open re-extracts
/// fresh instead of failing the same way forever. Best-effort.
pub fn remove(key: &str) {
    if let Some(dir) = cache_dir() {
        let _ = fs::remove_file(dir.join("normal").join(format!("{key}.png")));
    }
}

/// Whether a disk fail marker exists for this key.
pub fn disk_failed(key: &str) -> bool {
    cache_dir()
        .and_then(|d| fs::metadata(d.join("fail").join(key)).ok())
        .is_some()
}

/// Write a fail marker so the file is never retried (freedesktop `fail/`
/// lesson). Best-effort; ignores IO errors.
pub fn fail_mark(key: &str) {
    let Some(dir) = cache_dir() else { return };
    let fail_dir = dir.join("fail");
    let _ = fs::create_dir_all(&fail_dir);
    let _ = fs::write(fail_dir.join(key), "");
}

/// PNG-encode and write a thumbnail to the cache. Best-effort; ignores
/// IO errors so a disk failure never crashes the app.
pub fn store(key: &str, rgba: &[u8], w: u32, h: u32) {
    let Some(dir) = cache_dir() else { return };
    let normal_dir = dir.join("normal");
    let _ = fs::create_dir_all(&normal_dir);
    let Some(bytes) = encode_png_rgba(rgba, w, h) else {
        return;
    };
    let _ = fs::write(normal_dir.join(format!("{key}.png")), bytes);
}

/// PNG-encode raw RGBA8 bytes with the `png` crate directly.
fn encode_png_rgba(rgba: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    if w == 0 || h == 0 {
        return None;
    }
    let expect = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
    if rgba.len() != expect {
        return None;
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    Some(out)
}

/// Delete oldest-by-mtime files in `normal/` when total bytes exceed
/// `max_bytes`. Best-effort; never panics.
pub fn evict(max_bytes: u64) {
    let Some(dir) = cache_dir() else { return };
    evict_at(&dir.join("normal"), max_bytes);
}

/// Evict files in `normal_dir` whose aggregate size exceeds `max_bytes`,
/// oldest-first by mtime. Removing entries whose source file no longer exists
/// is a future extension; for now the cache is only size-bounded.
fn evict_at(normal_dir: &Path, max_bytes: u64) {
    let Ok(entries) = fs::read_dir(normal_dir) else {
        return;
    };

    // Collect (path, size, mtime) for every cached file.
    let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let size = meta.len();
        let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
        total += size;
        files.push((path, size, mtime));
    }

    // Sort oldest-first for LRU-style eviction.
    files.sort_by_key(|a| a.2);

    // Evict oldest entries until under the cap.
    for (path, size, _) in &files {
        if total <= max_bytes {
            break;
        }
        let _ = fs::remove_file(path);
        total = total.saturating_sub(*size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Unique temp dir per test so they never collide.
    fn temp_dir_for(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "ply_cache_test_{label}_{}_{}",
            std::process::id(),
            n
        ))
    }

    /// Store a PNG directly into a custom root (bypasses `dirs::data_dir()`).
    fn store_at(rgba: &[u8], w: u32, h: u32, key: &str, root: &Path) {
        let normal = root.join("normal");
        let _ = fs::create_dir_all(&normal);
        let bytes = encode_png_rgba(rgba, w, h).unwrap();
        let _ = fs::write(normal.join(format!("{key}.png")), bytes);
    }

    /// Read a PNG from a custom root.
    fn lookup_at(key: &str, root: &Path) -> Option<(Vec<u8>, u32, u32)> {
        let path = root.join("normal").join(format!("{key}.png"));
        let data = fs::read(&path).ok()?;
        decode_png_rgba(&data)
    }

    fn fail_mark_at(key: &str, root: &Path) {
        let fail_dir = root.join("fail");
        let _ = fs::create_dir_all(&fail_dir);
        let _ = fs::write(fail_dir.join(key), "");
    }

    fn disk_failed_at(key: &str, root: &Path) -> bool {
        root.join("fail").join(key).metadata().is_ok()
    }

    #[test]
    fn key_stability() {
        let p = Path::new("C:\\Users\\test\\photo.png");
        let stamp = 1_700_000_000_000_000_000u64;
        assert_eq!(content_key(p, stamp), content_key(p, stamp));
        assert_ne!(content_key(p, stamp), content_key(p, stamp + 1));
    }

    #[test]
    fn different_path_same_stamp_different_key() {
        let stamp = 42u64;
        assert_ne!(
            content_key(Path::new("a.png"), stamp),
            content_key(Path::new("b.png"), stamp)
        );
    }

    #[test]
    fn store_lookup_roundtrip() {
        let root = temp_dir_for("roundtrip");
        let _ = fs::create_dir_all(&root);
        let key = "deadbeef01234567";
        let rgba: Vec<u8> = (0..96 * 96 * 4).map(|i| (i % 256) as u8).collect();
        store_at(&rgba, 96, 96, key, &root);
        let (got, w, h) = lookup_at(key, &root).expect("cache miss after store");
        assert_eq!(w, 96);
        assert_eq!(h, 96);
        assert_eq!(got, rgba);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_bytes_return_none() {
        // Random bytes, bare magic plus garbage, and a truncated real PNG
        // must all read as a miss, never panic. The thumbs probe treats None
        // as a miss and heals via fresh extraction.
        assert!(decode_png_rgba(b"not a png at all").is_none());
        assert!(decode_png_rgba(&[137, 80, 78, 71, 13, 10, 26, 10, 0, 1, 2, 3]).is_none());
        assert!(decode_png_rgba(&[]).is_none());
        let rgba = vec![7u8; 4 * 4 * 4];
        let mut full = encode_png_rgba(&rgba, 4, 4).unwrap();
        full.truncate(full.len() / 2);
        assert!(decode_png_rgba(&full).is_none());
    }

    #[test]
    fn fail_mark_and_failed() {
        let root = temp_dir_for("failmark");
        let _ = fs::create_dir_all(&root);
        let key = "aabbccdd11223344";
        assert!(!disk_failed_at(key, &root));
        fail_mark_at(key, &root);
        assert!(disk_failed_at(key, &root));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn eviction_removes_oldest_first() {
        let root = temp_dir_for("evict");
        let normal = root.join("normal");
        let _ = fs::create_dir_all(&normal);

        // Create two files with identical content (same size).
        let rgba: Vec<u8> = vec![128u8; 96 * 96 * 4];
        let key_old = "0000000000000001";
        let key_new = "1111111111111111";
        store_at(&rgba, 96, 96, key_old, &root);
        store_at(&rgba, 96, 96, key_new, &root);

        let old_path = normal.join(format!("{key_old}.png"));
        let new_path = normal.join(format!("{key_new}.png"));
        let file_size = fs::metadata(&old_path).unwrap().len();
        assert_eq!(file_size, fs::metadata(&new_path).unwrap().len());

        // Cap at just under two files so exactly one must be evicted.
        let cap = file_size * 2 - 1;
        evict_at(&normal, cap);

        // Eviction is oldest-first by mtime. Both files were created in the
        // same test, so both have the same mtime. Exactly one should be
        // evicted to bring total under the cap. Which one is an
        // implementation detail (the sort is stable, so the first file in
        // directory order is evicted). Verify exactly one remains.
        let remaining: Vec<_> = fs::read_dir(&normal).unwrap().flatten().collect();
        assert_eq!(
            remaining.len(),
            1,
            "exactly one file should remain after eviction"
        );
        assert_ne!(
            remaining[0].path(),
            old_path,
            "the evicted file should be gone"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn eviction_under_cap_is_noop() {
        let root = temp_dir_for("noevict");
        let normal = root.join("normal");
        let _ = fs::create_dir_all(&normal);

        let rgba: Vec<u8> = vec![64u8; 96 * 96 * 4];
        let key = "aabbccdd11223344";
        store_at(&rgba, 96, 96, key, &root);

        let file_size = fs::metadata(normal.join(format!("{key}.png")))
            .unwrap()
            .len();
        evict_at(&normal, file_size * 2);

        assert!(
            normal.join(format!("{key}.png")).exists(),
            "file must not be evicted when under cap"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn lookup_returns_none_for_missing() {
        let root = temp_dir_for("missing");
        let _ = fs::create_dir_all(&root);
        assert!(lookup_at("nonexistent_key_abcdef", &root).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn key_length_and_hex_format() {
        let key = content_key(Path::new("test.png"), 0);
        assert_eq!(key.len(), 16);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
