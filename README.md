# Ply

A read-only GPUI file explorer with a Home / This PC shell: drive cards, quick access,
a directory sidebar, and list or grid browsing. Metadata lives in a Properties modal
(no separate Preview pane).

## Run

Nightly Rust is required (GPUI uses `std::hint::cold_path`).

```powershell
cd P:\Projects\ply
cargo run
```

On Linux, enable an X11 (or Wayland) backend:

```bash
cargo run --features gpui_platform/x11
```

## Keys

- **F5** refresh
- **Ctrl+O** open folder (new Workspace root)
- **Alt+Home** Home view
- **Alt+Left** / **Backspace** back · **Alt+Right** forward
- **Enter** open selection (folder → browse, file → OS)
- **Alt+Enter** Properties
- **Ctrl+1** / **Ctrl+2** list / grid
- **d** toggle light/dark theme
- **Ctrl+C** copy path(s)
- **Ctrl+H** show hidden
- Column headers sort the list; status-bar filter matches names in Current Folder only

See [CONTEXT.md](CONTEXT.md) for the domain language.
