# Ply agent notes

A GPUI desktop file explorer. Read [CONTEXT.md](CONTEXT.md) for domain language
and [docs/adr](docs/adr) for decisions. Do not invent terms.

## Design (locked)

- Zero-hue neutrals, `radius: 0`, native UI font. Tokens live in `src/theme.rs`.
- The only hue is `destructive`, used solely for Delete.
- Win11-style **single** context menu: optional toolbar + vertical list.
  Never "Show more options". Toolbar only on an Entry right-click; empty
  space is list-only.
- Monochrome lucide icons. PathCaps: omit the impossible; grey out the
  temporarily unavailable (empty Paste).
- Delete is recycle-bin only (`src/fs_ops.rs`). Never permanent delete.
- MTP growth is deferred (issue #15). Do not expand the WPD surface.
- Do not invent Compress, Share, Group by, or Select all in menus.

## Commands

```
cargo test
cargo test budgets_report -- --nocapture
cargo run
```

Nightly is required (`rust-toolchain.toml`). Obey the budget gate in
`src/budget.rs`: measure before performance patches; do not "optimise"
without a number.

## Layout

| Path | Role |
| --- | --- |
| `src/app/` | Window state, tabs, menu actions |
| `src/app/menu.rs` | Menu chrome model (toolbar + rows) |
| `src/app/ops.rs` | Side-effecting commands |
| `src/path_caps.rs` | Possible / unavailable / impossible |
| `src/file_clip.rs` | OS file clipboard (Cut/Copy/Paste) |
| `src/open_with.rs` | Known apps + native picker |
| `src/preview.rs` | Quick Look payloads |
| `src/ui/overlay.rs` | Menu, Properties, Quick Look |
| `src/ui/browser.rs` | List / Grid / Column |
| `CONTEXT.md` | Language. Update it when a term changes. |
