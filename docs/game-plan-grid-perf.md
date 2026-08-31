# Game plan: make huge-grid browsing maximally fast and efficient

Working hypothesis, grounded in the research note `huge-grid-thumbnails-history.md` and
by the bench on `Screenshots - Copy` (15,744 PNGs): with the current code the grid is
not virtualized (it builds all cells every frame) and every completed thumbnail fires
an unconditional `cx.notify()` that rebuilds the whole visible grid. That never quiesces:
CPU pinned at ~100% for 120+s, working set +~900 MB. The list view is already
virtualized (`uniform_list`) and is fine. Everything below is the grid path.

Ground truth: 15,744 × (96×96×4 = 36,864 B) ≈ 580 MB of decoded RGBA if held all at
once; observed +~900 MB means texture/upload/staging overhead on top.

Invariants to respect (from ADRs + AGENTS.md — see `docs/research/huge-grid-thumbnails-history.md` §Application):
- 10 MiB release binary gate, 100 MiB non-GPU working set ceiling (GPU shared doesn't count). Enforced by `cargo test budgets_report`.
- ADR 0003: gpui-component for `Input` only; everything else plain `div()`s. Hand-rolled shell precedent (recycle pool here must also be hand-rolled — GPUI has no element pool).
- ADR 0004: grid stays non-virtualized "until it hurts" — it hurts now, so virtualizing is in scope.
- ADR 0002: enumerate only the current folder; never background-index a drive.
- Per-file listing invariants: type icons lookups off the UI thread; shell decode cache single-writer on the shell worker; `content_pending` release on every completion; no notify on `same_contents` reload.

## What we will change (in order of leverage)

### A. Virtualize the grid (kills the scale problem — the big one)
**Why:** non-virtualized grid builds 15,744 cells every render and every notify. This is
the root cause of both the CPU pin and most of the memory.

**What GPUI gives us today:** there is no virtualized grid primitive (`uniform_list` is
single-column; `grid_layout.rs` is non-virtual; gpui-component has no grid). The list
view already uses `uniform_list` with a `Range<usize>` render closure and it scrolls
itself.

**Decision:** reuse `uniform_list` by packing a fixed number of cells per row.
- Fixed cell size is already a constraint (96 px cell, 56 px icon — `grid_cell`,
  browser.rs:290). Compute `cols = max(1, viewport_width / CELL)`, then each
  `uniform_list` item is one row that holds `cols` cells. The render range is already a
  flat item index range, so a row = `ix*cols .. ix*cols+cols` mapped into the flat
  `visible_indices`.
- A row's height is uniform and predictable (CELL + label line), so `uniform_list`'s
  uniform-height assumption holds. Measure item 0 once.
- Recompute `cols` on window resize (GPUI expose/viewport change notification) and on
  `rebuild_visible`. Do NOT put the grid inside `scroll_area` any more — `uniform_list`
  scrolls itself (browser.rs:35-37 comment already says this).
- This turns per-frame cost from O(15,744 cells) into O(visible rows ≈ 30-80 cells).

**Files:** `src/ui/browser.rs` (grid branch → `uniform_list` grid), `src/app/mod.rs`
(recompute cols on resize), `src/app/ops.rs` (no change to rebuild logic, but verify
`visible()` isn't called per-frame for the grid — see E).

### B. Stop rebuilding the whole grid per thumbnail (kills the notification herd)
**Why:** thumbs.rs:489 fires `cx.notify()` per completed thumbnail — full window
re-render, and with A that re-render still rebuilds every visible row's elements. Also
`class_icon` (539), `path_icon` (1633), `recycle_bin_icon` (1684), `refresh_lnk` (739).

**What to change:**
1. **Coalesce notifications into per-frame batches.** Replace per-completion
   `cx.notify()` with a dirty flag: set `thumbs_dirty = true` on the model and call
   `cx.notify()` at most once per frame (e.g. a flag checked in the render pass, or a
   short debounce). This is the FlatList lesson — batch cell updates, don't notify once
   per thumbnail. Target: one notify per frame at most during a herd, not one per item.
2. **Make per-thumbnail work cheap to re-render.** With A, a notify rebuilds only the
   visible rows anyway; the coalescer makes the herd near-idle.

**Files:** `src/thumbs.rs` (all the `cx.notify()` sites → set dirty + single notify),
`src/app/mod.rs` / `src/ui/browser.rs` (consume the dirty flag).

### C. Bound the decoded-thumbnail cache properly (kills the memory growth)
**Why:** `BUDGET = 32 MiB` exists (thumbs.rs:36) and `insert`/`push` evicts via plain LRU
past 32 MiB (thumbs.rs:267-281). But the bench showed +900 MB because (a) grid built
15,744 `RenderImage` textures that stayed alive via elements/kept references across
notifies even if the CPU buffer LRU-evicted, and (b) there is no per-item lock cap, so a
herd keeps them resident.

**What to change:**
1. **Count and cap like Chromium, not just LRU bytes.** Add a working-set lock: only
   thumbnails actually visible (or visible + small overscan) are held strongly;
   anything else lives in the 32 MiB LRU and is evicted when the byte budget trips or
   on a pressure signal. The grid with A holds only visible rows, so this falls out.
2. **Guarantee evictability.** Ensure nothing per-row keeps a strong reference across
   frames (the old grid did). With A, elements are reconstructed from the cache each
   frame, so eviction is free.
3. **Optionally:** treat decoded CPU pixels as discardable and re-decode on demand
   (WebKit lesson), keeping the on-disk path + mtime as the durable store keyed
   path+mtime (already the `CacheKey`).

**Files:** `src/thumbs.rs` (ThumbCache lock set + eviction), `src/budget.rs` (report a
decoded-thumbnail byte counter, leave the gate numbers).

### D. Scheduling: visible-first + cancellation (kills wasted extraction)
**Why:** the pool is bounded at `CONTENT_CAP=24` and drains pure FIFO top-down
(`content_dispatch` round-robin, queue order = request order). On a big folder it
spends time extracting off-screen rows; results for rows that scrolled away are still
inserted, and stale results from a navigated-away folder still fire (no generation
guard on `request_thumbnail`).

**What to change:**
1. **Visible-first.** Above `CONTENT_CAP` is already a hard gate; re-prioritize what the
   pool does so visible rows dispatch before off-screen rows. Smallest change: keep the
   pool FIFO but drive the *request* order from the visible window (request thumbnails
   for visible rows first, then a bounded lookahead), rather than every render calling
   `ensure_entry_icons` for `PREFETCH_SCAN` first. This mirrors KIO (pause/resume on
   scroll) and Nautilus (repioritize visible on one idle handle) — simplest form:
   request visible-range thumbnails first, then lookahead.
2. **Stale-result rejection + cancellation.** Thread a listing generation into
   `request_thumbnail` and drop the insert + notify when the folder changed (KIO
   sequence-index / `isOldPreview`; QuickLook `cancel`). For in-flight rows that scrolled
   off, don't bother re-using the result if the row left the working set.

**Files:** `src/thumbs.rs` (visible-first enqueue, generation guard/cancel),
`src/ui/browser.rs` (request visible-first, not `PREFETCH_SCAN`-first).

### E. Reduce per-frame allocation in `visible()`
**Why:** `visible()` rebuilds a `Vec<&Entry>` every call (ops.rs:311-319) and is called
per-frame in render (browser.rs:28) and in `uniform_list`'s processor (browser.rs:45)
plus several selection paths. Not the main cost, but free to fix and compounds with A.

**What to change:** when the listing/filter is stable, cache the `visible_indices`
snapshot and return a slice, avoiding a fresh Vec per call; or have the grid processor
index into the snapshot directly. Verify with the profiler before/after.

**Files:** `src/app/ops.rs`.

## How we will test it all

### Verification chain (must stay green every step)
Run after each change (list view regression + budgets):
```
cargo check
cargo test
cargo clippy --bin ply
cargo test budgets_report -- --nocapture   # enforces 10 MiB release gate
```
ADR: budget gate must keep passing; `budgets_report` asserts release size ≤ 10 MiB.

### Functional correctness tests (new, unit)
- **Virtualized grid correctness:** a grid-Layout function that maps a flat item index
  <-> (row, col) and the visible-range split must be unit-tested over edge widths
  (width not dividing cols, empty listings, 1 item, exactly cols, cols+1, a filter that
  hides rows). Assert the produced range covers exactly the windowed items.
- **Thumbnail coalescing:** a test asserting that a batch of N thumbnail completions
  produces ≤1 (or ≤ few) notify, not N.
- **Cache lock budget:** a test that inserting beyond the lock cap evicts to the working
  set and that a cached-but-not-locked thumbnail can be evicted/re-decoded.
- **Stale-result rejection:** a test that a thumbnail for a prior `list_generation` is
  not inserted/notified.
- **Visible-first scheduling:** a test (or an internal queue-order assertion) that
  visible rows are enqueued before off-screen rows.

### Bench / resource monitoring (the real acceptance gate)
The existing ad-hoc harness is at `C:\Users\Yashraj\AppData\Local\Temp\opencode\bench-ply.ps1`
(reusable: launches the release binary via `PLY_OPEN`/`PLY_GRID` env hooks, samples every
150 ms, scores settle-time + steady-state CPU/working-set after a 200 ms sample of
6 consecutive <3% CPU). We will formalize this into the repo (optional
`tests/bench` or a `benches/` + doc), and gate on pre/post deltas:

**Benches (run each, before and after each step A-E):**
1. `Screen Recordings - Copy` (895 items: 864 mp4). Baseline `5.86 s settle / peak 302 % / +110 MB`.
2. `Screenshots - Copy` (15,744 png). Baseline `>120 s (no settle) / ~100-125 % continuous / +900 MB`. **This is the target.** Acceptance: settles to idle CPU (<3% sustained) and working set stays within budget (non-GPU ≤ 100 MiB above typical idle, ideally bounded to ~the 32 MiB decoded cache + a couple screens of textures).
3. Idle Home (control): must stay at baseline ~345 MB / ~4 % CPU.
4. A mixed folder (folders + files + media + a few .lnk) for regression on the icon path, grid and list.

**What we measure per bench:** launch→settle seconds; peak CPU %; steady idle CPU %;
steady working-set MB; peak working-set MB; (dev) private MB. We compare before/after
per step and require: A fixes the never-settle; B drops the per-frame herd cost; C keeps
memory bounded and re-decodes under pressure; D keeps visible rows snappy and stops
stale work; E reduces steady per-frame allocation.

### AGENTS.md gate
After implementation, `cargo test budgets_report -- --nocapture` must still PASS and the
printed gate obeyed (10 MiB release binary; the 100 MiB working-set is reported — we will
make the grid behavior keep non-GPU working set well under it).

## Execution order / milestones
1. **A (virtualize grid)** — biggest lever. Verify functional + bench #2 settles.
2. **B (coalesce notify)** — remove the herd. Verify bench #2 CPU drops dramatically.
3. **C (lock-capped cache)** — bound memory. Verify bench #2 working set stays put.
4. **D (visible-first + cancel)** — snappy, no stale work. Verify scrolling feels right.
5. **E (de-allocate visible())** — polish. Verify with profiler.
6. Full verification chain + all four benches + budgets gate. Commit.

Not committing or changing budgets until the plan is approved. Each step is independent
enough to land and re-bench on its own, so we can stop early if a layer isn't paying off.

## Scope guard / non-goals (per ADRs)
- No drive background indexing (ADR 0002). Virtualization doesn't enumerate more — it
  only renders less.
- No MTP feature growth (ADR 0004 deferred).
- No hexagon/ports/plugin walls (ADR 0004) — a hand-rolled `Element` or `uniform_list`
  grid packing stays inside `src/ui`.
- No new crates unless a benchmark harness genuinely needs one and it stays ≤ the
  established bar.
