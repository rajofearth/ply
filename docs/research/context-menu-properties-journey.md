# Context menu and Properties parity journey

Goal: close the gap with Explorer on context menu and Properties without hosting shell extensions. Keep the hand rolled shell, keep theming, stay inside budgets.

Start point, 6 Sep 2026. Ply menu has Open, one child Open with, Run as admin for exe like targets, Open in Terminal for dirs, Pin, Copy path, Reveal, Properties, Delete. Cut, Copy, Paste are stubs. Properties is a 320px card with Type, Size, Modified, Path plus async Author, Title, Comment, Dimensions, Created. Icons are lucide only in menus. Listing and sidebar already use shell rasters through SHGetFileInfo plus SHGetImageList.

This file logs each step, what changed, what it cost, what stayed out.

## Log

### 1. Start
Opened this log. No code change yet. Next: shell icons in menu and Properties header, richer read only Properties rows, Open With list from the shell.

### 2. Shell backend for menu icons and richer Properties, 7 Sep 2026
Backend only, in `src/thumbs.rs`. App layer (`src/app/mod.rs`, `src/app/ops.rs`) and UI layer (`src/ui/overlay.rs`) untouched.

What changed:
- `StockIcon` grew from one variant to eight: `RecycleBin`, `Shield`, `Info`, `Delete`, `FolderOpen`, `Application`, `MixedFiles`, `Folder` (`src/thumbs.rs:135`). Each maps to one `SIID_*` id in `backend::stock_siid` (`src/thumbs.rs:2129`); only the bin keeps the empty/full branch, decided in `stock_pixels` by the existing `$Recycle.Bin` scan (`src/thumbs.rs:2146`). Resolution still uses `SHGetStockIconInfo` with `SHGSI_ICON` on the single STA `SHELL_WORKER`.
- New `MenuIconSource` enum (`src/thumbs.rs:158`): `Path { path, stamp }`, `Class(String)`, `Stock(StockIcon)`, `Exe { path, index }`. The stamp is caller cache identity and the worker ignores it; the exe index is a fallback when the fresh `SHGetFileInfoW` lookup fails. One `ShellJob::ResolveMenuIcon` variant (`src/thumbs.rs:1446`) serves all four through `menu_icon_pixels` (`src/thumbs.rs:2177`), which reuses the worker per-index cache and the shared 48px `SHIL_EXTRALARGE` decode (`index_icon_at`, `list_icon_at`). No new image list size, no `content_dispatch` for icons.
- New blocking helper `backend::request_menu_icon_pixels` (`src/thumbs.rs:1631`) plus crate root `menu_icon` (`src/thumbs.rs:2514`) for background threads. MTP paths return `None` before dispatch and again worker side, so MTP never queues. `request_properties` got the same never queue MTP guard (`src/thumbs.rs:1644`).
- `read_properties_impl` (`src/thumbs.rs:1677`) keeps the five existing PKEY rows and adds: `Type` (`PKEY_ItemTypeText`, else `SHGetFileInfoW` with `SHGFI_TYPENAME`, `src/thumbs.rs:1849`), `Size on disk` (`GetCompressedFileSizeW` rounded up via `GetDiskFreeSpaceW`, with `INVALID_SET_FILE_POINTER` plus `GetLastError` handling, skipped for directories, `src/thumbs.rs:1889`), `Created` fallback and `Accessed` (`PKEY_DateAccessed`, else `metadata`, `src/thumbs.rs:1745`), `Attributes` from `GetFileAttributesW` (`src/thumbs.rs:1776`), and an extension gated media allowlist off the same property store (image width/height/bit depth/date taken, media length as `m:ss`, video frame size, document pages, subject, `src/thumbs.rs:1779`). Filesystem facts resolve even when the store fails to open. Still sync, no hosting. Pure helpers `format_attributes`, `round_to_cluster`, `format_duration_100ns` (`src/thumbs.rs:178`, `:213`, `:227`) reuse `listing::format_size` and `format_mtime`.
- Non Windows stubs for the two request helpers return `None` (`src/thumbs.rs:2438`).

Merge point for the app agent: `src/app/mod.rs:68` defines its own `MenuIconSource`/`MenuStock` (no `Exe`, no stamp). Mapping to the backend type is `Path(p)` to `Path { path: p, stamp }`, `Class(e)` to `Class(e)`, `Stock(s)` to `Stock(...)` per same named variant, and exe rows to `Exe { path, index: -1 }` until a real index is known. Wiring `overlay.rs:166` to `thumbs::menu_icon` on a background thread is still open.

