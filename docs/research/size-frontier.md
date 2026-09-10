# Shrinking Ply: full research record

Date: 2026-09-09. Status: profile cut and field spike in progress, everything else decided and sequenced.
Constraints holding through all of this: thumbnails stay, app stays graphical, cross platform stays open (Windows now, macOS and Linux later), UX never pays for bytes.

## Baseline

Release binary `target/release/ply.exe` is 10,248,192 bytes, 9.77 MiB. Ceiling in `src/budget.rs` is 10 MiB. Headroom is 0.23 MiB. `cargo test budgets_report -- --nocapture` prints PASS. Non GPU working set ceiling is 100 MiB, GPU shared memory ignored by policy.

Release profile in `Cargo.toml` before this work: thin LTO, codegen units 1, strip true, opt s, panic abort. Dev keeps codegen units 16 with gpui and gpui platform at opt 3.

PE sections measured with a PE parser: .text 7,184,896, .rdata 2,541,056, .data 267,264, .pdata 136,704, .rsrc 30,208, .reloc 87,040. Code is about 70 percent, read only data about 25 percent.

Dependency counts: 700 plus locked packages, 544 nodes in the native Windows resolve, 1079 nodes with `--target all` (that number lies for Windows, it includes gpui web, linux, macos). Direct deps are 11: anyhow, dirs, gpui from zed git, gpui platform with font kit, gpui component at rev f3ba893 with no default features, notify 7, open 5, trash, chrono clock only, image with no default features, windows 0.62 with 12 features.

Source is 13,904 lines across 23 files. Biggest are `thumbs.rs`, `app/ops.rs`, `fs_ops.rs`. Assets total 2.06 MiB, icons 0.015 MiB. Icons are noise.

## What fx is

vercel-labs/fx is a coding agent harness and CLI in Zig, not a file explorer. Studied as technique, not as product. 911 files, 585 Zig files near 605k lines, version 0.0.8, minimum Zig 0.16.0. Zero package dependencies, `build.zig.zon` has empty `.dependencies`. Release binary claimed 7.8 MiB ceiling, 6.12 measured for macOS arm64. Agents scanned every file name, size, header, and top level symbol table, and read entry, loop, dispatch, workspace, and build files fully. Nobody read all 605k lines end to end.

## Why fx is small and fast, and what transfers

1. Zig with no packages and no runtime. Ply equivalent is direction only. Rust monomorphizes per crate and GPUI links regardless.
2. Terminal bytes instead of a GPU scene. `terminal_diff.zig` writes only changed cells. Zero transfer. This is the price of pixels.
3. Diffed redraws with explicit invalidation. Transfers as mindset. Ply already has Snapshot fingerprint skipping notify per ADR 0002, uniform list virtualization, storm settle 300 ms, working set locks, generation guards.
4. Hostile flags policed by CI. Transfers fully. Ply had the flags. What Ply lacked was the per PR delta warning at 52 KiB growth. Now added as `.github/workflows/size-delta.yml`.
5. Explicit allocators, arena habit. Transfers as capacity reuse and scratch buffers, not as a new allocator crate.
6. Lazy catalogs, bounded startup. Transfers fully. Matches ADR 0002, enumerate Current Folder, never index the machine. Session summary reuse maps to Snapshot equality.
7. Almost no bundled data. Two inputs total, sound recipes and Unicode tables. Transfers as cutting codec and font tables we never use, which is the fork plan below.

## Ply binary teardown, measured

String hits in ply.exe via a byte count script, low means leftover strings, high means linked code: wgpu 1, naga 4, cosmic text 0, swash 0, skrifa 0, pathfinder 0, tokio 0, rav1e 0, icu 0, rayon 3, smol 2, serde 4, tiny skia 33, resvg 38, usvg 82, rustybuzz 43, fontdb 2, ttf parser 11, image 238, exr 32, png 31, tiff 13, gif 12, html5ever 9, markdown 38, ropey 17, accesskit 16, windows 120, notify 3.

Correction that killed a whole branch of planning: wgpu, naga, cosmic text, swash, skrifa, pathfinder, tokio are all empty on the `x86_64-pc-windows-msvc` target. They appear only under `--target all` through gpui web. Windows renders through the DirectX path in gpui windows and shapes text through OS native DirectWrite. There is no DX12 only cut on Windows and no bundled shaper to subset on this target.

