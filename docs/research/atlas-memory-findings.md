# Atlas memory findings

Measured on Windows/ARM64, Qualcomm Adreno iGPU (unified memory), 1280x800
window, release build. Method: `Get-Process` WorkingSet64/PrivateMemorySize64
plus the `GPU Process Memory(*)\Shared Usage` perf counter, which splits
driver-committed GPU-shared memory out of the process total. Per AGENTS.md,
only the non-GPU working set counts toward the 100 MiB RAM ceiling; Task
Manager shows the total including GPU-shared.

## The split

| State | Private | GPU-shared | Non-GPU |
| --- | --- | --- | --- |
| Home, before | ~362 MB | ~326 MB | ~36 MB |
| Home, after | ~195-256 MB | ~159-220 MB | ~36 MB |
| 2k-image folder open | ~436 MB | ~391 MB | ~44 MB |
| After 60-wheel fling + settle | ~511 MB | ~454 MB | ~58-76 MB |

Non-GPU was always under the ceiling. Everything below is about the
Task Manager total, which is ~90% GPU-shared on this machine.

## What each fix addressed

1. Subpixel text pipeline (~100 MB). One painted text run in a named font
added ~110 MB GPU-shared over a bare window (minimal GPUI examples: 64 MB
vs 184 MB). Grayscale mode avoids the 4-variant subpixel atlas entirely.
Shipped as `set_text_rendering_mode(Grayscale)` in `main.rs`. Visual
difference is negligible.
2. Uncapped shell bitmaps. `IShellItemImageFactory::GetImage` can hand back
bitmaps far larger than requested; each became a full-size atlas tile.
`fit_thumb` in `thumbs.rs` normalises every decoded raster to 96 px on the
long side before it becomes a `RenderImage`, and the disk cache persists
the fitted bytes.
3. Atlas never evicts. GPUI's window atlas has no eviction, so every painted
thumbnail kept its tile forever. `ThumbCache` now queues fully-evicted
rasters in `pending_drops`, drained each render via `Window::drop_image`.
4. Dead-bucket retention. The atlas allocator only reuses freed space once a
whole bucket empties, and the locked working set pins tiles across pages,
so turnover holes are never reusable. Past `FLUSH_THRESHOLD` evicted bytes
(`drain_drops`), every live tile (map, locked, class/index/stock maps) is
dropped at once; visible tiles re-upload on the same paint, netting the
atlas near the live set. Class/index/stock tiles are included because they
interleave temporally with thumbnails and would otherwise pin mixed
buckets forever.
5. Fling paintings. Each distinct tile painting permanently costs on the
order of 500 KB GPU-shared on this driver, so a fling past 2k files costs
hundreds of MB for frames visible 16 ms each. `Ply::note_paint` detects
sustained ~48 fps repaints; while set, listing cells paint placeholder
slots for content thumbnails only. Shared class icons still paint, and
extraction keeps warming the cache, so previews fill in on settle.

Tuning: `BUDGET` 8 MiB (~220 tiles: viewport + lookahead + scroll-back),
`LOCK_CAP` 128, `FLUSH_THRESHOLD` 2 MiB.

## Round two: the lock was locking everything

`visible()` returns the whole listing, and the render path passed all of it
to `set_working_set` every frame while prefetching rows `0..N` regardless of
scroll position. Consequences, all measured or code-proven:

- O(listing) PathBuf clones plus hashing on every frame (the largest
  main-thread cost in the app at 15k files).
- The 128-entry lock cap spilled for any folder over ~128 thumbnails, so
  on-screen tiles were evicted by off-screen completions. An evicted video
  tile re-extracts in seconds (image: milliseconds), which is the video
  "blink" (blank slot, slow reappear). Images cycled the same way but too
  fast to see.
- Prefetch warmed the top of the folder even when scrolled to row 9,000.

Fix: the virtualized row processors record the painted entry range into
`Ply::last_viewport`; prefetch and the lock use that window (plus symmetric
overscan), clamped to `LOCK_CAP` around the painted center so the spill can
never take an on-screen tile. First paint and generation changes fall back
to the top.

The storm gate is movement-only: viewport travel over 40 entries per
250 ms window, with a sticky reference across windows (no on/off flicker
mid-fling). Repaint rate alone is the wrong signal both ways: fill trickle
repaints fast while stationary (must stay progressive), and upload-bound
fling frames render too slowly to trip any fps threshold. Verified by
transition logging: one engagement per fling, no flicker.

## Results after round two

15,744-PNG folder, 60-wheel fling: storm peak flat ~465 MB (was a
520-920 MB sawtooth), settles to 274 MB Task Manager (219 MB GPU-shared).
811-video folder: down-and-back scroll flat ~450 MB throughout, idle 0% CPU
— the blink mechanism (on-screen eviction) is gone. Home unchanged ~256 MB.

## Round three: instant icons and folder-change release

User report: grid cells sit blank (not even the type icon) on open and
after flings. Three compounding causes, all fixed:

- The type-icon batch did one shell lookup per entry (512 identical
  `SHGetFileInfoW` calls for an all-PNG folder), stalling every icon behind
  it on the single shared shell worker. The worker now resolves each
  distinct extension once; per-path targets (folders, executables) stay
  per-entry.
- A fling's last frames could all be storm slots with nothing left pending,
  so no further render ever repainted the settled viewport: the screen
  froze on placeholders. A debounced 300 ms settle timer per storm
  guarantees the follow-up paint.
- Storm cells painted blank slots even when the shared class icon was
  cached. They now paint the class icon (one shared tile, dedupes free)
  and fall back to the slot only when it is genuinely unknown.

Navigating folders now releases per-file rasters eagerly
(`ThumbCache::clear_content` on every location change; shared type icons
survive): Back out of a flung 15k folder drops ~200 MB, Forward
re-resolves from the disk cache. Verified live: 449 MB after fling,
247 MB after Back, 473 MB after Forward.

## Round four: sidebar chrome stops blinking

The folder-change release had a side effect: sidebar and Home drive
icons live in the same evictable tiers, so every navigation blanked the
chrome too. Per-path icons for chrome now live in a pinned LRU tier
(256 entries) that survives navigation, budget pressure, and working-set
replacement; listing folder rows stay in the normal tiers. The flush
covers pinned tiles so they can't pin mixed buckets either.

Same review pass also closed four smaller holes, all found by tracing
every blank-slot path: disk fail-marker and corrupt-PNG completions now
notify (they stranded cells blank with nothing pending), a corrupt cached
PNG is deleted and the resolve genuinely falls through to shell
extraction (the comment always promised this; the code returned early),
LRU order entries are removed on promote (duplicates let live entries
evict early and grew the deque without bound while scrolling), and the
stock Recycle Bin icon memoizes failure instead of retry-repainting
forever.

## Known limit

The settle number is floored by per-painting driver retention times live
tile count; no app-side eviction scheme reclaims it (drops run, pages free
per the allocator, the driver keeps the memory). Getting a fully-browsed
2k-image folder under 300 MB Task Manager needs structurally fewer tiles:
pack viewport thumbnails into a few contact-sheet textures instead of one
tile per file. That is future work; it needs custom UV painting.