Blast radius: single STA worker kept, `sfi.iIcon` indexing untouched, `ThumbCache` stock maps keyed by the wider enum with no behaviour change for the bin row, no new deps. `read_properties` signature unchanged so `src/app/ops.rs:1169` keeps compiling.

Measured: `cargo check --all-targets` clean (eight dead code warnings for the not yet wired menu helpers, expected). `cargo test` 131 passed, 0 failed. `cargo test thumbs::` 30 passed, including live shell decodes for all eight stock ids and menu path/class/stock sources. `cargo test budgets_report -- --nocapture` prints release size 9.71 MiB, PASS against the 10 MiB gate.

### 2. Shell icons in menu rows and Properties header, 6 Sep 2026

UI layer only. Touched `src/ui/overlay.rs`, nothing else.

What changed

- Menu rows now paint a shell raster when the row carries a shell source. `menu_shell_source` (overlay.rs:151) prefers the `shell` field the app layer sets on `MenuItem` and falls back to the Open action target for rows built without one. `probe_menu_shell` (overlay.rs:164) resolves it: Path goes through the shared path probe (overlay.rs:195), Class reads the shared per-extension cache without extracting, and only Recycle Bin among the stock variants uses a real raster. The rest keep the lucide glyph.
- The leading box in `menu_row` (overlay.rs:240) holds a fixed 14px in all three states: Ready paints `thumb_img` at 14px (overlay.rs:245), Loading paints `icon_slot` at 14px (overlay.rs:246), failure paints the existing lucide `icon()` or the spacer. Shell rasters are never tinted; only the fallback glyph takes the danger and disabled colors.
- The toolbar is unchanged and stays lucide-only (overlay.rs:35). Full color shell art at that size would clash with the monochrome glyphs.
- The Properties dialog has a 32px header icon left of the name (overlay.rs:372), probed the same way as the Open row. Loading shows `icon_slot` at 32px (overlay.rs:375) so the row keeps its height. The card stays 320px wide; the name takes the remaining space with truncate. The fallback glyph comes from `properties_fallback` (overlay.rs:287): volume icon, then listing entry icon, then folder glyph for folders, else the file glyph.
- No new deps. No new colors, radius, or fonts. Rasters are the shared cached `Arc` handles, so no extra uploads per row.

Test output

- `cargo fmt --check` reports no diffs in `src/ui/overlay.rs` or `src/ui/mod.rs`.
- `cargo check` and `cargo test budgets_report -- --nocapture` do not pass yet, and none of the errors point at the UI files. The failures sit in `src/fs_ops.rs` (backend shell calls: handler enum args, AssocQueryStringW pointer type) plus earlier in the session missing app helpers that have since landed. Those files belong to the backend and app agents. The UI half is done and waiting on their compile fix; rerun the gate once the tree builds.

### 3. App layer for menu shell sources and Properties data, 6 Sep 2026

App layer only. Touched `src/app/mod.rs`, `src/app/ops.rs`, `src/fs_ops.rs`. Did not touch `src/thumbs.rs` or `src/ui/overlay.rs`.

What changed

