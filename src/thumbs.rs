//! Raster thumbnails for media entries and per-file shell icons (executables,
//! shortcuts), drawn from Windows Explorer's own thumbnail/icon pipeline via
//! `IShellItemImageFactory`.
//!
//! A single size is extracted per path (clamped by Explorer to the larger
//! side) and the UI scales it down; this keeps the working set small and the
//! extraction fast. Results are cached in [`ThumbCache`], keyed by path plus a
//! stamp: the own-file mtime for media/executables, or the resolved icon-source
//! identity for `.lnk` shortcuts so a rebuilt icon invalidates on its own.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{AppContext, Context, RenderImage};

use crate::app::Ply;
use crate::listing::{Entry, KindClass, is_executable_or_shortcut, kind_class};

/// One thumbnail size, in device-independent pixels on the larger side.
pub const THUMB_SIZE: u32 = 96;

/// Hard ceiling on cached pixel bytes (~32 MiB of RGBA).
const BUDGET: usize = 32 * 1024 * 1024;

/// Identity of a cached raster: a path plus the "stamp" it was derived from.
/// For media and executables the stamp is the own-file mtime, so editing the
/// file re-extracts it. For `.lnk` shortcuts the stamp is the resolved icon
/// source's identity (path + mtime + index), so rebuilding the target's icon
/// invalidates the cached icon even though the link itself never moved.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    path: PathBuf,
    stamp: u64,
}

/// Default key: a path plus its own mtime (media, executables, fallback).
pub fn cache_key(path: &Path, mtime: Option<SystemTime>) -> CacheKey {
    stamped_key(path, mtime_nanos(mtime))
}

/// Key from an explicit stamp, for `.lnk` icons whose validity is keyed by the
/// icon source rather than the link's own mtime.
pub(crate) fn stamped_key(path: &Path, stamp: u64) -> CacheKey {
    CacheKey {
        path: path.to_path_buf(),
        stamp,
    }
}

fn mtime_nanos(t: Option<SystemTime>) -> u64 {
    t.and_then(|t| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_nanos() as u64))
        .unwrap_or(0)
}

/// Per-window cache of decoded rasters, kept inside [`Ply`] so it is dropped
/// with the window. LRU-evicts oldest entries past [`BUDGET`].
pub struct ThumbCache {
    map: HashMap<CacheKey, Arc<RenderImage>>,
    order: VecDeque<CacheKey>,
    inflight: HashSet<CacheKey>,
    bytes: usize,
    /// Resolved icon-source stamp per `.lnk` path, so the render probe can key
    /// on the source identity without re-reading the link on every paint.
    lnk_stamp: HashMap<PathBuf, u64>,
}

impl ThumbCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            inflight: HashSet::new(),
            bytes: 0,
            lnk_stamp: HashMap::new(),
        }
    }

    pub fn get(&self, key: &CacheKey) -> Option<Arc<RenderImage>> {
        self.map.get(key).cloned()
    }

    pub fn is_inflight(&self, key: &CacheKey) -> bool {
        self.inflight.contains(key)
    }

    /// True if any in-flight raster for `path` exists, regardless of stamp.
    /// Used as a path-level guard so refresh and the render-driven request do
    /// not double up on the same `.lnk`.
    pub fn inflight_path(&self, path: &Path) -> bool {
        self.inflight.iter().any(|k| k.path == path)
    }

    pub fn mark_inflight(&mut self, key: CacheKey) {
        self.inflight.insert(key);
    }

    fn unmark_inflight(&mut self, key: &CacheKey) {
        self.inflight.remove(key);
    }

    /// Stamp a `.lnk` resolves to; `None` means not resolved yet.
    pub fn lnk_stamp(&self, path: &Path) -> Option<u64> {
        self.lnk_stamp.get(path).copied()
    }

    fn set_lnk_stamp(&mut self, path: &Path, stamp: u64) {
        self.lnk_stamp.insert(path.to_path_buf(), stamp);
    }

    pub fn insert(&mut self, key: CacheKey, img: Arc<RenderImage>) {
        if self.map.contains_key(&key) {
            return;
        }
        self.push(key, img);
    }

    /// Insert, replacing any existing raster at the same key (used when a
    /// `.lnk` re-extracts to a new icon).
    fn insert_force(&mut self, key: CacheKey, img: Arc<RenderImage>) {
        if let Some(old) = self.map.remove(&key) {
            self.bytes = self.bytes.saturating_sub(byte_size(&old));
        }
        self.push(key, img);
    }

    fn push(&mut self, key: CacheKey, img: Arc<RenderImage>) {
        let sz = byte_size(&img);
        self.bytes = self.bytes.saturating_add(sz);
        self.map.insert(key.clone(), img);
        self.order.push_back(key);
        while self.bytes > BUDGET {
            match self.order.pop_front() {
                Some(old) => {
                    if let Some(evicted) = self.map.remove(&old) {
                        self.bytes = self.bytes.saturating_sub(byte_size(&evicted));
                    }
                }
                None => break,
            }
        }
    }
}