Real weight, largest first:

1. `image` 0.25 with 15 formats unified back on from zed gpui (bmp, dds, exr, ff, gif, hdr, ico, jpeg, png, pnm, qoi, rayon, tga, tiff, webp) even though Ply asks default features false. Ply src uses it in `thumbs.rs` and `cache.rs` only: PNG decode of own cache files, PNG encode, Triangle downscale.
2. resvg twice, 0.46.0 via gpui plus 0.45.1 via gpui component, each with own usvg and tiny skia. Ply src has no direct resvg import. Vendored lucide icons are 16 KB of pure paths, no text, no embedded rasters.
3. gpui component tail: html5ever, markdown, ropey, lsp types, rust i18n, gpui base, smol. Ply uses it for exactly one thing, `Input` in filter and rename, plus theme glue duplicating Ply own palette. Probe measured the marginal cost at 3.6 MiB for one text field.
4. windows crate four times: 0.62.2 direct, 0.56.0 via trash, 0.58.0 via component, 0.61.3 via gpui. Ply asks 12 features. Each flag has an owner today, nothing drops without a subsystem going.
5. icu plus idna plus url plus http client chain through gpui. Probe measured url plus idna plus ICU at 0.19 over empty, full http chain likely 0.4 to 0.6. A local explorer fetches nothing.
6. serde family, rayon via image and sum tree, smol via component only, accesskit for a11y which stays.
7. std floor: empty Rust binary 0.11 MiB, bare Win32 message box 0.10 MiB. no std and C rewrites chase at most 0.7 combined. Rejected.

Probe numbers, all measured release on this machine: stock GPUI hello 5.40 MiB, same hello fat LTO plus opt z 4.08 MiB (minus 24 percent, profile alone). GPUI hello plus one component Input 9.02 MiB. image full codecs 1.29 versus png ico bmp only 0.28. resvg full features 1.76 versus bare 0.78. eframe hello 3.20.

## Decisions and choices

D1. Profile to fat LTO plus opt z. Prize about 1.9 booked at 20 percent to stay conservative. Breaks nothing, doubles link time, negligible runtime cost for an IO bound explorer. Status: DONE 2026-09-09. Measured 10,248,192 to 7,883,264 bytes, 9.77 to 7.52 MiB, saved 2.26. Link took 11m03s. Full suite 210 passed. Kept.

D2. Drop gpui component, hand roll one single line field. Prize about 2.9 booked below the 3.6 measured to allow LTO interaction. Alone takes 7.8 to about 4.9. Four files touch the library. Field contract: value, set value, placeholder, focus handle, selected range, marked range for IME, EntityInputHandler for clipboard correctness, events Change, PressEnter, Blur, Focus, typing as the bare key standdown. Rename keeps stem pre selection ordering and the defer against mid callback teardown. Styling paints caret and selection from Ply palette directly, deleting the three library theme functions. Status: spike A DONE 2026-09-09. `src/field.rs` created with 6 tests green, full suite 210 passed, zero caller changes (only a `mod field;` line in main.rs, behavior neutral). Known gap: cursor walks char boundaries, not graphemes, since unicode-segmentation is transitive only. Paint half plus key bindings plus four call-site swaps remain for spike B. Update 2026-09-10, spikes B through D DONE: elements repointed, shim deleted, component dep removed. Measured 7,883,264 to 5,400,576 bytes, 7.52 to 5.15 MiB, saved 2.37. Suite 207 passed. ADR 0003 now stale where it keeps Input as the exception.

D3. Fork pinned zed, strip image codecs to png, ico, bmp. Prize about 1.0 measured. Safe because of one architectural fact: shell APIs decode user files, Ply own calls only ever see Ply written PNGs. `cache.rs` lookup gains a magic bytes debug assert. Fork cost is pin to fork rev and occasional rebase. Ply already pins, so the tax is small. Status: sequenced after D2.