- Added `MenuIconSource` with Path, Class, and Stock variants plus `MenuStock` with Shield, Folder, FolderOpen, Info, RecycleBin, Delete, and MixedFiles (`src/app/mod.rs:68`, `src/app/mod.rs:79`). Added `shell` to `MenuItem` (`src/app/mod.rs:94`) with a `with_shell` builder, keeping `icon` as the lucide fallback. Merge point: `src/thumbs.rs` now defines its own `MenuIconSource` with Path plus stamp, Class, Stock, and Exe, and its own `StockIcon`. The UI in `src/ui/overlay.rs` imports from `crate::app`, so the app type stays as the contract. The backend should map from the app type to its own type when resolving, or the two should be unified in one place.
- Added `MenuAction::OpenWithHandler` carrying path and handler name (`src/app/mod.rs:159`). The run path (`src/app/ops.rs:817`) shows a note and opens the Choose-app picker, since shell Invoke is still deferred.
- Wired `open_menu` (`src/app/ops.rs:552`): Open uses path or class for a single target and MixedFiles stock for multi (`src/app/ops.rs:1239`), Run as admin uses Shield stock, Open in Terminal uses the terminal exe path (`src/fs_ops.rs:459`), Reveal uses FolderOpen stock, Properties uses Info stock, Delete uses RecycleBin stock when all targets can trash else Delete stock (`src/app/ops.rs:1250`). Copy path, Pin rows, View, Sort, and Refresh keep lucide only with no shell source. The New Folder child in the empty menu uses Folder stock (`src/app/ops.rs:741`).
- Added `list_open_with_handlers` capped at 6 (`src/fs_ops.rs:313`, `src/fs_ops.rs:315`) via SHAssocEnumHandlers plus IAssocHandler on Windows, empty elsewhere. It refuses MTP and bin paths and returns empty on any failure, so menu open never blocks. The Open with flyout (`src/app/ops.rs:611`) lists those names as `OpenWithHandler` rows with Choose another app kept last.
- Split Location from name in `show_properties`: `split_location` returns the parent dir or the full path for roots (`src/app/ops.rs:1263`). Added `location` and `opens_with` to `Properties` (`src/app/mod.rs:195`, `src/app/mod.rs:198`). The opens-with value comes from `friendly_app_name` via AssocQueryString with fallback to the kind label (`src/fs_ops.rs:404`, `src/app/ops.rs:1272`). The async `fill_properties` path is unchanged.
- Added copy quoting groundwork: `quote_path_for_copy` quotes on spaces (`src/fs_ops.rs:472`) and `join_paths_for_copy` newline joins for multi (`src/fs_ops.rs:483`). `CopyPath` now writes the joined quoted form (`src/app/ops.rs:838`); the note text and single path scope are unchanged.
- No HWND hosting, no IContextMenu hosting, no new deps. Shell work stays to fast registry reads on the calling thread with empty fallback.

Test output

- `cargo check` passes with warnings only. Remaining warnings are the new `location` and `opens_with` fields waiting on the UI to read them, plus unused backend menu icon helpers waiting on the type unification above.
- `cargo test` passes: 131 passed, 0 failed, 1 ignored (MTP probe needs hardware).
- New tests: quoting and join in `src/fs_ops.rs`, handler cap and MTP refusal in `src/fs_ops.rs`, open shell path versus class versus mixed, delete stock gating, location split, opens-with fallback, and caps gating in `src/app/ops.rs`.
- `cargo test budgets_report -- --nocapture` passes: release size 9.71 MiB, PASS against the 10 MiB ceiling.

### 4. Integration, 7 Sep 2026

Parent merged the three agent branches. No new shell calls, no new deps.

What changed:

- `src/thumbs.rs`: added generic `stock_icon` plus `stock_probe` for any `StockIcon`. `recycle_bin_icon` and `recycle_bin_probe` are now thin wrappers over them, so sidebar behavior is unchanged. Menu rows get one raster per stock id, decoded once on the STA worker, shared through the existing stock map.
- `src/ui/overlay.rs`: `probe_menu_shell` maps every `MenuStock` to its `StockIcon` and probes through `stock_probe`. Before this only RecycleBin painted a raster. The rest fell back to lucide. Rasters stay untinted. The toolbar stays lucide-only. Properties now shows Opens with and Location rows from the app layer fields, plus the existing Type, Size, Modified, Path and async details. The 32px header icon and 320px card are unchanged.
- `src/app/mod.rs`: trimmed the merge note. `MenuItem.shell` stays the single UI contract.
- Ran `cargo fmt` for the two formatting diffs the agents left in `ops.rs` and `fs_ops.rs`.

Measured: `cargo check --all-targets` clean with dead code warnings only for the background-thread `menu_icon` wrapper family, which tests cover. `cargo test` 131 passed, 0 failed, 1 ignored. `cargo test budgets_report -- --nocapture` 9.71 MiB, PASS.

Left out on purpose: third party verbs, Security editor, Sharing wizard, Previous Versions, editable Details, hosted sheets and menus. Those need HWND hosting and break theming. Follow-up worth doing: unify the worker-side `thumbs::MenuIconSource` blocking wrapper with the app-side `MenuItem.shell` contract, or delete the wrapper since probes cover the UI path.

### 5. Clippy cleanup, 7 Sep 2026

Your `cargo clippy` showed 9 warnings. All 9 are gone. `cargo clippy` on the binary is silent. `--all-targets` keeps 8 test-only style lints that predate this work.

What changed, one writer per file:

