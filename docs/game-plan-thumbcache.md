# Game plan: persistent thumbnail cache

Follows from `huge-grid-thumbnails-history.md` §4. The one big remaining throttle:
reopening the same huge folder regenerates every thumbnail from scratch. The research
(Explorer `thumcache*.db`, freedesktop Thumbnail Managing Standard, QuickLook memory→disk)
all converge on the same fix: a *persistent* on-disk cache keyed by source path + mtime,
checked before generating, with failed files memoized and the cache bounded/evicted
oldest-first.

Target scope phrasing from the user: "our own thumbnail cache for everything wherever
it's applicable." So: persist the rasters we already decode (media content thumbnails,
and where sensible per-path/class icons), NOT the OS shell-data we get for free.

## Design

### Cache location & format
- Directory: `${data_dir}/ply/thumbcache/` (use the `dirs` crate already in deps; it's
  the app-data dir on Windows). Follow a freedesktop-style layout: `normal/` and `fail/`.
- File name: `<md5(path_bytes + ':' + mtime_nanos)>` (or a hex of a stable content key).
  Using path+stamp in the name means a changed mtime is automatically a new file — no
  index table needed. Old/stale files are cleaned by an eviction pass.
- Format: PNG (via the `image` crate already in `Cargo.toml`, 0.25). Store at
  `THUMB_SIZE` (96) large-side, decoded exactly as today. RGBA → RGBA PNG.
- `fail/` dir: empty marker file per key for files that can't be thumbnailed, so they're
  never retried (freedesktop `fail/` lesson).

### Lookup-before-generate on reload
- In `reload` (ops.rs) and/or the request path, before dispatching an extraction, check
  the disk cache for an existing PNG for that key; if present, decode it straight into
  the working set (skip the shell `GetImage` entirely). Only on disk-miss + source-newer
  do we extract, then write back.
- This makes the *second* and later opens of a huge folder instant: 15,744 PNG reads +
  decodes instead of 15,744 shell extractions.

### Where it hooks in
- `request_thumbnail` (thumbs.rs): on completion, write the decoded raster to the PNG
  cache (best-effort, off the UI thread). On miss-before-extract, read from cache.
  Respect the existing generation guard: only persist results for the current folder.
- The shell/type-icon tier (class/path/index icons) already dedup heavily in memory and
  are cheap (~25µs); persist only if it's free and clearly wins. **Scope decision: start
  with content thumbnails only** (the 15,744-PNG case), which is where the win is. Type
  icons stay in-memory-only for now (small, cheap, and OS-sourced). Note this explicitly
  so the scope is clear.

### Eviction & bounds
- Bound total cache bytes on the croderot (e.g. 128 MiB, mirroring Chromium's decoded
  budget; largest source is 15,744×~4 KB PNG ≈ 63 MB, so 128 MiB is comfortable).
- Eviction policy: LRU/last-accessed, oldest-first by file mtime when over the cap —
  the freedesktop delete policy. Thumbnails whose source no longer exists are removed.
- Keep memory (RAM) the thing we bound hard; the disk cache is separate budget.

## Testing
- Disk cache round-trips: write → read → identical pixels.
- Key = (path, mtime): different mtime ⇒ different file; same ⇒ cache hit.
- Lookup-before-generate: a populated cache does not re-extract (mock/assert no shell
  call for a cached key).
- Fail memo: a failed file writes a fail marker and is not retried.
- Eviction: over-cap removes oldest first; missing-source files removed.
- Budget gate still passes; full test suite green; bench 2nd-open of Screenshots is fast.

## Files
- `src/cache.rs` (new): disk-cache read/write/evict, key computation, PNG encode/decode
  via `image`, fail-memo, size accounting.
- `src/thumbs.rs`: hook lookup-before-generate into the request path; persist on
  completion (guarded by generation).
- `src/app/ops.rs`: wire reload to warm the working set from disk cache if applicable.
- `src/budget.rs`: report disk-cache size (RAM budget unchanged).

## Non-goals (per research + ADRs)
- No drive-wide background indexing (ADR 0002) — cache is per-requested-folder.
- No MTP growth (ADR 0004 deferred).
- No new heavy deps: `image` already present; md5 → use a small pure fn or `std` hasher
  (avoid adding a crypto crate for a cache key).
