//! Raster thumbnails and shell icons drawn from Windows Explorer's own
//! thumbnail/icon pipeline via `IShellItemImageFactory` and `SHGetFileInfoW`.
//!
//! Icons fall into two tiers, mirroring Explorer:
//!
//! * **Type icons** — folders, executables and per-extension class icons —
//!   resolve off the system image list *before* a listing paints. [`reload`]
//!   (`src/app/ops.rs`) resolves them in one batched worker round trip and
//!   stores them in [`ThumbCache`], so names and icons land on the same frame.
//!   Path icons are keyed by path plus mtime (folders honor `desktop.ini`,
//!   exes their embedded artwork); class icons are one shared raster per
//!   extension.
//! * **Content thumbnails** — real media previews for images/videos/audio —
//!   are extracted async via `IShellItemImageFactory` and swap in over the
//!   type icon when they land, exactly like Explorer fills previews later.
//!
//! `.lnk` shortcuts resolve their target's icon through the same
//! `IShellItemImageFactory` call, keyed by the resolved icon-source identity so
//! a rebuilt target invalidates on its own. Everything that has no shell
//! artwork (or whose resolution failed) renders the themed glyph.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{AppContext, Context, RenderImage};

use crate::app::Ply;
use crate::listing::{Entry, KindClass, kind_class};

/// One thumbnail size, in device-independent pixels on the larger side.
pub const THUMB_SIZE: u32 = 96;

/// Hard ceiling on cached pixel bytes (~32 MiB of RGBA).
const BUDGET: usize = 32 * 1024 * 1024;

/// Cap on simultaneously locked (on-screen) thumbnails. Mirrors Chromium's
/// `kMaxItemsInWorkingSet` scaled to Ply: a viewport of ~60-80 cells at 96x96x4
/// bytes each uses ~2.8 MiB; 128 is two viewports + generous overscan and keeps
/// locked memory under 5 MiB, well within the 32 MiB byte budget.
const LOCK_CAP: usize = 128;

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

/// A decoded shell icon's system-image-list index plus its BGRA pixels and
/// dimensions. The index is `SHFILEINFOW.iIcon`; many paths share one index,
/// so the decoded raster is shared and the index lets caches dedup on it.
type IndexedPixels = (i32, Vec<u8>, u32, u32);

/// What a batched type-icon resolution should resolve for one listing entry:
/// the shell icon of its real path (a folder honoring `desktop.ini`, or an
/// executable's embedded artwork) or the per-extension class icon.
pub(crate) enum IconTarget {
    Path(PathBuf),
    Class(String),
}

/// The result of a batched type-icon resolution: for each entry that resolved,
/// which system-image-list index its icon is, plus the decoded raster per
/// *distinct* index so the UI uploads each shell icon once and shares it.
pub(crate) struct ListingIcons {
    pub per_entry: Vec<(usize, i32)>,
    pub decoded: Vec<IndexedPixels>,
}

/// Up-front cap on how many listing entries pre-resolve type icons. Beyond it
/// the on-demand probe path covers scrolling rows. Dir/exe lookups are ~25µs
/// each and class lookups dedup by extension, so 512 stays well under a frame.
pub(crate) const TYPE_ICON_BATCH_CAP: usize = 512;

/// How many content extractions (GetImage) may be queued to the thumbnail pool
/// at once. Above this, render-driven probes yield until a completion frees a
/// slot, so a media-heavy folder drains steadily instead of lining up hundreds
/// of jobs that would starve names on the old single worker.
pub(crate) const CONTENT_CAP: usize = 24;

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
    t.and_then(|t| {
        t.duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_nanos() as u64)
    })
    .unwrap_or(0)
}

/// A fixed, environment-independent shell icon shared across all folders or
/// entries that resolve to it. The UI never distinguishes the variants (e.g.
/// an empty vs full Recycle Bin); the worker decides which artwork to use.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum StockIcon {
    RecycleBin,
}

/// Per-window cache of decoded rasters, kept inside [`Ply`] so it is dropped
/// with the window. Two-tier design: an LRU evictable tier bounded by
/// [`BUDGET`] bytes, and a locked tier for on-screen entries that is bounded by
/// [`LOCK_CAP`] count. The render path calls [`ThumbCache::set_working_set`]
/// once per frame with the visible keys; everything outside the working set is
/// evictable and re-decodes on demand.
pub struct ThumbCache {
    map: HashMap<CacheKey, Arc<RenderImage>>,
    order: VecDeque<CacheKey>,
    inflight: HashSet<CacheKey>,
    bytes: usize,
    /// Keys currently visible on screen. Eviction never frees a locked entry;
    /// bounded by [`LOCK_CAP`]. Locked entries do not count toward [`BUDGET`].
    locked_map: HashMap<CacheKey, Arc<RenderImage>>,
    locked_set: HashSet<CacheKey>,
    locked_bytes: usize,
    /// Resolved icon-source stamp per `.lnk` path, so the render probe can key
    /// on the source identity without re-reading the link on every paint.
    lnk_stamp: HashMap<PathBuf, u64>,
    /// Per-extension "class" icons from the system image list, shared across
    /// all files of a type. Extension set is small, so no LRU is needed.
    class_icons: HashMap<String, Arc<RenderImage>>,
    class_inflight: HashSet<String>,
    /// One shared `RenderImage` per shell icon index (folders/classes resolving
    /// to the same index hold one texture, not one per path). Tiny, few, and
    /// canonical, so they are not LRU-accounted or evicted like per-path bytes.
    index_icons: HashMap<i32, Arc<RenderImage>>,
    /// Fixed stock icons (e.g. the Recycle Bin), one per [`StockIcon`].
    stock_icons: HashMap<StockIcon, Arc<RenderImage>>,
    stock_inflight: HashSet<StockIcon>,
    /// Keys whose shell extraction permanently failed; a failed key renders the
    /// themed glyph and is never re-requested.
    failed: HashSet<CacheKey>,
    /// Extensions whose class-icon resolution permanently failed.
    class_failed: HashSet<String>,
    /// Content extractions currently dispatched to the thumbnail pool. Cap
    /// guards opening a folder full of media from lining up hundreds of
    /// GetImage jobs; the drain releases a slot on each completion.
    content_pending: usize,
}

