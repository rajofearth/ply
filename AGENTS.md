# AGENTS.md

## Cursor Cloud specific instructions

Ply is a **native GPUI desktop app** (a read-only File Explorer shell), not a web/server app.
It was originally developed on Windows (see `README.md`); Linux Cloud agents need the notes below.

### Toolchain
- Nightly Rust is required and is pinned via `rust-toolchain.toml` (uses `std::hint::cold_path`);
  no manual toolchain selection is needed.

### Linux windowing feature is NOT enabled by default (important)
- `Cargo.toml` depends on `gpui_platform` with only `features = ["font-kit"]`. The `x11` and
  `wayland` features are **not** in `gpui_platform`'s defaults, so a plain `cargo run`/`cargo build`
  produces a binary that panics at startup on Linux:
  `internal error: ... At least one of the "wayland" or "x11" features must be enabled ...`.
- On Linux you MUST enable a windowing backend at the command line (do not edit `Cargo.toml`):
  `cargo run --features gpui_platform/x11`. This flag is a no-op on Windows/macOS.

### Running the GUI (headless VM)
- There is no GPU, so GPUI's Vulkan renderer must use the Mesa **lavapipe** software driver.
- An X11 display (`:1`) is provided by the computer-use desktop. Required env vars to run the app:
  - `DISPLAY=:1`
  - `XDG_RUNTIME_DIR=/tmp/xdg-runtime` (create it, `chmod 700`)
  - `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json` (forces software Vulkan)
- Full run command:
  `DISPLAY=:1 XDG_RUNTIME_DIR=/tmp/xdg-runtime VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo run --features gpui_platform/x11`
- Keep the window at ~1280×800 when automating clicks; a collapsed/tiny window makes hit-testing fail.

### UI shell (post File Explorer redesign)
- Boots on **Home**: drive cards with capacity bars, Devices & network, Quick access pins.
- Opening a drive/pin browses the real filesystem (read-only). No separate Preview pane — use
  **Properties** (`Alt+Enter` or context menu) for metadata.
- Useful keys: `d` theme, `Ctrl+1`/`Ctrl+2` list/grid, `Alt+Home` Home, `Enter` open selection,
  `F5` refresh. Filter lives in the status bar.
- Folder watches ignore Access/atime events (readdir used to cause an infinite reload loop).

### Lint / test / build
- Format: `cargo fmt --check`
- Lint: `cargo clippy`
- Tests: `cargo test --features gpui_platform/x11` (unit tests for listing/sidebar/volumes/watch)
- Build (dev): `cargo build --features gpui_platform/x11`