fn byte_size(img: &Arc<RenderImage>) -> usize {
    let s = img.size(0);
    let w = s.width.0 as usize;
    let h = s.height.0 as usize;
    w.saturating_mul(h).max(1) * 4
}

/// Entries whose per-file image should be extracted: media thumbnails, plus
/// executables and shortcuts with their own shell icon.
fn wants_shell_icon(entry: &Entry) -> bool {
    matches!(kind_class(entry), KindClass::Image | KindClass::Video)
        || is_executable_or_shortcut(entry)
}

fn is_lnk(entry: &Entry) -> bool {
    Path::new(&entry.name).extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
}

/// The cache key to probe for a visible entry. For `.lnk` shortcuts this uses
/// the resolved icon-source stamp (path + source mtime + index) so the probe
/// follows the source's identity; absent a resolution it falls back to the
/// link's own mtime. Everything else keys by its own mtime.
pub(crate) fn probe_key(ply: &Ply, entry: &Entry, cx: &mut Context<Ply>) -> CacheKey {
    let cache = ply.thumb_cache().read(cx);
    if is_lnk(entry) {
        match cache.lnk_stamp(&entry.path) {
            Some(stamp) => stamped_key(&entry.path, stamp),
            None => cache_key(&entry.path, entry.modified),
        }
    } else {
        cache_key(&entry.path, entry.modified)
    }
}

/// Kicks off extraction for an image, video, executable, or shortcut entry if
/// it is not already cached or in flight, then notifies the window when it
/// lands. Everything else falls back to the SVG icon in the UI.
///
/// Executables and `.lnk` shortcuts resolve their own shell icon (embedded
/// exe icon / shortcut target icon) through the same `IShellItemImageFactory`
/// call as media, so e.g. an app shows its real icon rather than a generic
/// file glyph.
pub fn request_thumbnail(ply: &Ply, entry: &Entry, cx: &mut Context<Ply>) {
    if !wants_shell_icon(entry) {
        return;
    }
    if crate::mtp::is_mtp(&entry.path) {
        return;
    }
    let key = probe_key(ply, entry, cx);
    let cache_entity = ply.thumb_cache();
    let (cached, inflight) = {
        let c = cache_entity.read(cx);
        (c.get(&key).is_some(), c.is_inflight(&key))
    };
    if cached || inflight {
        return;
    }
    cache_entity.update(cx, |c, _| c.mark_inflight(key.clone()));

    let path = entry.path.clone();
    let worker_path = path.clone();
    let lnk = is_lnk(entry);
    cx.spawn(async move |this, cx| {
        let got = if lnk {
            cx.background_spawn(async move { request_lnk(worker_path, THUMB_SIZE) })
                .await
        } else {
            cx.background_spawn(
                async move { request(worker_path, THUMB_SIZE).map(|p| (p, None)) },
            )
            .await
        };
        match got {
            Some(((bytes, w, h), stamp)) => {
                let img = to_render_image(bytes, w, h);
                let _ = this.update(cx, |this, cx| {
                    this.thumb_cache().update(cx, |c, _| {
                        c.unmark_inflight(&key);
                        match (img, stamp) {
                            (Some(image), Some(stamp)) => {
                                c.set_lnk_stamp(&path, stamp);
                                c.insert_force(stamped_key(&path, stamp), image);
                            }
                            (Some(image), None) => {
                                c.insert(key.clone(), image);
                            }
                            _ => {}
                        }
                    });
                });
            }
            None => {
                let _ = this.update(cx, |this, cx| {
                    this.thumb_cache().update(cx, |c, _| c.unmark_inflight(&key));
                });
            }
        }
        let _ = this.update(cx, |_, cx| cx.notify());
    })
    .detach();
}

