# Settled base and efficiency pass

Tracks the cleanup and feel work in
[epic #3](https://github.com/rajofearth/ply/issues/3). Style bias: deep modules
with small public surfaces, Theo/Matt simplicity — no hexagonal ports, no
plugin walls, no crates for ≤100 LOC of local code.

**Properties.** Built from a real `Entry` or `Volume`, never a dummy `Entry`
with an empty name. The dialog field is **Path**, not Location (Location stays
nav vocabulary).

**Portable paths.** A small `PathCaps` gate decides rename / trash / reveal /
watch. Unsupported actions stay off the menu. MTP feature work (cheaper poll,
batch props, stream fetch, writes) is deferred — issue C under the epic.

**App shape.** `app.rs` splits into `app/{mod,nav,ops}` — move-only, same `Ply`
methods. Keep `listing` and `fs_ops` separate (read vs mutate).

**Listing.** On Windows, build Entries from `DirEntry` metadata / attribute
bits; do not re-stat every child. Only `read_link` when the entry is a reparse
point.

**List view.** Use GPUI `uniform_list` for fixed-height rows. Grid stays
flex-wrap until it hurts.

**Volume poll.** Gate rediscovery on the `GetLogicalDrives` mask; keep MTP off
the drive poll path so a quiet machine is not re-serializing WPD every tick.
