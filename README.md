# Ply

A read-only GPUI file explorer: one Workspace, a directory Tree, a Current Folder table, and a Preview.

## Run

Nightly Rust is required (GPUI uses `std::hint::cold_path`).

```powershell
cd P:\Projects\ply
cargo run
```

- **F5** refresh
- **Ctrl+O** open folder (becomes the Workspace)
- **Enter** open (folder → Current Folder, file → OS)
- **Alt+Up** / Backspace parent
- **Ctrl+C** copy path
- Column headers sort the list
- Filter box matches names in Current Folder only

See [CONTEXT.md](CONTEXT.md) for the domain language.
