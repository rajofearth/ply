# AGENTS.md

## Cursor Cloud specific instructions

Ply is a **native GPUI desktop app** (a read-only file explorer), not a web/server app. It
was originally developed on Windows (see `README.md`), so a few Linux-specific caveats apply
when building, running, or testing in the Cloud environment.

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
- The app defaults its Workspace to the home directory. Use double-click on a folder row (or a
  Tree click) to navigate; single-click a file to preview it. See `README.md` for keybindings.

### Lint / test / build
- Format check: `cargo fmt --check` (repo currently has some pre-existing formatting diffs).
- Lint: `cargo clippy` (currently emits pre-existing warnings only).
- Tests: `cargo test` — there are currently **no** unit/integration tests in the repo (0 tests).
- Build (dev): `cargo build --features gpui_platform/x11`.
