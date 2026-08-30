# GPUI texture path, at the Zed commit Ply pins

Zed is pinned in Cargo.lock at `9bde578ef5afa84920c4300af25f9dee31c96fcf`. Everything below is read from that commit, so it stays true even after Zed moves on.

## What a RenderImage actually is

`RenderImage::new` is CPU-only. It hands out an `ImageId` from an atomic counter and keeps `image::Frame` (RGBA8) buffers in a `SmallVec` on the heap. Nothing touches the GPU until the image is painted. `RenderImageParams { image_id, frame_index }` is the atlassing key.

Source: crates/gpui/src/assets.rs; the element path in crates/gpui/src/elements/img.rs paints `Arc<RenderImage>` directly into `Window::paint_image`.

## Where the bytes hit the GPU

Each window owns `sprite_atlas: Arc<dyn PlatformAtlas>` (gpui window.rs:1122). On Windows that is `DirectXAtlas` in crates/gpui_windows/src/directx_atlas.rs. `paint_image` (gpui window.rs:4389) calls `sprite_atlas.get_or_insert_with(&params.into(), build)`.

On a miss, `DirectXAtlas::get_or_insert_with` (directx_atlas.rs:74) runs the build closure, pulls a region out of an `etagere::BucketedAtlasAllocator`, and calls `UpdateSubresource` on the D3D11 immediate context (directx_atlas.rs:304). The copy is synchronous and happens inline, on whichever thread painted. This runs exactly once per distinct `ImageId`, then the tile is memoized in `tiles_by_key: FxHashMap<AtlasKey, AtlasTile>`.

Atlas pages are B8G8R8A8_UNORM, 1024px minimum up to 16384px (directx_atlas.rs:159-189). A 48px icon tile is about 9 KiB of GPU copy. Roughly 440 tiles fit one 1024px page. `remove` frees the tile when the last reference to the `RenderImage` drops (gpui window.rs:4507 `drop_image`); Ply keeps its `Arc`s for the window life, so tiles persist across scroll.

## The five questions

1. Creating a RenderImage does not upload anything. It is an increment on an atomic plus a byte buffer.

2. Upload happens at first paint, on the main thread, via that single `UpdateSubresource` per new tile. No async path, no texture command queue.

3. All 48 rows sharing one `Arc<RenderImage>` cost one tile and one upload, then a `PolychromeSprite` primitive per paint. Distinct RenderImages cost one upload each. Ply already shares per-extension class icons and the stock recycle bin icon. It does not share folder icons: `folder_icon` keys by path plus mtime (thumbs.rs:1288), so every folder in a directory is its own extract, its own RenderImage, its own ImageId, its own tile and upload, even when all of them resolve to the same shell icon index.

4. No batching or deferral exists. Missed tiles update the texture immediately, in paint order. Per-file icons also land one at a time because each completion calls `cx.notify()` and repaints the row that just resolved (thumbs.rs:353).

5. Glyphs use the same `DirectXAtlas`, monochrome R8 pages, same `get_or_insert_with` memoization during paint. The 48-tiny-textures fear is unfounded: icons ride the same texture atlas the font renderer already maintains.

## What actually makes rows appear row-by-row

Three stacked causes, none of them the GPU path.

First, every shell call runs on one STA worker thread (`CoInitializeEx(STA)` then an mpsc loop, thumbs.rs:671-710). `SHGetFileInfoW`, `IShellItemImageFactory::GetImage`, the icon-list `GetIcon`, the GDI decode, all serialize on that thread. A visible window of folders is a queue.

Second, folders force a fresh extract per path. `path_icon_pixels` (thumbs.rs:926) asks the shell for the index, then pulls that index's artwork from SHIL_EXTRALARGE. Nearly every folder in a directory returns the same index, but Ply throws that index away and keys the result by path. So the queue is full of duplicate work that produces pixel-identical icons.

Third, nothing reveals the listing as a set. `entry_icon_probe` (thumbs.rs:458) flips each row Loading to Ready the moment its own task lands, and each flip notifies and repaints, so a scroll fills in one row at a time. Comments mention a reveal gate that holds rasters until the visible window is all-resolved; that gate does not exist in the code.

## First attack: share folder icons by shell icon index

Make folder icons behave like class icons. Have `path_icon_pixels` return the `sfi.iIcon`, and cache the decoded raster in a per-cache `HashMap<i32, Arc<RenderImage>>` bucket instead of per path. One generic-folder index collapses an N-folder directory into one extract and one upload. Custom `desktop.ini` icons survive this: a custom icon changes the index, so a re-extract replaces on its own; the mtime key only exists to catch that, and the index does the job more directly.

Blast radius before touching it: `folder_icon`/`path_icon`/`probe_key`/`ThumbCache` keying, the Sidebar and Home `path_icon_probe` call sites, and the tests that pin mtime keying (`folder_stamp_is_zero_when_no_mtime_and_distinct_when_set`). MTP exclusion and per-path inflight failure state must stay. This is the change with the best win per line, so it is where to start.

Secondary, cheaper-real-wins-than-GPU work: coalesce the reveal into one notify once no visible entry is pending, and admit the STA worker is the real serialization point if a directory of media thumbnails still feels slow.

## Verdict

Dedup identical icons and pre-create them beats the current per-item async plus upload, but not for the reason suspected. The upload was never the cost; repeated shell extracts and one-row-at-a-time reveals are. Sharing `Arc<RenderImage>` by icon index kills duplicate extracts, duplicate allocations, and duplicate uploads at once, and it needs no changes to the GPUI layer at all.