D4. Fork resvg features off (text, system fonts, memmap fonts, raster images), stub the SVG font database in `svg_renderer.rs`. Prize about 1.0 measured. Deletes fontdb, rustybuzz, unicode tables from the link. Constraint documented: SVG text never renders. Nothing in product uses it. Existing icons test renders every variant and compares. Status: sequenced after D3.

D5. Gate http client, url, serde json out of GPUI in the fork. Prize 0.4 to 0.6. Most invasive fork edit for the smallest prize, so last. Breaks remote asset loads, which are unused. Status: sequenced last.

D6. Per PR size gate. `.github/workflows/size-delta.yml` added. Every PR builds release on Windows, prints MiB plus headroom to the job summary, uploads size bytes as artifact for 90 days, warns when headroom drops under 0.5 MiB, fails only through the existing 10 MiB gate with require release set. Second job runs `cargo check` on Ubuntu and macOS so the cross platform path cannot rot. CI only, zero shipped code changed, budget test still PASS. Status: done.

D7. DX12 only rejected. Reasons: nothing to cut on Windows since wgpu is already absent there, and a DX12 only mask breaks macOS and Linux by construction plus kills GLES and software fallback on weak machines. Backend selection stays per target upstream or in a patch, never as a DX12 only flag. Only if measurement shows over 1 MiB saved.

D8. Disqualified with reasons. Iced pulls wgpu plus cosmic text, same problem stacked. Slint empty quotes 3.5. Tauri needs WebView2 and fails single binary discipline. UPX lacks arm64 support and trips Windows Security. C and no std rewrites chase under 0.7 at the cost of the unsafe Win32 layer and the borrow checker. Raw winit scene rewrites layout, text, and a11y to save what the fork already saves. egui port at 3.2 measured stays as insurance if the fork stalls, never the plan.

D9. Sub 1MB verdict. Real only if the OS carries rendering, text, decoding, and thumbnail caching: native shells per OS plus one std only shared core, estimated 350 to 600 KB per OS, thumbnails free via IShellItemImageFactory, QuickLook, freedesktop cache. Alternate is Zig or C core at 100 to 600 KB with a second language in repo. Software raster path lands 2 to 4 and misses. TUI near 100 KB breaks graphical thumbnails and the product. Deferred until someone commits to three shells. Not this pass.

D10. RAM and speed program. Verdict first: total under 100 MiB including GPU is not real while keeping GPU compositing. Ship the 100 MiB non GPU gate as is, track total as info. Realistic: 40 to 70 non GPU idle on Home, 60 to 90 in a 10k folder, 180 to 280 total with GPU. Ranked changes: decode plus resize plus RenderImage build off the UI thread; split tiles 96 grid versus 32 to 48 list and sidebar; budget 8 to 4 MiB with locks 128 to 64 and flush live tiles on idle only; filter zero alloc compare plus 60 to 120 ms debounce; scratch vecs plus index select plus memo size and time for flat per frame allocs; icon batch 512 to 128 plus page on scroll; disk cache 128 to 32 to 64 MiB with background evict; gate volume, lnk, and watch timers when idle; defer Fluent font chain and add CLI early out before GPU init. Bench to add: 10k list sort, keystroke under 16 ms with zero per entry String alloc, 100 frames flat allocs, storm capped, same snapshot reload issues no notify.

## Target math, MiB, conservative

9.77 baseline. Minus 1.9 profile is 7.8. Minus 2.9 component cut is 4.9, under budget with no margin. Minus 1.0 codecs is 3.9. Minus 1.0 resvg fonts is 2.9. Minus 0.5 http chain is 2.4. Table scraps (regex subsets, uuid, chrono) take roughly 2.1. Half delivery of the fork still lands near 3.5.

## Sequencing

Profile now, component cut next since it alone nearly closes the gap, fork cuts after in prize order, http gating last. RAM and speed changes interleave with the field work since they touch different functions. One writer per hot file throughout: Cargo.toml, `src/field.rs`, `app/mod.rs` plus `ops.rs`, `ui/status.rs` plus `browser.rs`, `cache.rs` plus thumbs fit path, then fork files.

## Open questions owned by user

Q1. Fork threshold: 1 MiB measured saving or stop. Q2. First non Windows thumbnail backend when the trait lands: macOS or Linux.