- `src/thumbs.rs`: deleted the duplicate blocking wrapper family outright. `MenuIconSource`, `ResolveMenuIcon` plus its match arm, `request_menu_icon_pixels` with both stubs, `menu_icon_pixels`, `menu_icon` are gone. The UI never called them. Probes cover that path. Deleted `recycle_bin_icon` too. Zero callers, the probe covers it. Deleted `StockIcon::Application` with its `stock_siid` arm. Nothing constructed it. The stock set is 7 now. Tests moved to the direct resolvers they already wrapped, and the MTP test now guards `request_properties`.
- `src/app/mod.rs` plus `src/app/ops.rs`: `set_props` took 9 args. It takes one now. New `PropsFields` struct carries name, kind, size, modified, path, location, opens_with. A pure `props_from_fields` maps it to `Properties`. All 3 call sites pass the same values through the struct. Four new tests pin the mapping, the volume free-of-total line, the dir em-dash size, the fallback kind and size.
- `src/ui/overlay.rs`: both nested ifs collapsed into let-chains, same early returns. The stock mapping came out into a pure `menu_stock_icon` fn with a test over all 7 variants.

Measured: `cargo clippy` zero warnings on the binary. `cargo test` 136 passed, 0 failed, 1 ignored, up 5 from 131. `cargo test budgets_report -- --nocapture` 9.71 MiB, PASS. `cargo fmt --check` clean.

### 6. Menu and Properties redesign research, 7 Sep 2026

Three research agents, no code. Screenshots of our menu, our Properties, Explorer Properties, Explorer menu as reference.

Menu teardown. Explorer has a labelled top strip plus grouped rows with right-aligned accelerators. Ours has an icon-only toolbar with permanently disabled Cut/Copy, no accelerators, 28px rows, 14px icons, content-width panel. The flyout floats detached because of fixed math in `src/ui/overlay.rs:87-89`. It assumes 28px per row but separators are 9px, assumes 42px toolbar and 172px width. Every separator above the flyout drifts it ~19px. Flyouts open on click only, no hover, no keyboard nav, Esc closes the whole menu instead of the flyout first.

Properties teardown. Explorer General is icon plus name header, two-column grid, Size with byte counts, Size on disk, Contains for folders, three full dates, Attributes checkboxes, OK/Cancel/Apply footer. Ours is a 320px hairline list with real bugs. Duplicate Type rows from base plus details. Opens with shown for folders where it means nothing. Size em-dash for folders with no Contains. Relative dates instead of full ones. Attributes as a comma string including internals like Directory and Link.

Feasibility. Nest the flyout in its parent row instead of computing offsets, hover intent with 150-200ms grace, Esc and left-arrow close flyout first. Keyboard nav needs a selected index plus scoped bindings, medium work. Folder Size plus Contains needs a dedicated background thread off the STA worker, iterative walk, no symlink following, MTP and network exclusion, generation guard, separate cache that never writes back into Entry size or sorting and watch equality break. All of it fits the 10 MiB and 100 MiB budgets with std::fs only.

### 7. Rebuild, 8 Sep 2026

Four agents, one file group each, shared contract up front. Parent wired the last gap by hand.

What changed:

- Backend (`src/thumbs.rs`). The duplicate Type row is gone from shell details. Base row owns it.
- Data (`src/app/mod.rs`, `src/app/ops.rs`). Item menu has no toolbar and no dead rows. Labelled groups with shortcuts. Open on Enter bold, Rename on F2, Copy path on Ctrl+Shift+C, Properties on Alt+Enter, Delete on Del. Cut, Copy, Paste are out until a clipboard engine exists. Empty menu has a direct New folder row. Properties carries full dates, byte-count size detail, size on disk, Contains with a Calculating state fed by a background walk, attributes as real checkbox states with an Apply path that writes through `set_attr_bits` and notes elevation failures. Opens with hides for folders. Details drop the 8 promoted labels.
- OS helpers (`src/fs_ops.rs`, `src/listing.rs`). Folder walker on std::fs only, no symlink following, depth cap, cancel flag, system names skipped. Attribute reader and writer. Full-date formatter. No new deps.
- UI (`src/ui/overlay.rs`, `src/main.rs`). Menu is 272px wide, 32px rows, shortcut column, hover opens flyouts, flyout offset counts separators. Properties is 380px, 48px header icon, label grid, checkboxes with mixed state, OK, Cancel, Apply footer.
- Parent (`src/ui/mod.rs`). Menu arrow keys, Enter, and flyout-first Esc run now. Listing keys stand down while a menu is open so nothing fires twice. Stale suppression comments from the build are gone.

Measured: `cargo clippy` silent on the binary. `cargo test` 166 passed, 0 failed. Budget 9.73 MiB, PASS. `cargo fmt --check` clean.

### 8. Review round, 8 Sep 2026

