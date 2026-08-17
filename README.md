# Ply

A GPUI file explorer: a Home of Volumes, a static sidebar, and one location at
a time. Zero-hue neutral palette, sharp corners, lucide icons. Properties is a
modal (no preview pane). MTP devices are supported but limited — browse and
open where the OS allows; richer MTP work is deferred. Release builds use a
size-minded profile (thin LTO, size opts, strip); GPUI still dominates binary
size and RAM.

## Commands

Nightly Rust is required (GPUI uses `std::hint::cold_path`).

| Command | Does |
| --- | --- |
| `cargo run` | Run the explorer (debug) |
| `cargo run --release` | Run the size-minded release build |
| `cargo build --release` | Build the release binary at `target/release/` |
| `cargo check` | Fast type-check |
| `cargo test` | Full suite |
| `cargo test budgets_report -- --nocapture` | Print the binary / RAM budgets and gate the release size |
| `cargo fmt` / `cargo clippy` | Format / lint |

The budget test fails the suite when a release binary exists and exceeds the
size ceiling; set `PLY_BUDGET_REQUIRE_RELEASE=1` to also fail when no release
binary is built yet.

## Keys

| Key | Action |
| --- | --- |
| `d` | Toggle light / dark |
| `Enter` | Open (folder → navigate, file → OS default app) |
| `↑` / `↓` | Move selection |
| `Shift+↑` / `Shift+↓` | Extend selection |
| `Alt+←` / `Backspace` | Back |
| `Alt+→` | Forward |
| `Alt+↑` | Parent folder |
| `Alt+Home` | Home |
| `F2` | Rename |
| `Delete` | Move to Recycle Bin |
| `Ctrl+C` | Copy path |
| `Ctrl+F` | Focus the filter |
| `F5` | Refresh |
| `Esc` | Close the topmost thing (dialog → menu → rename → selection) |

Bare-key shortcuts stand down while a text field has focus.

## Mouse

- Click to select, `Ctrl`+click to toggle, `Shift`+click to extend.
- Double-click to open. Right-click for the context menu.
- Drag a folder onto the sidebar's Home section to pin it.
- Sidebar branches expand **only** via the chevron — navigating never expands
  the tree.

## Writes

Ply is mostly read-only, but Rename and Delete are real. Delete goes to the
platform recycle bin and never deletes permanently.

See [CONTEXT.md](CONTEXT.md) for the domain language,
[AGENTS.md](AGENTS.md) for how coding agents should work in this repo, and
[docs/adr](docs/adr) for decisions.