/// Re-resolve the icon-source stamp for the given `.lnk` paths and re-extract
/// any whose source changed. Called periodically so a rebuilt target or a
/// replaced icon file refreshes without the link's own mtime budging.
pub fn refresh_lnk(paths: &[PathBuf], cx: &mut Context<Ply>) {
    for path in paths {
        if crate::mtp::is_mtp(path) {
            continue;
        }
        let path = path.clone();
        cx.spawn(async move |this, cx| {
            // Resolving the source is cheap (read the link, stat the icon file);
            // running it on the background executor keeps it off the paint path.
            let worker_path = path.clone();
            let Some(stamp) = cx
                .background_spawn(async move { resolve_lnk_source(&worker_path) })
                .await
            else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let cache = this.thumb_cache();
                let (current, busy) = {
                    let c = cache.read(cx);
                    (c.lnk_stamp(&path), c.inflight_path(&path))
                };
                // Unchanged source, or a request already in flight: skip.
                if busy || current == Some(stamp) {
                    return;
                }
                let store_key = stamped_key(&path, stamp);
                cache.update(cx, |c, _| {
                    c.mark_inflight(store_key.clone());
                    c.set_lnk_stamp(&path, stamp);
                });
                let path = path.clone();
                cx.spawn(async move |this, cx| {
                    let got = cx
                        .background_spawn(async move { request_lnk(path, THUMB_SIZE) })
                        .await;
                    let img = got.and_then(|((bytes, w, h), _)| to_render_image(bytes, w, h));
                    let _ = this.update(cx, |this, cx| {
                        this.thumb_cache().update(cx, |c, _| {
                            c.unmark_inflight(&store_key);
                            if let Some(img) = img {
                                c.insert_force(store_key.clone(), img);
                            }
                        });
                    });
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            })
            .ok();
        })
        .detach();
    }
}

fn to_render_image(bytes: Vec<u8>, w: u32, h: u32) -> Option<Arc<RenderImage>> {
    let buf = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(w, h, bytes)?;
    let frame = image::Frame::new(buf);
    Some(Arc::new(RenderImage::new(vec![frame])))
}