User screenshots against Explorer. Four research agents, no code. Verdicts:

- Icons. Explorer draws chrome commands as Segoe Fluent Icons font glyphs and file verbs as shell rasters. Our mix of lucide outlines plus stray full-color rasters is the patched-together look. Fix is a font swap for chrome rows, codepoints verified against MS docs, with MDL2 fallback for Win10. DLL bitmap extraction is feasible but brittle and worse rendering. Per-handler exe icons are the one genuine raster gap, via `GetIconLocation` plus `ExtractIconEx` on the existing worker.
- Menu size and flyout. The offset table can never be right. Two window-snapped layers computed from one unsnapped origin, plus a 100px wrong x guess and a 7px systematic y error. Fix is anchoring the flyout to its row so layout does the math. Compact target is 224px wide, 30px rows, tighter shortcuts. Single-child Open with collapses to a direct row. Dead toolbar code goes.
- Rename and buttons. The white pill is gpui-component light-theme defaults showing through because Ply never syncs the library theme. Fix is `appearance(false)` with a Ply-owned box, mirroring the filter field. Buttons get borders, a shared helper, OK emphasis without hue. No library Button. ADR 0003 stands.
- Pins. In-memory vec reseeded every launch, no save anywhere. Fix is a line-per-path file under config dir, sync save on pin and unpin, validated load with seeds as fallback. No serde. The dep would cost hundreds of KB against the 10 MiB gate for a file with no structure.

### 9. Match Explorer icons, compact menu, rename, pins, 8 Sep 2026

Five agents, one file group each, contract up front. Parent merged and fixed two things the agents flagged.

What changed:

- Icons. Chrome rows now paint Segoe Fluent Icons glyphs that tint with the row, same family Explorer uses. Shell rasters stay primary where they exist. Open-with rows show each handler's exe icon. DLL index extraction skipped on purpose, exe art is right nearly always.
- Menu. 224px wide, 30px rows, shortcut column tightened. Flyout offset counts separators and the systematic 7px error is corrected. Groups regrouped, single-child Open with is a direct row, Pin row back so unpin is reachable. Dead toolbar struct and offset consts deleted.
- Rename. Library pill chrome stripped, Ply-owned dark box, same helper serves list and grid.
- Buttons. One shared bordered helper for both footers, OK emphasis without hue, disabled Apply look.
- Pins. Saved to `quick_access.txt` on every pin and unpin, loaded with validation. File wins exactly, so an unpinned seed stays unpinned. No serde.

Parent fixes: unpinned seeds used to merge back every launch, now the file is the truth. Deleted the toolbar model and the superseded handler list. Leftover references updated.

Measured: `cargo clippy` silent on the binary. `cargo test` 185 passed, 0 failed. Budget 9.76 MiB, PASS. `cargo fmt --check` clean.

### 10. Audit fixes, 8 Sep 2026

Audit agent found 19 issues, three more agents covered icons, sidebar, caret. Three builders, parent merged.

What changed:

- Menu rows glyph-only for Properties and Delete, matching Explorer's monochrome command rows. Rasters stay for real file and folder art. Open stays. Dropping it would cost the only Enter hint and the raster preview, and Explorer keeps it. Ever-present is the point.
- Open in Terminal and Open with offered for all files now. Copy as path always quotes, Explorer-style. Size on disk carries byte detail. Parent folder truncates instead of wrapping.
- Rename commits on Enter only, blur and Esc cancel. Stem selects without extension. Caret and selection follow the Ply palette in both modes through a library theme sync.
- Sidebar rail has its own menu per kind with no selection hijack. Volumes cannot be renamed or deleted there. Pins carry Remove, Home and Bin get nothing.
- Disabled Apply cannot fire. Dirty scrim click stays open. Volumes skip empty rows. Details scroll under a pinned footer. Buttons shrunk with a filled neutral OK.
- Sidebar active and hover split, chevron target 22px, filter placeholder sentence-case with a clear button.
- Pins persist to `quick_access.txt`, file wins, unpins stick.

Parent fixes: always-quote with test updates, sidebar wired to the new menu, placeholder wired through a re-export, toolbar model and old handler list deleted.

Measured: `cargo clippy` silent on the binary. `cargo test` 204 passed, 0 failed. Budget 9.76 MiB, PASS. `cargo fmt --check` clean.

Deferred honestly: clipboard engine for Cut, Copy, Paste; new tabs and windows need architecture; Pin to Start is blocked on Explorer identity; Compress needs a zip dep and a progress design.
