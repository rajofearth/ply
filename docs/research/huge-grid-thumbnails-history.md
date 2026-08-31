# Large grids with async thumbnails: how the problem was solved

Observed problem in Ply: a non-virtualized grid renders every cell of a 15,744-item
folder every frame, and every completed thumbnail fires a notification that rebuilds
the whole grid. That never quiesces (CPU pinned at ~100% for 120+s, working set
+~900 MB). This is a research note: what past systems did about the same two problems
(scale of items, and a herd of async image completions), grounded in primary sources.

## 1. Never materialize all rows: the virtualized-list lineage

Every major platform arrived at the same shape: the view knows a count and a
per-index pull accessor; it materializes only the visible slice, plus a small
overscan, and recycles the concrete row objects on scroll.

- **Win32 list-view owner data (`LVS_OWNERDATA`, Common Controls, late Win95 era).**
  The control stores no per-item data; the owner sets item count (sizes the
  scrollbar) and answers `LVN_GETDISPINFO` per row at draw time. `LVN_ODCACHEHINT`
  hands the owner the predicted paint range so it can prefetch that slice. This is
  exactly the "count + pull" contract.
  https://learn.microsoft.com/en-us/windows/win32/controls/list-view-controls-overview ,
  https://learn.microsoft.com/en-us/windows/win32/controls/lvn-getdispinfo ,
  https://learn.microsoft.com/en-us/cpp/mfc/virtual-list-controls (MFC: "only a subset
  of data items in memory at any one time")