#[cfg(windows)]
mod backend {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{channel, Sender};
    use std::sync::LazyLock;
    use std::thread;
    use std::time::UNIX_EPOCH;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CreateCompatibleDC, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, GetDIBits, GetObjectW, SelectObject, HBITMAP, HGDIOBJ,
    };
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SHFILEINFOW, SHGetFileInfoW, SIIGBF,
        SHGFI_ICONLOCATION,
    };
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;

    /// Decoded BGRA pixels plus dimensions.
    type ThumbPixels = (Vec<u8>, u32, u32);

    /// A raster plus, for `.lnk`, the resolved icon-source stamp. `None` stamp
    /// means the source identity could not be determined (fall back to mtime).
    type ExtractResult = (ThumbPixels, Option<u64>);

    enum Job {
        /// Extract the shell image for a path (media / executable). For a
        /// `.lnk` also resolve the icon source and fold it into the stamp.
        Extract {
            path: PathBuf,
            size: u32,
            lnk: bool,
            reply: Sender<Option<ExtractResult>>,
        },
        /// Resolve only the `.lnk` icon-source stamp, without extracting.
        ResolveLnkSource {
            path: PathBuf,
            reply: Sender<Option<u64>>,
        },
    }

    static WORKER: LazyLock<Sender<Job>> = LazyLock::new(|| {
        let (tx, rx) = channel::<Job>();
        thread::spawn(move || {
            // STA: IShellItemImageFactory/SHGetFileInfo expect an
            // apartment-threaded caller.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            while let Ok(job) = rx.recv() {
                match job {
                    Job::Extract {
                        path,
                        size,
                        lnk,
                        reply,
                    } => {
                        let pixels = extract(&path, size);
                        let stamp = if lnk { lnk_source_stamp(&path) } else { None };
                        let _ = reply.send(pixels.map(|p| (p, stamp)));
                    }
                    Job::ResolveLnkSource { path, reply } => {
                        let _ = reply.send(lnk_source_stamp(&path));
                    }
                }
            }
        });
        tx
    });

    /// Extract a media/executable raster. Returns `(pixels, None)`.
    pub(super) fn request(path: PathBuf, size: u32) -> Option<(Vec<u8>, u32, u32)> {
        let (tx, rx) = channel();
        if WORKER
            .send(Job::Extract {
                path,
                size,
                lnk: false,
                reply: tx,
            })
            .is_err()
        {
            return None;
        }
        rx.recv().ok().flatten().map(|(p, _)| p)
    }

    /// Extract a `.lnk` icon and its resolved source stamp.
    pub(super) fn request_lnk(path: PathBuf, size: u32) -> Option<ExtractResult> {
        let (tx, rx) = channel();
        if WORKER
            .send(Job::Extract {
                path,
                size,
                lnk: true,
                reply: tx,
            })
            .is_err()
        {
            return None;
        }
        rx.recv().ok().flatten()
    }

    /// Resolve only the `.lnk` icon-source stamp, without extracting.
    pub(super) fn resolve_lnk_source(path: &Path) -> Option<u64> {
        let (tx, rx) = channel();
        if WORKER
            .send(Job::ResolveLnkSource {
                path: path.to_path_buf(),
                reply: tx,
            })
            .is_err()
        {
            return None;
        }
        rx.recv().ok().flatten()
    }

    /// Identity of a `.lnk`'s icon source: the path plus mtime of the file it
    /// draws its icon from, folded with the icon index. This is what moves when
    /// a target is rebuilt or an icon file is replaced, and is the same
    /// (source file, index) tuple the shell itself keys its icon cache by.
    fn lnk_source_stamp(path: &Path) -> Option<u64> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut sfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            SHGetFileInfoW(
                PCWSTR(wide.as_ptr()),
                FILE_ATTRIBUTE_NORMAL,
                Some(&mut sfi as *mut _ as *mut _),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICONLOCATION,
            )
        };
        if ok == 0 {
            return None;
        }
        // szDisplayName holds the icon source file ("" means no custom icon,
        // i.e. the link's target), iIcon the index into it.
        let source = wide_to_string(&sfi.szDisplayName);
        let index = sfi.iIcon;
        let src_path = if source.is_empty() {
            // No explicit icon: the icon comes from the link target. Resolving
            // the target here would need IShellLink; instead key on the link
            // path mtime, which still beats keying only on nothing.
            path.to_path_buf()
        } else {
            PathBuf::from(source)
        };
        let mtime = std::fs::metadata(&src_path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        // Mix source path (as a stable u64 hash) with its mtime and index so a
        // rebuilt icon file is a different key.
        let path_hash = hash_bytes(src_path.as_os_str().to_string_lossy().as_bytes());
        Some(path_hash ^ mtime.rotate_left(1) ^ (index as u64).rotate_left(17))
    }

    fn wide_to_string(u: &[u16]) -> String {
        // A SHFILEINFOW buffer up to the terminator, converted for a path check.
        let end = u.iter().position(|&c| c == 0).unwrap_or(u.len());
        String::from_utf16_lossy(&u[..end])
    }

    fn hash_bytes(b: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for &byte in b {
            h ^= byte as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    fn extract(path: &Path, size: u32) -> Option<ThumbPixels> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let item: IShellItemImageFactory =
            unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }.ok()?;
        let hbitmap: HBITMAP = unsafe {
            item.GetImage(
                SIZE {
                    cx: size as i32,
                    cy: size as i32,
                },
                SIIGBF(0),
            )
        }
        .ok()?;
        let res = hbitmap_to_bgra(hbitmap);
        unsafe {
            let _ = DeleteObject(HGDIOBJ(hbitmap.0));
        }
        res
    }

    // Windows hands back a bottom-up 32bpp DIB; flip to top-down and normalise
    // alpha (GetDIBits often reports a fully-zero alpha byte) so GPUI's BGRA
    // frame is correct.
    fn hbitmap_to_bgra(hbitmap: HBITMAP) -> Option<ThumbPixels> {
        let mut bm: BITMAP = unsafe { std::mem::zeroed() };
        if unsafe {
            GetObjectW(
                HGDIOBJ(hbitmap.0),
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut bm as *mut _ as *mut core::ffi::c_void),
            )
        } == 0
        {
            return None;
        }
        let w = bm.bmWidth as u32;
        let h = bm.bmHeight as u32;
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.0.is_null() {
            return None;
        }
        let prev = unsafe { SelectObject(dc, HGDIOBJ(hbitmap.0)) };
        let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w as i32;
        bmi.bmiHeader.biHeight = h as i32;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;
        let mut buf: Vec<u8> = vec![0u8; (w as usize) * (h as usize) * 4];
        let lines = unsafe {
            GetDIBits(
                dc,
                hbitmap,
                0,
                h,
                Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
                &mut bmi as *mut _ as *mut BITMAPINFO,
                DIB_RGB_COLORS,
            )
        };
        unsafe {
            let _ = SelectObject(dc, prev);
            let _ = DeleteDC(dc);
        }
        if lines == 0 {
            return None;
        }

        let stride = (w as usize) * 4;
        let mut out = vec![0u8; buf.len()];
        for y in 0..(h as usize) {
            let src = &buf[y * stride..(y + 1) * stride];
            let dst = &mut out[(h as usize - 1 - y) * stride..(h as usize - y) * stride];
            dst.copy_from_slice(src);
        }
        let all_zero_alpha = out.iter().step_by(4).skip(1).all(|a| *a == 0);
        if all_zero_alpha {
            for px in out.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }
        Some((out, w, h))
    }
}

#[cfg(not(windows))]
fn request(_path: PathBuf, _size: u32) -> Option<(Vec<u8>, u32, u32)> {
    None
}

#[cfg(not(windows))]
fn request_lnk(_path: PathBuf, _size: u32) -> Option<((Vec<u8>, u32, u32), Option<u64>)> {
    None
}

#[cfg(not(windows))]
fn resolve_lnk_source(_path: &Path) -> Option<u64> {
    None
}

#[cfg(windows)]
use backend::{request, request_lnk, resolve_lnk_source};

#[cfg(not(windows))]
use {request, request_lnk, resolve_lnk_source};