impl ThumbCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            inflight: HashSet::new(),
            bytes: 0,
            locked_map: HashMap::new(),
            locked_set: HashSet::new(),
            locked_bytes: 0,
            lnk_stamp: HashMap::new(),
            class_icons: HashMap::new(),
            class_inflight: HashSet::new(),
            index_icons: HashMap::new(),
            stock_icons: HashMap::new(),
            stock_inflight: HashSet::new(),
            failed: HashSet::new(),
            class_failed: HashSet::new(),
            content_pending: 0,
        }
    }

    pub fn get(&self, key: &CacheKey) -> Option<Arc<RenderImage>> {
        self.locked_map
            .get(key)
            .or_else(|| self.map.get(key))
            .cloned()
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

    pub fn content_pending(&self) -> usize {
        self.content_pending
    }

    fn reserve_content(&mut self) {
        self.content_pending += 1;
    }

    fn release_content(&mut self) {
        self.content_pending = self.content_pending.saturating_sub(1);
    }

    /// Stamp a `.lnk` resolves to; `None` means not resolved yet.
    pub fn lnk_stamp(&self, path: &Path) -> Option<u64> {
        self.lnk_stamp.get(path).copied()
    }

    fn set_lnk_stamp(&mut self, path: &Path, stamp: u64) {
        self.lnk_stamp.insert(path.to_path_buf(), stamp);
    }

    /// Cached class icon for an extension, if resolved.
    pub fn class_icon(&self, ext: &str) -> Option<Arc<RenderImage>> {
        self.class_icons.get(ext).cloned()
    }

    pub fn class_is_inflight(&self, ext: &str) -> bool {
        self.class_inflight.contains(ext)
    }

    pub fn mark_class_inflight(&mut self, ext: String) {
        self.class_inflight.insert(ext);
    }

    /// Cached stock icon (e.g. Recycle Bin), if resolved.
    pub fn stock_icon(&self, stock: StockIcon) -> Option<Arc<RenderImage>> {
        self.stock_icons.get(&stock).cloned()
    }

    pub fn stock_is_inflight(&self, stock: StockIcon) -> bool {
        self.stock_inflight.contains(&stock)
    }

    pub fn mark_stock_inflight(&mut self, stock: StockIcon) {
        self.stock_inflight.insert(stock);
    }

    /// True if extraction for `key` permanently failed, so its slot settles on
    /// the themed glyph instead of re-requesting on every render.
    pub fn is_failed(&self, key: &CacheKey) -> bool {
        self.failed.contains(key)
    }

    pub fn mark_failed(&mut self, key: CacheKey) {
        if self.failed.len() >= 512 {
            self.failed.clear();
        }
        self.failed.insert(key);
    }

    /// True if the per-extension class icon for `ext` permanently failed.
    pub fn class_is_failed(&self, ext: &str) -> bool {
        self.class_failed.contains(ext)
    }

    pub fn insert(&mut self, key: CacheKey, img: Arc<RenderImage>) {
        if self.map.contains_key(&key) {
            return;
        }
        self.push(key, img);
    }

    /// Insert, replacing any existing raster at the same key (used when a
    /// `.lnk` re-extracts to a new icon). If the key is currently locked,
    /// the replacement goes into the locked tier directly.
    fn insert_force(&mut self, key: CacheKey, img: Arc<RenderImage>) {
        if self.locked_set.contains(&key) {
            if let Some(old) = self.locked_map.remove(&key) {
                self.locked_bytes = self.locked_bytes.saturating_sub(byte_size(&old));
            }
            self.locked_bytes = self.locked_bytes.saturating_add(byte_size(&img));
            self.locked_map.insert(key, img);
            return;
        }
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
                    if self.locked_set.contains(&old) {
                        continue;
                    }
                    if let Some(evicted) = self.map.remove(&old) {
                        self.bytes = self.bytes.saturating_sub(byte_size(&evicted));
                    }
                }
                None => break,
            }
        }
    }

    /// Replace the locked (on-screen) working set. Keys in `new_keys` that are
    /// not yet locked are promoted from the evictable tier; keys that were
    /// locked but are absent from `new_keys` are demoted back to the evictable
    /// LRU. If the locked count exceeds [`LOCK_CAP`], excess entries are evicted
    /// from the back of the locked set (oldest lock).
    pub fn set_working_set(&mut self, new_keys: &[CacheKey]) {
        let new: HashSet<CacheKey> = new_keys.iter().cloned().collect();

        // Promote: evictable -> locked.
        for key in new.iter() {
            if self.locked_set.contains(key) {
                continue;
            }
            if let Some(img) = self.map.remove(key) {
                self.bytes = self.bytes.saturating_sub(byte_size(&img));
                self.locked_bytes = self.locked_bytes.saturating_add(byte_size(&img));
                self.locked_map.insert(key.clone(), img);
                self.locked_set.insert(key.clone());
            }
        }

        // Demote: locked -> evictable.
        let to_unlock: Vec<CacheKey> = self
            .locked_set
            .iter()
            .filter(|k| !new.contains(k))
            .cloned()
            .collect();
        for key in &to_unlock {
            if let Some(img) = self.locked_map.remove(key) {
                self.locked_bytes = self.locked_bytes.saturating_sub(byte_size(&img));
                self.bytes = self.bytes.saturating_add(byte_size(&img));
                self.map.insert(key.clone(), img.clone());
                self.order.push_back(key.clone());
            }
            self.locked_set.remove(key);
        }

        // Enforce lock cap: evict oldest locked entries beyond the cap.
        while self.locked_set.len() > LOCK_CAP {
            let oldest = self
                .locked_set
                .iter()
                .min_by_key(|k| self.locked_map.get(*k).map(byte_size))
                .cloned();
            if let Some(key) = oldest {
                if let Some(img) = self.locked_map.remove(&key) {
                    self.locked_bytes = self.locked_bytes.saturating_sub(byte_size(&img));
                    self.bytes = self.bytes.saturating_add(byte_size(&img));
                    self.map.insert(key.clone(), img);
                    self.order.push_back(key.clone());
                }
                self.locked_set.remove(&key);
            } else {
                break;
            }
        }
    }

    /// Dedup a freshly-decoded image at `index`: once an index is seen, later
    /// paths resolving to it reuse the stored texture and the passed `img` is
    /// dropped (no second GPU upload). `None` (no index) passes through.
    fn share_index(&mut self, index: Option<i32>, img: Arc<RenderImage>) -> Arc<RenderImage> {
        match index {
            Some(i) => {
                if let Some(existing) = self.index_icons.get(&i) {
                    return existing.clone();
                }
                if self.index_icons.len() >= 1024 {
                    self.index_icons.clear();
                }
                self.index_icons.insert(i, img.clone());
                img
            }
            None => img,
        }
    }

    /// Fold pre-resolved listing type icons into the caches, one shared texture
    /// per distinct shell index: path-keyed for folders/executables (so the
    /// render probe's `folder_icon` hit is a plain map lookup) and
    /// extension-keyed for everything else (docs, media, generic files). Runs
    /// on the reload path before the listing paints, so icons appear frame 1.
    /// Idempotent: already-present keys are left alone.
    pub(crate) fn apply_listing_icons(&mut self, entries: &[Entry], icons: &ListingIcons) {
        if icons.decoded.is_empty() || icons.per_entry.is_empty() {
            return;
        }
        let mut by_index: HashMap<i32, Arc<RenderImage>> =
            HashMap::with_capacity(icons.decoded.len());
        for (index, bytes, w, h) in &icons.decoded {
            if let Some(img) = to_render_image(bytes.clone(), *w, *h) {
                let shared = self.share_index(Some(*index), img);
                by_index.insert(*index, shared);
            }
        }
        if by_index.is_empty() {
            return;
        }
        for (ordinal, index) in &icons.per_entry {
            let Some(entry) = entries.get(*ordinal) else {
                continue;
            };
            let Some(img) = by_index.get(index) else {
                continue;
            };
            if wants_path_icon(entry) {
                let key = stamped_key(&entry.path, mtime_nanos(entry.modified));
                self.insert(key, img.clone());
            } else {
                let ext = extension_of(entry);
                if !ext.is_empty() {
                    self.class_icons.insert(ext.clone(), img.clone());
                }
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

pub(crate) fn is_lnk(entry: &Entry) -> bool {
    Path::new(&entry.name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
}

/// Executables whose icon is their own embedded/associated artwork, resolved by
/// real path (the way Explorer indexes them) rather than a per-extension class.
fn is_executable_like(entry: &Entry) -> bool {
    matches!(extension_of(entry).as_str(), "exe" | "msi")
}

/// Entries whose icon is the shell icon of their real path: folders (honoring
/// `desktop.ini`) and executables. Resolved in the pre-paint batch.
pub(crate) fn wants_path_icon(entry: &Entry) -> bool {
    entry.is_directory() || is_executable_like(entry)
}

/// Media that deserve a real content thumbnail: the per-extension type icon
/// shows first, then the thumbnail swaps in async, like Explorer fills previews
/// later. MTP paths get the type icon only (no content pipeline for them).
pub(crate) fn wants_content_thumbnail(entry: &Entry) -> bool {
    !is_lnk(entry)
        && !crate::mtp::is_mtp(&entry.path)
        && matches!(
            kind_class(entry),
            KindClass::Image | KindClass::Video | KindClass::Audio
        )
}

/// Entries that show a per-extension class icon, type-final: documents and
/// generic files (archives, unknown types). Media/exe/shortcuts are handled by
/// their own paths, folders keep a real path icon. Files with no extension
/// resolve nothing and keep the themed glyph.
pub(crate) fn wants_class_icon(entry: &Entry) -> bool {
    !is_lnk(entry) && !wants_path_icon(entry) && !wants_content_thumbnail(entry)
}

/// Lowercased extension of an entry's name, or "" if it has none. In-memory
/// only, never touches disk, so it is safe on the render thread.
pub(crate) fn extension_of(entry: &Entry) -> String {
    Path::new(&entry.name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
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

/// Kicks off content extraction for a `.lnk` shortcut (its target icon) or a
/// media entry (a real preview thumbnail) if it is not already cached or in
/// flight, then notifies the window when it lands. Type icons are resolved by
/// the pre-paint batch instead; this is the slow per-file tier.
pub fn request_thumbnail(ply: &Ply, entry: &Entry, cx: &mut Context<Ply>) {
    if !(is_lnk(entry) || wants_content_thumbnail(entry)) {
        return;
    }
    if crate::mtp::is_mtp(&entry.path) {
        return;
    }
    let key = probe_key(ply, entry, cx);
    let cache_entity = ply.thumb_cache();
    let (cached, inflight, failed, over_cap) = {
        let c = cache_entity.read(cx);
        (
            c.get(&key).is_some(),
            c.is_inflight(&key),
            c.is_failed(&key),
            c.content_pending() >= CONTENT_CAP,
        )
    };
    if cached || inflight || failed || over_cap {
        return;
    }
    cache_entity.update(cx, |c, _| {
        c.mark_inflight(key.clone());
        c.reserve_content();
    });

    let path = entry.path.clone();
    let worker_path = path.clone();
    let lnk = is_lnk(entry);
    cx.spawn(async move |this, cx| {
        let got = if lnk {
            cx.background_spawn(async move { request_lnk(worker_path, THUMB_SIZE) })
                .await
        } else {
            cx.background_spawn(async move { request(worker_path, THUMB_SIZE).map(|p| (p, None)) })
                .await
        };
        match got {
            Some(((bytes, w, h), stamp)) => {
                let img = to_render_image(bytes, w, h);
                let _ = this.update(cx, |this, cx| {
                    this.thumb_cache().update(cx, |c, _| {
                        c.unmark_inflight(&key);
                        c.release_content();
                        match (img, stamp) {
                            (Some(image), Some(stamp)) => {
                                c.set_lnk_stamp(&path, stamp);
                                c.insert_force(stamped_key(&path, stamp), image);
                            }
                            (Some(image), None) => {
                                c.insert(key.clone(), image);
                            }
                            _ => {
                                c.mark_failed(key.clone());
                            }
                        }
                    });
                });
            }
            None => {
                let _ = this.update(cx, |this, cx| {
                    this.thumb_cache().update(cx, |c, _| {
                        c.unmark_inflight(&key);
                        c.release_content();
                        // Remember the failure so the probe settles on the
                        // themed glyph instead of re-requesting every render.
                        c.mark_failed(key.clone());
                    });
                });
            }
        }
        let _ = this.update(cx, |this, cx| {
            this.mark_thumbs_dirty();
            this.schedule_thumbs_flush(cx);
        });
    })
    .detach();
}

/// Returns the cached per-extension class icon for an entry if it is ready,
/// otherwise kicks off resolution and returns `None` so the UI keeps the
/// themed glyph until the icon lands.
pub fn class_icon(ply: &Ply, entry: &Entry, cx: &mut Context<Ply>) -> Option<Arc<RenderImage>> {
    let ext = extension_of(entry);
    if ext.is_empty() {
        return None;
    }
    let cache_entity = ply.thumb_cache();
    let (cached, inflight, failed) = {
        let c = cache_entity.read(cx);
        (
            c.class_icon(&ext),
            c.class_is_inflight(&ext),
            c.class_is_failed(&ext),
        )
    };
    if let Some(img) = cached {
        return Some(img);
    }
    if inflight || failed {
        return None;
    }
    cache_entity.update(cx, |c, _| c.mark_class_inflight(ext.clone()));

    let worker_ext = ext.clone();
    cx.spawn(async move |this, cx| {
        let got = cx
            .background_spawn(async move { request_class_icon(worker_ext) })
            .await
            .and_then(|(index, bytes, w, h)| to_render_image(bytes, w, h).map(|img| (index, img)));
        let _ = this.update(cx, |this, cx| {
            this.thumb_cache().update(cx, |c, _| {
                c.class_inflight.remove(&ext);
                if let Some((index, image)) = got {
                    let shared = c.share_index(Some(index), image);
                    c.class_icons.insert(ext, shared);
                } else {
                    if c.class_failed.len() >= 512 {
                        c.class_failed.clear();
                    }
                    c.class_failed.insert(ext);
                }
            });
        });
        let _ = this.update(cx, |this, cx| {
            this.mark_thumbs_dirty();
            this.schedule_thumbs_flush(cx);
        });
    })
    .detach();
    None
}

/// Fire any icon extraction still pending for an entry — a path icon (folder /
/// executable), a media content thumbnail, or a per-extension class icon —
/// without deciding what to render. Idempotent; the prefetch pass calls this so
/// every icon in the window starts resolving at once instead of one row at a
/// time. After the pre-paint batch these mostly no-op on already-cached type
/// icons.
pub(crate) fn ensure_entry_icons(ply: &Ply, entry: &Entry, cx: &mut Context<Ply>) {
    if wants_path_icon(entry) {
        let _ = folder_icon(ply, entry, cx);
    } else if is_lnk(entry) {
        request_thumbnail(ply, entry, cx);
    } else if wants_content_thumbnail(entry) {
        let _ = class_icon(ply, entry, cx);
        request_thumbnail(ply, entry, cx);
    } else {
        let _ = class_icon(ply, entry, cx);
    }
}

/// Whether the icon for an entry is still being extracted right now. The row
/// renderers use this to show a blank placeholder instead of flashing the
/// themed glyph before the real raster lands.
pub(crate) fn entry_icon_pending(ply: &Ply, entry: &Entry, cx: &mut Context<Ply>) -> bool {
    if wants_path_icon(entry) {
        let key = stamped_key(&entry.path, mtime_nanos(entry.modified));
        return ply.thumb_cache().read(cx).is_inflight(&key);
    }
    if is_lnk(entry) || wants_content_thumbnail(entry) {
        let key = probe_key(ply, entry, cx);
        return ply.thumb_cache().read(cx).is_inflight(&key);
    }
    if wants_class_icon(entry) {
        let ext = extension_of(entry);
        return !ext.is_empty() && ply.thumb_cache().read(cx).class_is_inflight(&ext);
    }
    false
}

/// What a slot should render while a shell icon is, or is not, resolved.
/// The UI shows a blank placeholder for `Loading` so the themed fallback never
/// flashes first and then gets replaced by the real raster.
pub(crate) enum IconProbe {
    /// A real shell raster is cached.
    Ready(Arc<RenderImage>),
    /// A real shell raster is being extracted; the slot stays blank meanwhile.
    Loading,
    /// No shell icon applies (or it permanently failed); the themed glyph is
    /// the correct, final rendering.
    Glyph,
}

/// Probe an entry's icon: ensure its extraction is running, then classify it
/// for rendering. The single source of truth for both the prefetch pass and
/// the row renderer, so they can never disagree on a slot.
pub(crate) fn entry_icon_probe(ply: &Ply, entry: &Entry, cx: &mut Context<Ply>) -> IconProbe {
    if wants_path_icon(entry) {
        // Folders and executables: a real path icon, pre-resolved by the batch
        // so this is a plain map hit on freshly painted listings.
        if let Some(img) = folder_icon(ply, entry, cx) {
            return IconProbe::Ready(img);
        }
        if entry_icon_pending(ply, entry, cx) {
            IconProbe::Loading
        } else {
            IconProbe::Glyph
        }
    } else if is_lnk(entry) {
        let cache_entity = ply.thumb_cache();
        let key = probe_key(ply, entry, cx);
        if let Some(img) = cache_entity.read(cx).get(&key) {
            return IconProbe::Ready(img);
        }
        request_thumbnail(ply, entry, cx);
        if entry_icon_pending(ply, entry, cx) {
            IconProbe::Loading
        } else {
            IconProbe::Glyph
        }
    } else if wants_content_thumbnail(entry) {
        let cache_entity = ply.thumb_cache();
        let key = probe_key(ply, entry, cx);
        // A real preview wins over the type icon.
        if let Some(img) = cache_entity.read(cx).get(&key) {
            return IconProbe::Ready(img);
        }
        // Type icon as the placeholder while the thumbnail extracts async.
        if let Some(img) = class_icon(ply, entry, cx) {
            request_thumbnail(ply, entry, cx);
            return IconProbe::Ready(img);
        }
        request_thumbnail(ply, entry, cx);
        if entry_icon_pending(ply, entry, cx) {
            IconProbe::Loading
        } else {
            IconProbe::Glyph
        }
    } else if wants_class_icon(entry) {
        if let Some(img) = class_icon(ply, entry, cx) {
            return IconProbe::Ready(img);
        }
        let ext = extension_of(entry);
        if !ext.is_empty() && ply.thumb_cache().read(cx).class_is_inflight(&ext) {
            IconProbe::Loading
        } else {
            IconProbe::Glyph
        }
    } else {
        IconProbe::Glyph
    }
}

/// Probe a real path (folder / drive root) the same way [`entry_icon_probe`]
/// probes an entry, for the sidebar and Home rows.
pub(crate) fn path_icon_probe(
    ply: &Ply,
    path: &Path,
    stamp: u64,
    cx: &mut Context<Ply>,
) -> IconProbe {
    let cache_entity = ply.thumb_cache();
    let key = stamped_key(path, stamp);
    if let Some(img) = cache_entity.read(cx).get(&key) {
        return IconProbe::Ready(img);
    }
    let _ = path_icon(ply, path, stamp, cx);
    if cache_entity.read(cx).is_inflight(&key) {
        IconProbe::Loading
    } else {
        IconProbe::Glyph
    }
}

/// Probe the Recycle Bin stock icon for the sidebar row.
pub(crate) fn recycle_bin_probe(ply: &Ply, cx: &mut Context<Ply>) -> IconProbe {
    if let Some(img) = recycle_bin_icon(ply, cx) {
        return IconProbe::Ready(img);
    }
    if ply
        .thumb_cache()
        .read(cx)
        .stock_is_inflight(StockIcon::RecycleBin)
    {
        IconProbe::Loading
    } else {
        IconProbe::Glyph
    }
}

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
            let _ = this
                .update(cx, |this, cx| {
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
                        let _ = this.update(cx, |this, cx| {
                            this.mark_thumbs_dirty();
                            this.schedule_thumbs_flush(cx);
                        });
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
    use std::collections::{HashMap, HashSet};
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{Sender, channel};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, LazyLock};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{IconTarget, IndexedPixels, ListingIcons};

    use windows::Win32::Foundation::{FILETIME, SIZE};
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
        DeleteObject, GetDIBits, GetObjectW, HBITMAP, HGDIOBJ, SelectObject,
    };
    use windows::Win32::Storage::EnhancedStorage::{
        PKEY_Author, PKEY_Comment, PKEY_DateCreated, PKEY_Image_Dimensions, PKEY_Title,
    };
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, CoInitializeEx,
        StructuredStorage::{PROPVARIANT, PropVariantToFileTime, PropVariantToString},
    };
    use windows::Win32::System::Variant::PSTIME_FLAGS;
    use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
    use windows::Win32::UI::Shell::PropertiesSystem::{GETPROPERTYSTOREFLAGS, IPropertyStore};
    use windows::Win32::UI::Shell::{
        IShellItem2, IShellItemImageFactory, SHCreateItemFromParsingName, SHFILEINFOW, SHGFI_FLAGS,
        SHGFI_ICONLOCATION, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES, SHGSI_ICON,
        SHGetFileInfoW, SHGetImageList, SHGetStockIconInfo, SHIL_EXTRALARGE, SHSTOCKICONINFO,
        SIID_RECYCLER, SIID_RECYCLERFULL, SIIGBF,
    };
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
    use windows::core::PCWSTR;

    /// Decoded BGRA pixels plus dimensions.
    type ThumbPixels = (Vec<u8>, u32, u32);

    /// A raster plus, for `.lnk`, the resolved icon-source stamp. `None` stamp
    /// means the source identity could not be determined (fall back to mtime).
    type ExtractResult = (ThumbPixels, Option<u64>);

    /// Thumbnail extraction runs on a small STA pool: each worker owns its own
    /// apartment and queue, jobs are spread round-robin. A GetImage that hangs
    /// (e.g. a synced/on-demand cloud file) stalls one thread; the others keep
    /// draining, and names/type icons never share this pool at all.
    const CONTENT_THREADS: usize = 4;

    enum ContentJob {
        /// Extract the shell image for a path (media / executable). For a
        /// `.lnk` also resolve the icon source and fold it into the stamp.
        Extract {
            path: PathBuf,
            size: u32,
            lnk: bool,
            reply: Sender<Option<ExtractResult>>,
        },
    }

    enum ShellJob {
        /// Resolve only the `.lnk` icon-source stamp, without extracting.
        ResolveLnkSource {
            path: PathBuf,
            reply: Sender<Option<u64>>,
        },
        /// Resolve the per-extension "class" icon from the system image list,
        /// keyed by extension alone and shared across all files of that type.
        ResolveClassIcon {
            ext: String,
            reply: Sender<Option<IndexedPixels>>,
        },
        /// Resolve the shell icon for a real path (a folder or drive root),
        /// honoring `desktop.ini` custom icons the way Explorer does.
        ResolvePathIcon {
            path: PathBuf,
            reply: Sender<Option<IndexedPixels>>,
        },
        /// Resolve a fixed stock icon (Recycle Bin for now).
        ResolveStockIcon { reply: Sender<Option<ThumbPixels>> },
        /// Resolve the type icons for a whole listing up front: real-path
        /// icons (folders/executables) plus one class index per extension, all
        /// in a single round trip, decoding each distinct index once.
        ResolveListingTypeIcons {
            targets: Vec<(u32, IconTarget)>,
            reply: Sender<Option<ListingIcons>>,
        },
        /// Read a few shell properties for a real on-disk file (Properties
        /// dialog). Runs on the shell worker.
        ReadProperties {
            path: PathBuf,
            reply: Sender<Vec<(String, String)>>,
        },
    }

    /// One STA thread for the fast shell-index work (type icons, lnk sources,
    /// properties). Kept single: the lookups are ~microsecond rank shell cache
    /// hits, and the shared per-index decode cache lives here with it.
    static SHELL_WORKER: LazyLock<Sender<ShellJob>> = LazyLock::new(|| {
        let (tx, rx) = channel::<ShellJob>();
        thread::spawn(move || {
            // STA: SHGetFileInfo/SHGetStockIconInfo/property stores expect an
            // apartment-threaded caller.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            // Decoded BGRA per shell icon index, so a second folder/class that
            // lands on an already-seen index skips GetIcon+GetDIBits entirely.
            let mut index_cache: HashMap<i32, (Arc<Vec<u8>>, u32, u32)> = HashMap::new();
            for job in rx {
                match job {
                    ShellJob::ResolveLnkSource { path, reply } => {
                        let _ = reply.send(lnk_source_stamp(&path));
                    }
                    ShellJob::ResolveClassIcon { ext, reply } => {
                        let _ = reply.send(class_pixels(&ext, &mut index_cache));
                    }
                    ShellJob::ResolvePathIcon { path, reply } => {
                        let _ = reply.send(path_icon_pixels(&path, &mut index_cache));
                    }
                    ShellJob::ResolveStockIcon { reply } => {
                        let _ = reply.send(recycle_stock_pixels());
                    }
                    ShellJob::ResolveListingTypeIcons { targets, reply } => {
                        let _ = reply.send(listing_type_icons(&targets, &mut index_cache));
                    }
                    ShellJob::ReadProperties { path, reply } => {
                        let _ = reply.send(read_properties_impl(&path));
                    }
                }
            }
        });
        tx
    });

    /// Next worker to hand a content job to. Indexed into [`CONTENT_POOL`].
    static CONTENT_NEXT: AtomicUsize = AtomicUsize::new(0);

    static CONTENT_POOL: LazyLock<Vec<Sender<ContentJob>>> = LazyLock::new(|| {
        (0..CONTENT_THREADS)
            .map(|_| {
                let (tx, rx) = channel::<ContentJob>();
                thread::spawn(move || {
                    // STA: IShellItemImageFactory expects an apartment-threaded
                    // caller; every worker initialises its own apartment.
                    unsafe {
                        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                    }
                    for job in rx {
                        match job {
                            ContentJob::Extract {
                                path,
                                size,
                                lnk,
                                reply,
                            } => {
                                let pixels = extract(&path, size);
                                let stamp = if lnk { lnk_source_stamp(&path) } else { None };
                                let _ = reply.send(pixels.map(|p| (p, stamp)));
                            }
                        }
                    }
                });
                tx
            })
            .collect()
    });

    /// Spreads a content job over the pool. Round-robin load is even enough,
    /// since extractions cost about the same per item.
    fn content_dispatch(job: ContentJob) -> bool {
        let senders = &CONTENT_POOL;
        if senders.is_empty() {
            return false;
        }
        let ix =
            CONTENT_NEXT.fetch_add(1, Ordering::Relaxed) % senders.len();
        senders[ix].send(job).is_ok()
    }

    /// Respects the single-writer rule for the shell worker's decode cache.
    fn shell_dispatch(job: ShellJob) -> bool {
        SHELL_WORKER.send(job).is_ok()
    }

    /// Extract a media/executable raster. Returns `(pixels, None)`.
    pub(super) fn request(path: PathBuf, size: u32) -> Option<(Vec<u8>, u32, u32)> {
        let (tx, rx) = channel();
        if !content_dispatch(ContentJob::Extract {
            path,
            size,
            lnk: false,
            reply: tx,
        }) {
            return None;
        }
        rx.recv().ok().flatten().map(|(p, _)| p)
    }

    /// Extract a `.lnk` icon and its resolved source stamp.
    pub(super) fn request_lnk(path: PathBuf, size: u32) -> Option<ExtractResult> {
        let (tx, rx) = channel();
        if !content_dispatch(ContentJob::Extract {
            path,
            size,
            lnk: true,
            reply: tx,
        }) {
            return None;
        }
        rx.recv().ok().flatten()
    }

    /// Resolve only the `.lnk` icon-source stamp, without extracting.
    pub(super) fn resolve_lnk_source(path: &Path) -> Option<u64> {
        let (tx, rx) = channel();
        if !shell_dispatch(ShellJob::ResolveLnkSource {
            path: path.to_path_buf(),
            reply: tx,
        }) {
            return None;
        }
        rx.recv().ok().flatten()
    }

    /// Resolve the per-extension class icon (shared across all files of that
    /// type), as `None` on any failure so callers fall back to the SVG glyph.
    pub(super) fn request_class_icon(ext: String) -> Option<IndexedPixels> {
        let (tx, rx) = channel();
        if !shell_dispatch(ShellJob::ResolveClassIcon { ext, reply: tx }) {
            return None;
        }
        rx.recv().ok().flatten()
    }

    /// Resolve the shell icon for a real path (folder or drive root), as `None`
    /// on any failure so callers fall back to the themed glyph.
    pub(super) fn request_path_icon_pixels(path: PathBuf) -> Option<IndexedPixels> {
        let (tx, rx) = channel();
        if !shell_dispatch(ShellJob::ResolvePathIcon { path, reply: tx }) {
            return None;
        }
        rx.recv().ok().flatten()
    }

    /// Resolve a fixed stock icon (Recycle Bin), as `None` on any failure.
    pub(super) fn request_stock_icon_pixels() -> Option<ThumbPixels> {
        let (tx, rx) = channel();
        if !shell_dispatch(ShellJob::ResolveStockIcon { reply: tx }) {
            return None;
        }
        rx.recv().ok().flatten()
    }

    /// Resolve a batch of listing type icons in one shell worker round trip.
    pub(super) fn request_listing_type_icons(
        targets: Vec<(u32, IconTarget)>,
    ) -> Option<ListingIcons> {
        let (tx, rx) = channel();
        if !shell_dispatch(ShellJob::ResolveListingTypeIcons { targets, reply: tx }) {
            return None;
        }
        rx.recv().ok().flatten()
    }

    /// Dispatch a shell property read to the shell worker and block for the rows.
    pub(super) fn request_properties(path: &Path) -> Vec<(String, String)> {
        let (tx, rx) = channel();
        if !shell_dispatch(ShellJob::ReadProperties {
            path: path.to_path_buf(),
            reply: tx,
        }) {
            return Vec::new();
        }
        rx.recv().unwrap_or_default()
    }

    /// The shell reads themselves, run on the STA worker. Returns non-empty
    /// `(label, value)` rows; callers guard out MTP/portable paths.
    fn read_properties_impl(path: &Path) -> Vec<(String, String)> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let item: IShellItem2 =
            match unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) } {
                Ok(i) => i,
                Err(_) => return Vec::new(),
            };
        let store: IPropertyStore =
            match unsafe { item.GetPropertyStore(GETPROPERTYSTOREFLAGS::default()) } {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };

        let mut rows: Vec<(String, String)> = Vec::new();
        for (label, key) in [
            ("Author", &PKEY_Author),
            ("Title", &PKEY_Title),
            ("Comment", &PKEY_Comment),
            ("Dimensions", &PKEY_Image_Dimensions),
        ] {
            if let Ok(pv) = unsafe { store.GetValue(key) } {
                let s = pv_string(&pv);
                if !s.is_empty() {
                    rows.push((label.to_string(), s));
                }
            }
        }
        if let Ok(pv) = unsafe { store.GetValue(&PKEY_DateCreated) }
            && let Ok(ft) = unsafe { PropVariantToFileTime(&pv, PSTIME_FLAGS(0)) }
            && let Some(st) = ft_to_systemtime(ft)
        {
            rows.push((
                "Created".to_string(),
                crate::listing::format_mtime(Some(st), chrono::Local::now()),
            ));
        }
        rows
    }

    /// A `PROPVARIANT` as a plain string (strings and most scalar types), or
    /// empty when it cannot be read as one.
    fn pv_string(pv: &PROPVARIANT) -> String {
        let mut buf = [0u16; 1024];
        if unsafe { PropVariantToString(pv, &mut buf) }.is_ok() {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..len])
        } else {
            String::new()
        }
    }

    /// A `FILETIME` (100ns since 1601) as a Unix `SystemTime`, if it is a sane
    /// date.
    fn ft_to_systemtime(ft: FILETIME) -> Option<SystemTime> {
        let t: u64 = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
        if t < 116444736000000000 {
            return None;
        }
        SystemTime::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(
            (t - 116444736000000000) / 10_000_000,
        ))
    }

    /// Resolve the type icons for a user listing in one pass: each target maps
    /// to a system-image-list index (deduped by index when decoding), so the
    /// UI uploads every distinct shell icon once and shares it. Entries that
    /// fail to resolve are simply omitted.
    fn listing_type_icons(
        targets: &[(u32, IconTarget)],
        cache: &mut HashMap<i32, (Arc<Vec<u8>>, u32, u32)>,
    ) -> Option<ListingIcons> {
        let mut per_entry: Vec<(usize, i32)> = Vec::with_capacity(targets.len());
        let mut indices: Vec<i32> = Vec::with_capacity(targets.len());
        for (ordinal, target) in targets {
            let index = match target {
                IconTarget::Path(path) => path_icon_index(path),
                IconTarget::Class(ext) => class_icon_index(ext),
            };
            if let Some(index) = index {
                per_entry.push((*ordinal as usize, index));
                indices.push(index);
            }
        }
        let mut decoded: Vec<IndexedPixels> = Vec::new();
        let mut seen: HashSet<i32> = HashSet::new();
        for index in indices {
            if seen.insert(index)
                && let Some((bytes, w, h)) = index_icon_at(index, cache)
            {
                decoded.push((index, bytes.as_ref().clone(), w, h));
            }
        }
        Some(ListingIcons { per_entry, decoded })
    }

    /// The system-image-list index of the class icon for an extension, via a
    /// made-up name so no real file is needed. `SHGFI_SYSICONINDEX` finds the
    /// index the shell uses for that extension; the same artwork Explorer shows.
    fn class_icon_index(ext: &str) -> Option<i32> {
        if ext.is_empty() {
            return None;
        }
        let name: Vec<u16> = format!("C:\\classicon.{ext}\0").encode_utf16().collect();
        let mut sfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
        // Nonzero return means the call succeeded; the index is in `sfi.iIcon`.
        // The DWORD_PTR return value is not reliably the index on 64-bit.
        let ok = unsafe {
            SHGetFileInfoW(
                PCWSTR(name.as_ptr()),
                FILE_ATTRIBUTE_NORMAL,
                Some(&mut sfi as *mut _ as *mut _),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_FLAGS(SHGFI_SYSICONINDEX.0 | SHGFI_USEFILEATTRIBUTES.0),
            )
        };
        if ok == 0 {
            return None;
        }
        Some(sfi.iIcon)
    }

    /// The "class" (per-file-type) icon from the system image list, resolved
    /// for a made-up name so no real file is needed: `SHGetFileInfoW` finds the
    /// index (shared by the batch), then the extralarge 48px icon is pulled for
    /// that index from the system image list and decoded.
    fn class_pixels(
        ext: &str,
        cache: &mut HashMap<i32, (Arc<Vec<u8>>, u32, u32)>,
    ) -> Option<IndexedPixels> {
        let index = class_icon_index(ext)?;
        let (bytes, w, h) = index_icon_at(index, cache)?;
        Some((index, bytes.as_ref().clone(), w, h))
    }

    /// Decoded BGRA for a system-image-list icon, cached per index so a second
    /// hit reuses the raster. `Some` when the icon decodes.
    fn index_icon_at(
        index: i32,
        cache: &mut HashMap<i32, (Arc<Vec<u8>>, u32, u32)>,
    ) -> Option<(Arc<Vec<u8>>, u32, u32)> {
        if let Some(hit) = cache.get(&index) {
            return Some(hit.clone());
        }
        let (bytes, w, h) = list_icon_at(index)?;
        if cache.len() >= 1024 {
            cache.clear();
        }
        let bytes = Arc::new(bytes);
        cache.insert(index, (bytes.clone(), w, h));
        Some((bytes, w, h))
    }

    /// Pull the extralarge (48px) system-image-list icon at `index` and decode
    /// it to BGRA, destroying the `HICON` afterwards. Shared by the class icon
    /// and the real-path (folder/drive) icon paths.
    fn list_icon_at(index: i32) -> Option<ThumbPixels> {
        let list: IImageList =
            unsafe { SHGetImageList::<IImageList>(SHIL_EXTRALARGE as i32) }.ok()?;
        let hicon: HICON = unsafe { list.GetIcon(index, ILD_TRANSPARENT.0) }.ok()?;
        let res = hicon_to_pixels(hicon);
        unsafe {
            let _ = DestroyIcon(hicon);
        }
        res
    }

    /// The system-image-list index for a real path, which honors `desktop.ini`
    /// custom icons the way Explorer shows them. Shared by the single-path icon
    /// request and the listing batch.
    fn path_icon_index(path: &Path) -> Option<i32> {
        if crate::mtp::is_mtp(path) {
            return None;
        }
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut sfi: SHFILEINFOW = unsafe { std::mem::zeroed() };
        // Nonzero return means the call succeeded; the index is in `sfi.iIcon`.
        // The DWORD_PTR return value is not reliably the index on 64-bit.
        let ok = unsafe {
            SHGetFileInfoW(
                PCWSTR(wide.as_ptr()),
                FILE_ATTRIBUTE_NORMAL,
                Some(&mut sfi as *mut _ as *mut _),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_FLAGS(SHGFI_SYSICONINDEX.0),
            )
        };
        if ok == 0 {
            return None;
        }
        Some(sfi.iIcon)
    }

    /// The shell icon for a real path (a folder or drive root), which honors
    /// `desktop.ini` custom icons the way Explorer shows them: the shell
    /// resolves the path's own index, and the extralarge 48px icon is pulled
    /// from the system image list.
    fn path_icon_pixels(
        path: &Path,
        cache: &mut HashMap<i32, (Arc<Vec<u8>>, u32, u32)>,
    ) -> Option<IndexedPixels> {
        let index = path_icon_index(path)?;
        let (bytes, w, h) = index_icon_at(index, cache)?;
        Some((index, bytes.as_ref().clone(), w, h))
    }

    /// The stock Recycle Bin icon. Which artwork (empty vs full) is decided by
    /// scanning whether the bin holds anything; the caller never cares.
    fn recycle_stock_pixels() -> Option<ThumbPixels> {
        let id = if recycle_bin_has_items() {
            SIID_RECYCLERFULL
        } else {
            SIID_RECYCLER
        };
        let mut info: SHSTOCKICONINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHSTOCKICONINFO>() as u32;
        if unsafe { SHGetStockIconInfo(id, SHGSI_ICON, &mut info) }.is_err() {
            return None;
        }
        let res = hicon_to_pixels(info.hIcon);
        unsafe {
            let _ = DestroyIcon(info.hIcon);
        }
        res
    }

    /// Heuristic for whether any Recycle Bin holds items. For each present drive
    /// letter, look at `X:\$Recycle.Bin`; each of its subdirectories is one user
    /// SID, and a bin with anything in it has an entry there (even an empty bin
    /// keeps its SID folder, so the check goes one level deeper).
    fn recycle_bin_has_items() -> bool {
        let mask = crate::volumes::logical_drives_mask();
        for letter in 0..26u8 {
            if mask & (1 << letter) == 0 {
                continue;
            }
            let drive = format!("{}:\\", (b'A' + letter) as char);
            let bin = format!("{drive}$Recycle.Bin");
            let Ok(users) = std::fs::read_dir(&bin) else {
                continue;
            };
            for dir in users.flatten() {
                if let Ok(entries) = std::fs::read_dir(dir.path())
                    && entries.into_iter().next().is_some()
                {
                    return true;
                }
            }
        }
        false
    }

    /// Draw a `HICON` into BA pixels via its color bitmap, reusing the same
    /// BMP decode as `extract`.
    fn hicon_to_pixels(hicon: HICON) -> Option<ThumbPixels> {
        let mut info: ICONINFO = unsafe { std::mem::zeroed() };
        if unsafe { GetIconInfo(hicon, &mut info) }.is_err() {
            return None;
        }
        let bits = if info.hbmColor.0.is_null() {
            // Monochrome icons have only a mask; fall back to the mask.
            info.hbmMask
        } else {
            info.hbmColor
        };
        let res = hbitmap_to_bgra(bits);
        unsafe {
            if !info.hbmColor.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
            }
            if !info.hbmMask.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
            }
        }
        res
    }
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
                &mut bmi as *mut _,
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
            for px in out.as_chunks_mut::<4>().0 {
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

#[cfg(not(windows))]
fn request_class_icon(_ext: String) -> Option<IndexedPixels> {
    None
}

#[cfg(not(windows))]
fn request_path_icon_pixels(_path: PathBuf) -> Option<IndexedPixels> {
    None
}

#[cfg(not(windows))]
fn request_stock_icon_pixels() -> Option<(Vec<u8>, u32, u32)> {
    None
}

#[cfg(windows)]
use backend::{request, request_class_icon, request_lnk, resolve_lnk_source};

#[cfg(not(windows))]
use {request, request_class_icon, request_lnk, resolve_lnk_source};

/// Warm the shell icon worker once, at startup, off the hot path. The STA
/// worker thread and its apartment spin up lazily on the first icon request;
/// paying that plus a cold shell lookup at launch keeps the first listing's
/// icons from stalling a frame or two behind their names.
pub fn warm_shell() {
    #[cfg(windows)]
    {
        let dir = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
        let _ = backend::request_path_icon_pixels(dir);
        // Pre-warm the shell's per-type association lookups (the cold ~1ms
        // first hit per extension) so the first listing's batch is warm.
        for ext in [
            "zip", "pdf", "txt", "jpg", "png", "mp3", "mkv", "docx", "rar", "gif",
        ] {
            let _ = backend::request_class_icon(ext.into());
        }
    }
    #[cfg(not(windows))]
    {}
}

/// Resolve the *type* icons for listing entries in one batched worker round
/// trip, so the icons are cached before the listing paints: real-path icons
/// for folders/executables (`SHGetFileInfoW` on the actual path) and one
/// per-extension class index for everything else. Content thumbnails are not
/// part of the batch — media get their type icon here and swap in a preview
/// later. Runs on the background executor in `reload`.
pub(crate) fn resolve_listing_type_icons(entries: &[Entry]) -> Option<ListingIcons> {
    #[cfg(windows)]
    {
        let mut targets: Vec<(u32, IconTarget)> = Vec::new();
        for (ordinal, entry) in entries.iter().take(TYPE_ICON_BATCH_CAP).enumerate() {
            if crate::mtp::is_mtp(&entry.path) || is_lnk(entry) {
                continue;
            }
            if wants_path_icon(entry) {
                targets.push((ordinal as u32, IconTarget::Path(entry.path.clone())));
            } else if wants_class_icon(entry) || wants_content_thumbnail(entry) {
                let ext = extension_of(entry);
                if !ext.is_empty() {
                    targets.push((ordinal as u32, IconTarget::Class(ext)));
                }
            }
        }
        if targets.is_empty() {
            return None;
        }
        backend::request_listing_type_icons(targets)
    }
    #[cfg(not(windows))]
    {
        let _ = (entries, TYPE_ICON_BATCH_CAP);
        None
    }
}

/// Read a few common shell properties for a real on-disk file, returning
/// `(label, value)` rows for the Properties dialog. Runs on the STA worker;
/// no-ops on non-Windows.
pub fn read_properties(path: &Path) -> Vec<(String, String)> {
    #[cfg(windows)]
    {
        backend::request_properties(path)
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// The shell icon for an arbitrary real path — a folder or a drive root like
/// `C:\` — honoring `desktop.ini` custom icons the way Explorer shows them.
/// Returns `Some` when the raster is already cached; otherwise kicks off an
/// async extraction and returns `None` so the caller falls back to an SVG
/// glyph until it lands.
pub fn path_icon(
    ply: &Ply,
    path: &Path,
    stamp: u64,
    cx: &mut Context<Ply>,
) -> Option<Arc<RenderImage>> {
    let key = stamped_key(path, stamp);
    let cache_entity = ply.thumb_cache();
    let (cached, inflight, failed) = {
        let c = cache_entity.read(cx);
        (c.get(&key), c.is_inflight(&key), c.is_failed(&key))
    };
    if let Some(img) = cached {
        return Some(img);
    }
    if inflight || failed {
        return None;
    }
    if crate::mtp::is_mtp(path) {
        // Never queue an MTP request; the shell read would hang on it. Leave
        // the key unmarked so a later non-in-flight call can still probe.
        return None;
    }
    cache_entity.update(cx, |c, _| c.mark_inflight(key.clone()));

    let path = path.to_path_buf();
    cx.spawn(async move |this, cx| {
        let got = cx
            .background_spawn(async move {
                #[cfg(windows)]
                {
                    backend::request_path_icon_pixels(path.clone())
                }
                #[cfg(not(windows))]
                {
                    None
                }
            })
            .await
            .and_then(|(index, bytes, w, h)| to_render_image(bytes, w, h).map(|img| (index, img)));
        let _ = this.update(cx, |this, cx| {
            this.thumb_cache().update(cx, |c, _| {
                c.unmark_inflight(&key);
                if let Some((index, image)) = got {
                    let shared = c.share_index(Some(index), image);
                    c.insert(key.clone(), shared);
                } else {
                    c.mark_failed(key);
                }
            });
        });
        let _ = this.update(cx, |this, cx| {
            this.mark_thumbs_dirty();
            this.schedule_thumbs_flush(cx);
        });
    })
    .detach();
    None
}

/// The shell icon for a folder entry, keyed by the folder's own mtime so a
/// changed `desktop.ini` re-extracts the icon. Thin wrapper over [`path_icon`].
pub fn folder_icon(ply: &Ply, entry: &Entry, cx: &mut Context<Ply>) -> Option<Arc<RenderImage>> {
    path_icon(ply, &entry.path, mtime_nanos(entry.modified), cx)
}

/// The stock Recycle Bin icon, cached in the small per-cache stock map. Whether
/// it shows empty or full is decided in the worker; the UI never cares.
pub fn recycle_bin_icon(ply: &Ply, cx: &mut Context<Ply>) -> Option<Arc<RenderImage>> {
    let stock = StockIcon::RecycleBin;
    let cache_entity = ply.thumb_cache();
    let (cached, inflight) = {
        let c = cache_entity.read(cx);
        (c.stock_icon(stock), c.stock_is_inflight(stock))
    };
    if let Some(img) = cached {
        return Some(img);
    }
    if inflight {
        return None;
    }
    cache_entity.update(cx, |c, _| c.mark_stock_inflight(stock));

    cx.spawn(async move |this, cx| {
        let got = cx
            .background_spawn(async move {
                #[cfg(windows)]
                {
                    backend::request_stock_icon_pixels()
                }
                #[cfg(not(windows))]
                {
                    None
                }
            })
            .await
            .and_then(|(bytes, w, h)| to_render_image(bytes, w, h));
        let _ = this.update(cx, |this, cx| {
            this.thumb_cache().update(cx, |c, _| {
                c.stock_inflight.remove(&stock);
                if let Some(img) = got {
                    c.stock_icons.insert(stock, img);
                }
            });
        });
        let _ = this.update(cx, |this, cx| {
            this.mark_thumbs_dirty();
            this.schedule_thumbs_flush(cx);
        });
    })
    .detach();
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::{Entry, EntryKind};

    fn entry(name: &str) -> Entry {
        Entry {
            path: PathBuf::from(name),
            name: name.into(),
            kind: EntryKind::File,
            size: 0,
            modified: None,
            hidden: false,
        }
    }

    fn dir(name: &str) -> Entry {
        Entry {
            path: PathBuf::from(name),
            name: name.into(),
            kind: EntryKind::Directory,
            size: 0,
            modified: None,
            hidden: false,
        }
    }

    #[test]
    fn extension_of_lowercases_and_handles_missing() {
        assert_eq!(extension_of(&entry("report.PDF")), "pdf");
        assert_eq!(extension_of(&entry("archive.ZIP")), "zip");
        assert_eq!(extension_of(&entry("noext")), "");
    }

    #[test]
    fn class_icon_covers_documents_and_generic_files() {
        assert!(wants_class_icon(&entry("data.bin")), "unknown extension");
        assert!(
            wants_class_icon(&entry("archive.zip")),
            "archives have no thumbnail"
        );
        assert!(
            wants_class_icon(&entry("plain.txt")),
            "documents are type-final"
        );
        assert!(
            !wants_class_icon(&entry("song.mp3")),
            "audio gets a content thumbnail"
        );
        assert!(
            !wants_class_icon(&entry("photo.jpg")),
            "image gets a content thumbnail"
        );
        assert!(!wants_class_icon(&entry("setup.exe")), "exe is a path icon");
        assert!(!wants_class_icon(&entry("App.lnk")), "lnk is its own tier");
    }

    #[test]
    fn path_icon_covers_folders_and_executables_only() {
        assert!(
            wants_path_icon(&dir("Folder")),
            "folders resolve a real path icon"
        );
        assert!(wants_path_icon(&entry("setup.exe")));
        assert!(wants_path_icon(&entry("installer.msi")));
        assert!(
            !wants_path_icon(&entry("App.lnk")),
            "lnk is not a batch path"
        );
        assert!(!wants_path_icon(&entry("photo.jpg")));
        assert!(!wants_path_icon(&entry("doc.txt")));
    }

    #[test]
    fn content_thumbnail_covers_local_media_only() {
        assert!(wants_content_thumbnail(&entry("photo.jpg")));
        assert!(wants_content_thumbnail(&entry("song.mp3")));
        assert!(wants_content_thumbnail(&entry("clip.mkv")));
        assert!(
            !wants_content_thumbnail(&entry("report.pdf")),
            "docs are type-final"
        );
        assert!(
            !wants_content_thumbnail(&entry("setup.exe")),
            "exe icon is resolved by path"
        );
        assert!(!wants_content_thumbnail(&entry("App.lnk")));
        assert!(!wants_content_thumbnail(&entry("data.bin")));
    }

    #[test]
    #[cfg(windows)]
    fn batch_resolves_and_dedupes_listing_type_icons() {
        // A batch over a temp dir, a subfolder and class-icon file names must
        // resolve real indices for all of them, decode each distinct index once
        // (no duplicate rasters), and be stable across repeats.
        let dir = std::env::temp_dir();
        let sub = dir.join("ply_batch_test");
        let _ = std::fs::create_dir_all(&sub);
        let entries = vec![
            Entry {
                path: dir.clone(),
                name: "Temp".to_string(),
                kind: EntryKind::Directory,
                size: 0,
                modified: None,
                hidden: false,
            },
            Entry {
                path: sub.clone(),
                name: "sub".to_string(),
                kind: EntryKind::Directory,
                size: 0,
                modified: None,
                hidden: false,
            },
            Entry {
                path: sub.join("b.pdf"),
                name: "b.pdf".to_string(),
                kind: EntryKind::File,
                size: 0,
                modified: None,
                hidden: false,
            },
            Entry {
                path: sub.join("b.jar"),
                name: "b.jar".to_string(),
                kind: EntryKind::File,
                size: 0,
                modified: None,
                hidden: false,
            },
        ];
        let icons = resolve_listing_type_icons(&entries).expect("batch must resolve");
        assert!(
            !icons.per_entry.is_empty(),
            "dirs and class files must resolve"
        );
        assert!(!icons.decoded.is_empty(), "distinct indices must decode");
        let decoded: std::collections::HashSet<i32> =
            icons.decoded.iter().map(|(i, _, _, _)| *i).collect();
        for (_, index) in &icons.per_entry {
            assert!(
                decoded.contains(index),
                "every resolved entry must have its index decoded once"
            );
        }
        assert!(
            icons.decoded.len() <= icons.per_entry.len(),
            "no more distinct rasters than entries"
        );
        let again = resolve_listing_type_icons(&entries).expect("batch must resolve again");
        assert_eq!(
            icons.per_entry, again.per_entry,
            "the same listing resolves the same icons"
        );
        let _ = std::fs::remove_dir_all(&sub);
    }

    #[test]
    #[cfg(windows)]
    fn shell_icon_pipeline_decodes_on_windows() {
        // Regression: SHGetFileInfoW's DWORD_PTR return value is not reliably
        // the icon index on 64-bit; the index must come from `sfi.iIcon`.
        // Folders, per-type class icons and the stock recycle-bin icon must all
        // decode to real pixels through the STA worker.
        for dir in [std::env::temp_dir(), dirs::home_dir().unwrap()] {
            let got = super::backend::request_path_icon_pixels(dir.clone());
            assert!(
                got.is_some(),
                "path icon must decode for {:?}",
                dir.display()
            );
        }
        let cls = super::backend::request_class_icon("zip".into());
        assert!(cls.is_some(), "class icon must decode for .zip");
        let stock = super::backend::request_stock_icon_pixels();
        assert!(stock.is_some(), "stock recycle-bin icon must decode");
    }

    #[test]
    #[cfg(windows)]
    fn path_icon_index_is_stable_across_repeats() {
        // The same shell index must resolve to the same decoder each time,
        // so re-requesting a folder is free after the first decode.
        let dir = std::env::temp_dir();
        let (i1, pixels, w, h) =
            super::backend::request_path_icon_pixels(dir.clone()).expect("path icon must decode");
        let (i2, _, _, _) =
            super::backend::request_path_icon_pixels(dir).expect("path icon must decode again");
        assert_eq!(i1, i2, "same path must resolve to the same index");
        assert!(w >= 16 && h >= 16 && pixels.len() >= (w * h * 4) as usize);
    }

    #[test]
    fn folder_stamp_is_zero_when_no_mtime_and_distinct_when_set() {
        // `folder_icon` keys by the entry's own mtime, so a changed
        // `desktop.ini` re-extracts the icon. Unknown mtime stamps as 0.
        assert_eq!(mtime_nanos(None), 0);
        let known = Some(
            std::time::UNIX_EPOCH
                .checked_add(std::time::Duration::from_secs(1_700_000_000))
                .unwrap(),
        );
        let nanos = mtime_nanos(known);
        assert!(nanos > 0, "a known mtime must not stamp as 0");
        let p = Path::new("C:\\some-folder");
        assert!(
            stamped_key(p, 0) != stamped_key(p, nanos),
            "different stamps must key different cache entries"
        );
    }

    // --- Working-set lock tests ---

    /// Create a 96x96 RGBA thumbnail (36,864 bytes) keyed at `path` with a
    /// given stamp.  The pixel content is distinct per `path` so byte_size
    /// stays predictable.
    fn make_thumb(cache: &mut ThumbCache, path: &str, stamp: u64) -> CacheKey {
        let key = stamped_key(Path::new(path), stamp);
        let pixels: Vec<u8> = (0..36864)
            .map(|i| (i ^ path.len() as usize) as u8)
            .collect();
        let img = to_render_image(pixels, 96, 96).unwrap();
        cache.insert(key.clone(), img);
        key
    }

    #[test]
    fn locked_entries_are_never_evicted() {
        let mut c = ThumbCache::new();
        let k0 = make_thumb(&mut c, "a.png", 1);
        let k1 = make_thumb(&mut c, "b.png", 2);

        // Lock k0: it must survive any eviction pressure.
        c.set_working_set(&[k0.clone()]);
        assert!(c.locked_set.contains(&k0));

        // Flood with enough entries to blow past the budget.
        // 32 MiB / 36,864 bytes per thumb = ~868 entries needed.
        for i in 0..1200 {
            make_thumb(&mut c, &format!("x{i}.png"), i + 10);
        }

        assert!(
            c.get(&k0).is_some(),
            "locked entry must survive eviction"
        );
        assert!(
            c.get(&k1).is_none(),
            "unlocked entry should be evicted under pressure"
        );
    }

    #[test]
    fn set_working_set_locks_and_unlocks() {
        let mut c = ThumbCache::new();
        let k0 = make_thumb(&mut c, "a.png", 1);
        let k1 = make_thumb(&mut c, "b.png", 2);

        c.set_working_set(&[k0.clone(), k1.clone()]);
        assert!(c.locked_set.contains(&k0));
        assert!(c.locked_set.contains(&k1));

        // Scroll: k0 leaves the viewport, k1 stays, k2 enters.
        let k2 = make_thumb(&mut c, "c.png", 3);
        c.set_working_set(&[k1.clone(), k2.clone()]);
        assert!(!c.locked_set.contains(&k0), "k0 should be unlocked");
        assert!(c.locked_set.contains(&k1));
        assert!(c.locked_set.contains(&k2));
    }

    #[test]
    fn unlocked_entry_returns_to_evictable_lru() {
        let mut c = ThumbCache::new();
        let k0 = make_thumb(&mut c, "a.png", 1);

        // Lock, then unlock.
        c.set_working_set(&[k0.clone()]);
        assert!(c.locked_set.contains(&k0));
        c.set_working_set(&[]);
        assert!(!c.locked_set.contains(&k0));

        // k0 should be in the evictable tier and retrievable.
        assert!(c.get(&k0).is_some());
        assert!(c.map.contains_key(&k0));
        assert!(c.order.contains(&k0));
    }

    #[test]
    fn byte_budget_enforced_on_evictable_only() {
        let mut c = ThumbCache::new();
        let k0 = make_thumb(&mut c, "a.png", 1);

        // Lock k0. Its bytes leave the evictable budget.
        c.set_working_set(&[k0.clone()]);
        assert_eq!(c.bytes, 0);

        // Insert many evictable entries: eviction must keep bytes under BUDGET.
        // Need >868 entries (32 MiB / 36,864 bytes per thumb) to trigger eviction.
        for i in 0..1200 {
            make_thumb(&mut c, &format!("x{i}.png"), i + 10);
        }
        assert!(
            c.bytes <= BUDGET,
            "evictable bytes {} must not exceed BUDGET {BUDGET}",
            c.bytes
        );
        // k0 is still accessible despite being outside the budget.
        assert!(c.get(&k0).is_some());
    }

    #[test]
    fn insert_force_updates_locked_entry_in_place() {
        let mut c = ThumbCache::new();
        let k0 = make_thumb(&mut c, "a.png", 1);
        c.set_working_set(&[k0.clone()]);

        // Re-extract while locked: the new image replaces the old in-place.
        let new_pixels: Vec<u8> = vec![42u8; 36864];
        let new_img = to_render_image(new_pixels, 96, 96).unwrap();
        c.insert_force(k0.clone(), new_img);

        assert!(c.locked_set.contains(&k0), "still locked");
        assert!(c.get(&k0).is_some(), "still accessible");
        // The old evictable map must not have a stale copy.
        assert!(!c.map.contains_key(&k0));
    }

    #[test]
    fn lock_cap_evicts_excess_locked_entries() {
        let mut c = ThumbCache::new();
        let mut keys = Vec::new();
        // Insert LOCK_CAP + 10 entries.
        for i in 0..(LOCK_CAP + 10) {
            keys.push(make_thumb(&mut c, &format!("t{i}.png"), i as u64));
        }

        // Lock all of them. The excess should spill into the evictable tier.
        c.set_working_set(&keys);
        assert!(
            c.locked_set.len() <= LOCK_CAP,
            "locked set {} must not exceed LOCK_CAP {LOCK_CAP}",
            c.locked_set.len()
        );

        // The spilled keys must still be accessible (just evictable).
        for k in &keys {
            assert!(c.get(k).is_some(), "spilled key must still be in cache");
        }
    }

    #[test]
    fn re_insert_after_unlock_restores_to_lru() {
        let mut c = ThumbCache::new();
        let k0 = make_thumb(&mut c, "a.png", 1);

        // Lock, unlock, then re-insert (simulates re-decode on demand).
        c.set_working_set(&[k0.clone()]);
        c.set_working_set(&[]);
        let new_img = to_render_image(vec![99u8; 36864], 96, 96).unwrap();
        c.insert(k0.clone(), new_img);

        assert!(c.get(&k0).is_some());
        assert!(!c.locked_set.contains(&k0));
        assert!(c.map.contains_key(&k0));
    }

    #[test]
    fn empty_working_set_unlocks_everything() {
        let mut c = ThumbCache::new();
        let k0 = make_thumb(&mut c, "a.png", 1);
        let k1 = make_thumb(&mut c, "b.png", 2);

        c.set_working_set(&[k0.clone(), k1.clone()]);
        assert_eq!(c.locked_set.len(), 2);

        c.set_working_set(&[]);
        assert_eq!(c.locked_set.len(), 0);
        assert!(c.get(&k0).is_some());
        assert!(c.get(&k1).is_some());
    }

    #[test]
    fn idempotent_set_working_set_is_a_noop() {
        let mut c = ThumbCache::new();
        let k0 = make_thumb(&mut c, "a.png", 1);
        let k1 = make_thumb(&mut c, "b.png", 2);

        c.set_working_set(&[k0.clone(), k1.clone()]);
        let bytes_before = c.bytes;
        let locked_before = c.locked_bytes;

        c.set_working_set(&[k0.clone(), k1.clone()]);
        assert_eq!(c.bytes, bytes_before);
        assert_eq!(c.locked_bytes, locked_before);
        assert_eq!(c.locked_set.len(), 2);
    }
}
