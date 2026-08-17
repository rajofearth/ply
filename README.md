# Ply

A GPUI file explorer: a Home dashboard of drives, a static sidebar, and one
location at a time. Zero-hue neutral palette, sharp corners, lucide icons.

## Run

Nightly Rust is required (GPUI uses `std::hint::cold_path`).

```
cargo run
```

## Keys

Shortcuts follow the host OS (Cmd on macOS, Ctrl elsewhere). Bare-key shortcuts
stand down while a text field has focus.

| Action | Windows / Linux | macOS |
| --- | --- | --- |
| Open | Enter | ⌘O / ⌘↓ |
| Rename | F2 | Return |
| Cut / Copy / Paste files | Ctrl+X / C / V | ⌘X / C / V |
| Copy path | Ctrl+Shift+C | ⌘⇧C |
| Quick Look | Space | Space |
| Delete (Recycle Bin) | Delete | ⌘⌫ |
| New folder | Ctrl+Shift+N | ⌘⇧N |
| New tab | Ctrl+T | ⌘T |
| New window | Ctrl+N | ⌘N |
| Refresh | F5 | ⌘R |
| Focus filter | Ctrl+F | ⌘F |
| Back | Alt+← / Backspace | ⌘[ / Backspace |
| Forward | Alt+→ | ⌘] |
| Parent folder | Alt+↑ | ⌘↑ |
| Home | Alt+Home | ⌘⇧H |
| Toggle light / dark | `d` | `d` |
| Close topmost (dialog → Quick Look → menu → rename → selection) | Esc | Esc |

## Mouse

- Click to select, `Ctrl`+click (`⌘`+click) to toggle, `Shift`+click to extend.
- Double-click to open. Right-click for the context menu (toolbar on an Entry;
  list only on empty space).
- Drag a folder onto the sidebar's Home section to pin it.
- Sidebar branches expand **only** via the chevron — navigating never expands
  the tree.

## Writes

Rename, Delete, Cut, Copy, Paste, and New are real. Delete goes to the platform
recycle bin and never deletes permanently.

See [CONTEXT.md](CONTEXT.md) for the domain language,
[AGENTS.md](AGENTS.md) for contributor constraints, and
[docs/adr](docs/adr) for decisions.
