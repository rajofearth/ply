# Ply

A GPUI file explorer: a Home dashboard of drives, a static sidebar, and one
location at a time. Zero-hue neutral palette, sharp corners, lucide icons.

## Run

Nightly Rust is required (GPUI uses `std::hint::cold_path`).

```powershell
cd P:\Projects\ply
cargo run
```

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

See [CONTEXT.md](CONTEXT.md) for the domain language and
[docs/adr](docs/adr) for decisions.
