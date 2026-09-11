# Ply engineering philosophy

Ply is a file explorer, not a coding agent. Most of what keeps it small and fast
came from studying one that is. vercel-labs/fx is a coding agent harness and CLI
written in Zig. 585 source files, near 605k lines, zero package dependencies, a
terminal UI that diffs changed cells, and a release binary measured at 6.12 MiB
against a claimed 7.8 MiB ceiling. It warns on every pull request when the binary
grows past 52 KiB.

We studied fx as technique, not as product. Nobody needs a second one. What
transferred was discipline rather than code, and this doc records it with the Ply
numbers behind it. Every claim here traces to `docs/research/size-frontier.md`,
an ADR, or `src/budget.rs`.

## The smallest binary is the one you do not link

Ply's release binary went from 9.77 MiB to 5.15 MiB in two cuts. Neither was a
rewrite.

The first was the release profile. Fat LTO, `opt-level = "z"`, one codegen unit,
strip, `panic = "abort"` took 9.77 to 7.52 MiB and saved 2.26. Link time went to
11 minutes. For an IO-bound explorer that is a fine trade.

The second was deleting gpui-component. Ply used it for exactly one thing, a text
field. That field had a measured marginal cost of 3.6 MiB, because the library
drags in html5ever, markdown, ropey, lsp types, and a second copy of resvg.
Hand-rolling the field took 7.52 to 5.15 and saved 2.37.

A `default-features = false` line was not enough. Zed's gpui re-enables all
fifteen of `image`'s codecs no matter what Ply asks for. When Ply moved its own
PNG calls to the `png` crate directly, the measured change was +1,536 bytes.
Effectively zero. That null result is the lesson. Feature unification means your
own feature flags lie to you. Check `cargo tree` before you believe the manifest.

## Let the OS do the hard work

Ply decodes and shapes as little as it can. Thumbnails come from the Windows
shell. Text comes from DirectWrite through gpui_platform's font-kit. On the
Windows target, wgpu, naga, cosmic-text, swash, skrifa, pathfinder, and tokio all
measure empty. They appear only under `--target all`, pulled in by gpui's web
path. There is no shaper to subset and no DX12-only cut to make on Windows.

fx makes the same bet from the other side. Its terminal renders, and it writes
only the cells that changed. If the OS already solved the problem, call the OS.

## Enumerate what you show, never index the machine

ADR 0002. Ply lists the folder you are looking at and stops there. Sidebar
branches load subfolders only when you expand the chevron. There is no background
scan of a drive, no worktree, no index.

fx keeps a lazy catalog. Ply keeps none. The watch covers the Current Folder
only, ignores atime events so reading a folder cannot retrigger itself, and skips
`cx.notify()` when the Snapshot fingerprint has not changed. Directory IO and
volume discovery run off the UI thread, because enumerating volumes can block on
a network share that never answers.

## Paint only what changed

fx diffs terminal cells. Ply cannot diff a GPU scene at that granularity, so it
applies the same idea one layer up. `uniform_list` virtualizes fixed-height rows.
The Snapshot fingerprint stops a quiet reload from repainting at all. Working-set
locks and generation guards stop a fast scroll from rebuilding tiles it is about
to discard. Storm settle is 300 ms and coalesce is 16 ms. Decode, resize, and
RenderImage build stay off the UI thread.

## Measure, then decide

There is no optimization in Ply without a number.

The hard ceiling is 10 MiB, in `src/budget.rs`. `cargo test budgets_report --
--nocapture` prints the release size and fails the suite when it is over. At
baseline the headroom was 0.23 MiB, and that is what started the size work.

`.github/workflows/size-delta.yml` builds release on every PR, reports MiB and
headroom, warns when headroom drops under 0.5 MiB, and uploads the byte count as
an artifact. The component cut was booked at 2.9 against a measured 3.6,
deliberately conservative so LTO interaction could not turn the win into a
surprise. The PNG-only change that bought nothing is in the record too. A
measurement that says no is still a result.

## Small core, thin shells

Keep platform-specific code behind a trait, and give unsupported platforms a
stub that compiles.

Ply is real on Windows. macOS and Linux are open but not yet built. When the
thumbnail backend lands as a trait, Windows implements it with the shell and the
other targets get stubs, not half ports. CI already runs `cargo check
--all-targets` on Ubuntu and macOS so those stubs cannot rot. The recommendation
on file is Linux first, because the freedesktop thumbnail spec is documented and
testable without Apple hardware.

## Where Rust differs from Zig

Not everything transfers, and pretending otherwise would be dishonest.

Zig monomorphizes nothing by default. Rust monomorphizes per crate, so generics
over a large dependency tree inflate `.text`, and gpui links whether or not Ply
touches most of it. Feature unification is a Cargo behavior, which is why
`image`'s codecs came back and why those cuts need a fork rather than a flag.
Zig's zero dependencies come from carrying no package graph at all. Ply cannot
match that while the UI is gpui.

What transfers is the approach. Diffed redraws, hostile flags policed by CI,
explicit capacity reuse, bounded lazy work. What does not transfer is the 100 KB
floor. Ply is a graphical explorer on a GPU framework, and that floor belongs to
a different kind of program.

## What we deliberately did not do

No fork of zed for now. The rule was 1 MiB measured saving or stop. The image
fork's Ply-side change measured zero, so we banked 5.15 MiB and revisit only if
headroom pressure returns.

No DX12-only build. wgpu is already absent on Windows, so there is nothing to
cut, and the mask would break macOS and Linux and kill GLES and software
fallback. It would only be worth revisiting above 1 MiB.

No UPX. It lacks arm64 support and trips Windows Security.

No native rewrite and no allocator crate. A C or no-std rewrite chases under
0.7 MiB while giving up the borrow checker and the safe Win32 layer. fx's
explicit allocators transfer as capacity reuse and scratch buffers, not as a new
dependency.

egui stays as insurance. It measured 3.2 MiB, it is not the plan, and it only
gets a look if the gpui route stalls.

## Adding a feature

Measure first. Build release and run `cargo test budgets_report -- --nocapture`
before and after. A change with no number is a guess.

Prefer the OS. If the platform already does the job, call it and ship no bytes.

Hand-roll before you depend. A text field cost 3.6 MiB as a dependency. Local
code under a hundred lines does not need a crate.

Gate it. Put the ceiling in `src/budget.rs` and let the size workflow report the
delta on the PR. If the number does not justify the change, do not ship the
change.