- **Windows Explorer.** Folder views bind the shell namespace (`IShellFolder`) to an
  owner-data list view; only the enum pointers and the cached system image list exist
  in the GUI process. Modern "Items View" internals are undocumented (closed source);
  the reusable contract is the documented Win32 one, reimplemented by WINE/ReactOS
  (https://gitlab.winehq.org/wine/wine tree `programs/explorer/`,
  https://github.com/reactos/reactos tree `base/shell/explorer/`).
- **macOS.** `NSTableViewDataSource` (per-row pull, AppKit since 2001) with view
  recycling (`makeViewWithIdentifier:`); legacy `NSCollectionView` (2007) created a
  view for *every* item and was rebuilt on the `UICollectionView` model at 10.11
  (2015) to recycle (`makeItem(withIdentifier:for:)`); SwiftUI `LazyVGrid`/`LazyVStack`
  (2020) create items only as needed, and the docs explicitly warn the regular
  `Grid`/`VStack` materializes everything.
  https://developer.apple.com/documentation/appkit/nstableviewdatasource ,
  https://developer.apple.com/documentation/appkit/nscollectionview ,
  https://developer.apple.com/documentation/swiftui/lazyvgrid
- **GTK.** `GtkTreeView` (2002) paints visible rows through cell renderers; fixed-height
  mode (2.4) skips layout for off-screen rows. GTK 4 (2021) `GtkListView`/`GtkGridView`
  render from a `GListModel` through a factory and *recycle* listitems, keeping them
  only for on-screen items. `GtkDirectoryList` fills asynchronously from
  `g_file_enumerate_children_async()`.
  https://docs.gtk.org/gtk3/class.TreeView.html ,
  https://docs.gtk.org/gtk4/section-list-widget.html ,
  https://docs.gtk.org/gtk4/class.ListView.html
- **Qt.** Model/view (2005): `QAbstractItemModel` exposes count + `data(index, role)`;
  the view paints only the visible slice via the delegate. `QFileSystemModel` (used by
  Dolphin) holds no items and loads asynchronously. Qt 6.9 `updateThreshold` (default
  200) avoids full repaints on huge `dataChanged` bursts.
  https://doc.qt.io/qt-6/model-view-programming.html , https://doc.qt.io/qt-6/qfilesystemmodel.html

The frameworks side repeats it: React windowing (react-window fixed grids =
`itemCount` + `itemSize` + overscan), TanStack Virtual (`overscan` default 1),
RecyclerView view recycling + `RecycledViewPool`, Flutter `GridView.builder`
(builder only for visible children), `UICollectionViewDataSourcePrefetching`.
https://github.com/bvaughn/react-window ,
https://tanstack.com/virtual/latest/docs/api/virtualizer ,
https://developer.android.com/develop/ui/views/layout/recyclerview ,
https://api.flutter.dev/flutter/widgets/GridView/GridView.builder.html ,
https://developer.apple.com/documentation/uikit/uicollectionviewdatasourceprefetching

## 2. Async thumbnail generation: cache-first, visible-first, cancel, coalesce

- **Windows shell.** Two-tier cache access on `IThumbnailCache`: read cache-first on a
  high-priority thread (`WTS_INCACHEONLY`/`WTS_FASTEXTRACT`, which decodes only the
  embedded EXIF thumb), extract on a lower-priority pass (`WTS_EXTRACT`). Extraction is
  banned from the UI thread; `WTS_E_EXTRACTIONTIMEDOUT` aborts providers that overrun.
  Since Vista, `IThumbnailProvider` runs out-of-process in a COM surrogate
  (`dllhost.exe`) so a bad extractor can't freeze Explorer. `thumcache*.db` is a
  per-size, oldest-first-evicted disk cache with a hard cap (observed ~349 MB for
  `thumbcache_96.db`; exact number not in official docs). Dedup rule: check cache, only
  extract on miss or newer source.
  https://learn.microsoft.com/en-us/windows/win32/api/thumbcache/nn-thumbcache-ithumbnailcache ,
  https://learn.microsoft.com/en-us/previous-versions/bb776853(v=vs.85) ,
  https://devblogs.microsoft.com/oldnewthing/20090212-00/?p=19173 ,
  https://learn.microsoft.com/en-us/answers/questions/2799029/
- **freedesktop Thumbnail Managing Standard.** `$XDG_CACHE_HOME/thumbnails/{normal,large,x-large,xx-large}/`
  keyed `<md5(uri)>.png`, size caps 128/256/512/1024, a `fail/` dir so a bad file is
  never retried, generated once per file and downscaled for display.
  https://specifications.freedesktop.org/thumbnail/latest/directory.html ,
  https://specifications.freedesktop.org/thumbnail/latest/creation.html
- **Tumbler (Xfce).** A foreground scheduler that is LIFO (most-recently-requested
  first, which serves the current viewport) and runs one worker thread; clients queue
  a batch of URIs, cached ones are filtered out first, and a `ready` signal is emitted
  per thumbnail. Hard file-size throttle via `MaxFileSize`.
  https://docs.xfce.org/xfce/tumbler/available_plugins ,
  https://github.com/xfce-mirror/thunar/blob/master/thunar/thunar-thumbnailer.c
- **Nautilus (GNOME).** On scroll, schedules `nautilus_thumbnail_prioritize` on a
  single idle handle — explicitly "to rate limit and to avoid delaying scrolling" —
  which walks the visible range in reverse (top first) and prio-pushes those rows.
  Per-file async load with a cancellable.
  https://lists.gnome.org/archives/nautilus-list/2007-September/msg00025.html ,
  https://bugzilla.gnome.org/show_bug.cgi?id=104224
- **KIO / Dolphin (KDE).** `KFilePreviewGenerator` creates visible-item previews
  first (`orderItems()`). On scroll it *pauses all pending previews* and resumes only
  after a 200 ms quiet timer, then re-orders visible-first and kills suspended jobs.
  Stale results are rejected by sequence index + directory freshness
  (`isOldPreview`) — a generation counter.
  https://api.kde.org/kfilepreviewgenerator.html ,
  https://invent.kde.org/glebkasachou/kio/-/blob/master/src/filewidgets/kfilepreviewgenerator.cpp
- **macOS QuickLook.** `QLThumbnailGenerator.cancel(request:)` abandons in-flight
  generations (row scrolled away, window closed). Finder caches generated thumbnails
  in memory for fluid scrolling, purged under memory pressure.
  https://developer.apple.com/documentation/quicklookthumbnailing/qlthumbnailgenerator/cancel(for:) ,
  https://developer.apple.com/library/archive/documentation/UserExperience/Conceptual/Quicklook_Programming_Guide/Articles/QLCancelPreviewThumbnail.html

## 3. Update discipline: one row, not all rows; and frame budgets

- **Dirty rectangles (the root).** Retained-mode UIs coalesce damage; Windows
  `GetUpdateRect` returns the smallest rect enclosing the union of all pending damage.
  A batch of "something changed" marks becomes one repaint pass.
  https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getupdaterect
- **MVC (Smalltalk-80, Reenskaug).** Model changes broadcast once; views decide what
  to repaint. The conceptual ancestor of "a data change doesn't rebuild the view."
  https://en.wikipedia.org/wiki/Model%E2%80%93view%E2%80%93controller
- **Partial rebind.** RecyclerView `notifyItemChanged(index, payload)` updates just the
  changed cell (e.g. only the thumb), and DiffUtil computes the minimal change set —
  never "update the whole list because one item landed."
  https://developer.android.com/develop/ui/views/layout/recyclerview
- **Batch + frame budget.** React Native FlatList: `maxToRenderPerBatch` = 10 rows per
  batch, `updateCellsBatchingPeriod` = 50 ms. React's scheduler yields to the host
  every `frameYieldMs` = 5 ms so one long job can't stall a frame.
  https://reactnative.dev/docs/optimizing-flatlist-configuration ,
  https://github.com/facebook/react (packages/scheduler)
- **Image libs bind work to the view's lifecycle, not a global queue.** Glide checks
  `target.recycled` and drops work for views that scrolled away; SDWebImage caps
  concurrent downloads at 2; Coil pauses/cancels on scroll.
  https://github.com/bumptech/glide , https://github.com/SDWebImage/SDWebImage/issues/680

## 4. Memory: byte budgets, size-aware eviction, per-size pools, GPU re-upload

- **Classical replacement.** GDS / Greedy-Dual-Size (Cao & Irani, USITS 1997): evict by
  cost-per-byte, so big idle entries die before small busy ones. LRU-K (O'Neil et al.,
  SIGMOD 1993) and 2Q (Johnson & Shasha, VLDB 1994): a second tier protects
  repeated-access entries from a cold scan. ARC (Megiddo & Modha, FAST 2003): an
  adaptive recency/frequency split.
  Plain LRU fails for thumbnails: a scroll-past floods the cache with once-touched
  entries of equal size.
- **WebKit MemoryCache** (Source/WebCore/loader/cache/MemoryCache.cpp): 8 MB default
  budget; LRU-SP eviction (bucket by `log2(size / accessCount)`); on over-budget it
  *destroys the decoded pixels and keeps the encoded bytes*; prunes to 95% of capacity
  (hysteresis); refuses to prune a live image decoded < 1 s ago (avoids re-decode
  loops). https://raw.githubusercontent.com/WebKit/WebKit/main/Source/WebCore/loader/cache/MemoryCache.cpp
- **Chromium GpuImageDecodeCache**
  (cc/tiles/gpu_image_decode_cache.cc, image_decode_cache_utils.cc): byte budget
  `max_working_set_bytes` — 128 MB default, 32 MB low-end, 256 MB >4 GB RAM; item cap
  `kNormalMaxItemsInCacheForGpu` = 2000; simultaneous-lock cap `kMaxItemsInWorkingSet`
  = 256, with the comment "keeping very large numbers of small images simultaneously
  locked can lead to performance issues and memory spikes." Uses DiscardableMemory so
  the OS can reclaim without an explicit call, and drops the uploaded texture for
  unbudgeted images (re-upload on demand; CPU decode buffer retained separately).
  https://raw.githubusercontent.com/chromium/chromium/main/cc/tiles/gpu_image_decode_cache.cc ,
  https://raw.githubusercontent.com/chromium/chromium/main/cc/tiles/image_decode_cache_utils.cc
- **App caches.** Android `LruCache` (byte-aware `sizeOf()`); `ComponentCallbacks2.onTrimMemory`
  to shrink under pressure. iOS `NSCache` (`countLimit`/`totalCostLimit`,
  `NSDiscardableContent`, purge on memory warnings). Glide: active-resources tier in
  front of the LRU (never evict what's on screen) + `LruBitmapPool`. Fresco: fixed-size
  bucketed buffer pool (soft/hard caps) — the answer for thousands of same-size
  96x96 thumbnails. Coil: memory LRU sized as a % of the process. Qt scene graph:
  `setTextureCacheLimit`, default 64 MB.
  https://developer.android.com/reference/android/util/LruCache ,
  https://developer.apple.com/documentation/foundation/nscache ,
  https://bumptech.github.io/glide/doc/caching.html ,
  https://frescolib.org/javadoc/reference/com/facebook/imagepipeline/memory/BasePool.html
- **GPU side.** D3D9 managed textures auto-evict to system memory under pressure
  (`EvictManagedResources`) and re-upload on use — history's template for "keep the
  CPU/store copy, drop the GPU copy." D3D11/Vulkan give you hard errors
  (`DXGI_ERROR_DEVICE_REMOVED`, `VK_ERROR_OUT_OF_DEVICE_MEMORY`), so a native app must
  impose its own budget and a drop-and-re-upload path.
  https://learn.microsoft.com/en-us/windows/win32/direct3d9/automatic-texture-management ,
  https://learn.microsoft.com/en-us/windows/uwp/gaming/handling-device-lost-scenarios
- **Texture atlases.** Pack many small quads into one texture to cut per-texture
  objects and draw calls, so eviction is "drop an atlas," not 15,744 allocations.
  GitHub GPUZen2 SpriteBatching.

## 5. The recurring laws

1. **Window the view.** Only visible cells (+ overscan) exist as UI elements. Count +
   pull accessor; never materialize all rows.
2. **Recycle, don't rebuild.** Rebinding a pooled cell to a new index is the norm;
   rebuild-every-frame is the documented anti-pattern (see SwiftUI's eager-vs-lazy).
3. **Lookup before generate, generate once, memoize failures.** Cache keyed by
   path+mtime (freedesktop `fail/`, thumcache).
4. **Visible-first, and re-prioritize on scroll.** KIO pauses previews and resumes
   after 200 ms of stillness; Nautilus reprioritizes visible rows on one idle handle;
   Tumbler LIFO serves the current viewport.
5. **Cancel stale work.** Generation counters / per-viewlife cancellation
   (QuickLook `cancel`, KIO sequence index, Glide `recycled`).
6. **Coalesce notifications & slice work per frame.** One completed thumb marks one
   cell dirty (`notifyItemChanged(payload)`); batch cell updates (FlatList 10 per
   50 ms, React 5 ms yield); never notify the whole grid per thumbnail.
7. **Bound memory on three axes at once.** Bytes (decoded budget 32-128 MB), item
   count (2000), simultaneous locks (256, Chromium).
8. **Budget the volatile copy; keep the durable one.** Drop decoded pixels, keep the
   source file/encoded bytes, re-decode visible only (WebKit, D3D9, Chromium).
9. **Never evict what's on screen.** In-use/locked tier in front of the LRU.
10. **Pool uniform buffers.** Fixed 96x96 pixel pool instead of 15,744 allocations.
11. **React to whole-app pressure** (iOS warnings, Android trim, Chromium memory
    suppressor) and drop to zero on demand.
12. **The GPU won't guard you.** Native D3D11/Vulkan gives errors, not eviction.

## 6. Application to Ply (15,744 cells, +~900 MB render storm)

Ground truth: 15,744 x (96x96x4 = 36,864 B) ≈ 580 MB of decoded RGBA if held all at
once. The observed ~+900 MB working set is bigger than even that: textures + upload
staging + per-render allocations on top.

What history says to copy, in order of leverage:

1. **Window the grid.** Render only the visible cell slice (+ small overscan, e.g.
   TanStack's default of 1) and rebind/recycle as the scroll window moves. This
   collapses the base cost from 15,744 cells to ~a screenful and removes the reason
   the drain never quiesces. This is the Win32/GTK4/RecyclerView/Flutter contract; and
   GTK fixed-height-mode (fixed cell size => O(1) layout skip) is directly portable.
2. **Decouple thumbnail completions from a full grid rebuild.** On completion, touch
   the one cell's render handle only (RecyclerView payload / Win32 `LVM_REDRAWITEMS`
   lineage), or coalesce completions into a per-frame batch (FlatList's 50 ms / 10-row
   budget) instead of one notification per thumbnail.
3. **Visible-first scheduling + cancel.** Reprioritize the pool for the visible window
   on scroll (KIO 200 ms pause/resume, Nautilus idle-handle reprioritize, Tumbler
   LIFO) and drop requests for rows that left the viewport (QuickLook `cancel`,
   KIO sequence counter). Keep the 24-in-flight cap but make it visible-first rather
   than pure FIFO.
4. **Bound the decoded cache on bytes + a per-item lock cap, and drop the texture
   before the buffer.** Chromium's model (e.g. 64 MB decoded budget ≈ ~1,700 thumbs,
   a couple screens' worth, plus a lock cap and re-decode-on-demand) replaces "grow
   until the process does." Keep the file path as the durable store; never hold all
   15,744 decoded.
5. **Persist a thumcache-style disk/memory cache keyed path+mtime with a `fail/`-style
   memo**, so reopening a folder never regenerates the same 15,744 thumbnails (this is
   the single biggest implicit throttle Explorer/Tumbler/QuickLook all share and Ply
   currently lacks).
6. **Texture atlas for grid cells** (one shared atlas/upload path instead of thousands
   of tiny textures) and size-aware eviction (GDS/LRU-SP) instead of pure LRU.
7. A per-window telemetry/cap the way budgets.rs already monitors binary+RAM: make the
   decoded-thumbnail working set a counted budget and assert it.

Not every idea transfers as-is: GPUI has no built-in element pool, so the recycle pool
must be hand-rolled (consistent with the repo's ADR 0003 hand-rolled shell); but the
mechanisms above are all implementable with std + GPUI primitives already in use.