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

/// Hard ceiling for the on-disk cache (~48 MiB).
pub const DISK_CACHE_MAX_BYTES: u64 = 48 * 1024 * 1024;

/// Hard ceiling on cached file count. Bounds readdir/stat work and keeps a
/// small-file-heavy cache from sprawling in count while staying under bytes.
pub const DISK_CACHE_MAX_FILES: u64 = 4000;

/// Per-file size the launch fast path assumes: count times this ceiling
/// bounds the total without stating every file. Stored PNGs are fitted
/// thumbnails (at most 96px), so encoded bytes stay far below this.
const SCAN_FAST_PATH_FILE_CEIL: u64 = 64 * 1024;

/// Compute a stable content key from a source path and mtime stamp.
/// The same (path, stamp) always produces the same hex string; a different
/// stamp produces a different key (edited file re-extracts).
pub fn content_key(path: &Path, stamp: u64) -> String {
    let mut h = Fnv1a::new();
    h.write_bytes(path.as_os_str().to_string_lossy().as_bytes());
    h.write_u64(stamp);
    format!("{:016x}", h.finish_u64())
}

/// [`content_key`] at an explicit tile size. Full-size keys hash exactly like
/// `content_key`, so PNGs written before the list/grid split still hit; small
/// tiles mix the size in and live next to the full file. Disk bytes stay PNG
/// either way, so the format never changes.
pub fn content_key_sized(path: &Path, stamp: u64, size: u32) -> String {
    if size == crate::thumbs::THUMB_SIZE {
        return content_key(path, stamp);
    }
    let mut h = Fnv1a::new();
    h.write_bytes(path.as_os_str().to_string_lossy().as_bytes());
    h.write_u64(stamp);
    h.write_u64(size as u64);
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
    let pixels = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
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
            for px in raw[..pixels / 4 * 3].as_chunks::<3>().0 {
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
            for px in raw[..pixels / 2].as_chunks::<2>().0 {
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
/// `max_bytes` or the file count exceeds [`DISK_CACHE_MAX_FILES`].
/// Best-effort; never panics.
pub fn evict(max_bytes: u64) {
    evict_with_limits(max_bytes, DISK_CACHE_MAX_FILES);
}

/// [`evict`] with explicit caps, so tests and the background pass can tune
/// each bound independently.
pub fn evict_with_limits(max_bytes: u64, max_files: u64) {
    let Some(dir) = cache_dir() else { return };
    evict_at_limited(&dir.join("normal"), max_bytes, max_files, usize::MAX);
}

/// Evict oldest-first, removing at most `max_remove` files per call. Returns
/// the files removed; call again until it returns 0 to drain fully. Lets the
/// app bound background eviction work per idle slice instead of deleting
/// thousands of files in one launch pass.
#[allow(dead_code)] // wired up by the background idle pass next; unit-covered.
pub fn evict_incremental(max_bytes: u64, max_files: u64, max_remove: usize) -> usize {
    let Some(dir) = cache_dir() else { return 0 };
    evict_at_limited(&dir.join("normal"), max_bytes, max_files, max_remove)
}

/// Evict files in `normal_dir` whose aggregate size exceeds `max_bytes` or
/// whose count exceeds `max_files`, oldest-first by mtime. Fast path: when a
/// plain readdir (no stat, no sort) already proves the cache is small, return
/// without stating every file, so a warm launch does one directory read and
/// nothing else. Removing entries whose source file no longer exists is a
/// future extension; for now the cache is only size- and count-bounded.
/// Returns the files removed.
fn evict_at_limited(normal_dir: &Path, max_bytes: u64, max_files: u64, max_remove: usize) -> usize {
    let Ok(entries) = fs::read_dir(normal_dir) else {
        return 0;
    };
    let names: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();

    // Fast path: count times a per-file ceiling every stored PNG stays under
    // proves both caps hold without a single stat call.
    if names.len() as u64 <= max_files
        && (names.len() as u64).saturating_mul(SCAN_FAST_PATH_FILE_CEIL) <= max_bytes
    {
        return 0;
    }

    // Collect (path, size, mtime) for every cached file.
    let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::with_capacity(names.len());
    let mut total: u64 = 0;
    for path in &names {
        let meta = match fs::metadata(path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let size = meta.len();
        let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
        total += size;
        files.push((path.clone(), size, mtime));
    }

    // Sort oldest-first for LRU-style eviction.
    files.sort_by_key(|a| a.2);

    // Evict oldest entries until under both caps (or the per-call budget).
    let mut removed = 0usize;
    for (path, size, _) in &files {
        if total <= max_bytes && names.len() as u64 - removed as u64 <= max_files {
            break;
        }
        if removed >= max_remove {
            break;
        }
        let _ = fs::remove_file(path);
        total = total.saturating_sub(*size);
        removed += 1;
    }
    removed
}

/// Back-compat seam for the existing tests: full eviction at the default file
/// cap, removing everything over budget in one call.
#[cfg(test)]
fn evict_at(normal_dir: &Path, max_bytes: u64) {
    evict_at_limited(normal_dir, max_bytes, DISK_CACHE_MAX_FILES, usize::MAX);
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
    fn lookup_magic_invalid_returns_none() {
        // A `.png` whose bytes lack the signature, one that is only the
        // signature, and a JPEG-style header all read as a miss, so the caller
        // heals by re-extracting instead of panicking. The valid PNG under the
        // same key proves the miss is about the bytes, not the lookup path.
        let root = temp_dir_for("magic");
        let normal = root.join("normal");
        let _ = fs::create_dir_all(&normal);
        fs::write(normal.join("bad0.png"), b"not a png").unwrap();
        fs::write(normal.join("bad1.png"), PNG_MAGIC).unwrap();
        fs::write(normal.join("bad2.png"), [0xFFu8, 0xD8, 0xFF, 0xE0]).unwrap();
        assert!(lookup_at("bad0", &root).is_none());
        assert!(lookup_at("bad1", &root).is_none());
        assert!(lookup_at("bad2", &root).is_none());
        let rgba = vec![9u8; 2 * 2 * 4];
        store_at(&rgba, 2, 2, "good", &root);
        assert_eq!(lookup_at("good", &root).unwrap().0, rgba);
        let _ = fs::remove_dir_all(&root);
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

        // Pin distinct mtimes so "oldest-first" is deterministic. Both files
        // are written in the same test, so without this the tie rides on
        // readdir order, which is name-ordered on NTFS but hash-ordered on
        // some other filesystems.
        let t0 = UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        fs::File::options()
            .write(true)
            .open(&old_path)
            .unwrap()
            .set_modified(t0)
            .unwrap();
        fs::File::options()
            .write(true)
            .open(&new_path)
            .unwrap()
            .set_modified(t0 + std::time::Duration::from_secs(10))
            .unwrap();

        // Cap at just under two files so exactly one must be evicted.
        let cap = file_size * 2 - 1;
        evict_at(&normal, cap);

        assert!(!old_path.exists(), "the oldest file must be evicted first");
        assert!(new_path.exists(), "the newest file must survive");
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

    #[test]
    fn sized_keys_split_small_tiles_but_keep_full_tiles_stable() {
        let p = Path::new("C:\\pics\\a.jpg");
        assert_eq!(
            content_key_sized(p, 7, 96),
            content_key(p, 7),
            "PNGs written before the split must still hit"
        );
        assert_ne!(
            content_key_sized(p, 7, 48),
            content_key(p, 7),
            "small tiles live next to the full file"
        );
        assert_eq!(content_key_sized(p, 7, 48), content_key_sized(p, 7, 48));
    }

    #[test]
    fn file_count_cap_evicts_down_to_the_cap() {
        let root = temp_dir_for("filecap");
        let normal = root.join("normal");
        let _ = fs::create_dir_all(&normal);
        let rgba: Vec<u8> = vec![64u8; 8 * 8 * 4];
        for i in 0..4 {
            store_at(&rgba, 8, 8, &format!("key{i:016x}"), &root);
        }
        // Bytes cap huge: only the count cap binds. Exactly two must remain.
        evict_at_limited(&normal, u64::MAX, 2, usize::MAX);
        let remaining: Vec<_> = fs::read_dir(&normal).unwrap().flatten().collect();
        assert_eq!(remaining.len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn incremental_evict_bounds_work_per_call() {
        let root = temp_dir_for("incremental");
        let normal = root.join("normal");
        let _ = fs::create_dir_all(&normal);
        let rgba: Vec<u8> = vec![64u8; 8 * 8 * 4];
        for i in 0..4 {
            store_at(&rgba, 8, 8, &format!("inc{i:016x}"), &root);
        }
        // One removal per call until the count cap holds.
        assert_eq!(evict_at_limited(&normal, u64::MAX, 1, 1), 1);
        assert_eq!(evict_at_limited(&normal, u64::MAX, 1, 1), 1);
        assert_eq!(evict_at_limited(&normal, u64::MAX, 1, 1), 1);
        assert_eq!(evict_at_limited(&normal, u64::MAX, 1, 0), 0);
        assert_eq!(evict_at_limited(&normal, u64::MAX, 1, usize::MAX), 0);
        let remaining: Vec<_> = fs::read_dir(&normal).unwrap().flatten().collect();
        assert_eq!(remaining.len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn warm_cache_under_cap_keeps_everything() {
        let root = temp_dir_for("warmevict");
        let normal = root.join("normal");
        let _ = fs::create_dir_all(&normal);
        let rgba: Vec<u8> = vec![64u8; 8 * 8 * 4];
        for i in 0..3 {
            store_at(&rgba, 8, 8, &format!("warm{i:016x}"), &root);
        }
        // Three tiny files sit far under both caps: the fast path must not
        // touch them.
        evict_at_limited(
            &normal,
            DISK_CACHE_MAX_BYTES,
            DISK_CACHE_MAX_FILES,
            usize::MAX,
        );
        let remaining: Vec<_> = fs::read_dir(&normal).unwrap().flatten().collect();
        assert_eq!(remaining.len(), 3);
        let _ = fs::remove_dir_all(&root);
    }
}
