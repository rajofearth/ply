# Ply

A desktop file explorer: a Home dashboard of Volumes, a static Sidebar, and one
Location at a time in the centre pane.

## Language

**Location**:
What the centre pane is pointed at — either Home or a Current Folder. Navigation
history is a stack of Locations.
_Avoid_: page, route, view (view means list-vs-grid)

**Home**:
The idle Location. Shows Volumes as capacity cards and nothing else — no
listing, no status bar.
_Avoid_: dashboard, start page, This PC

**Current Folder**:
The directory whose children fill the centre pane. Reached from a Volume, the
Sidebar, a breadcrumb, or by opening a folder in the listing.
_Avoid_: cwd, path (alone), workspace

**Volume**:
A drive, removable device, or network share, with a name and capacity. Grouped
as Drives versus Devices & network.
_Avoid_: disk, mount, partition

**Entry**:
A file, directory, or symlink inside the Current Folder.
_Avoid_: item, node, inode, document

**Listing**:
The ordered vector of Entries in the Current Folder: directories first, then
name. Source of truth for both list and grid.
_Avoid_: cache, index, worktree

**Snapshot**:
A Listing plus a fingerprint (names, kinds, sizes, mtimes, attrs). Equal
fingerprints must not replace the Listing, so watch-driven reloads stay quiet.
_Avoid_: state, cache

**Selection**:
The set of selected Entries. Plain click replaces it, ctrl-click toggles one,
shift-click and shift-arrow extend from the anchor.
_Avoid_: focus, highlight, current file

**Sidebar**:
The fixed left rail: Home and pinned folders, This PC, Devices & network. It is
loosely coupled to the centre pane — it highlights the Current Folder only if
that row already happens to be visible, and **never auto-expands to reveal it**.
_Avoid_: tree pane, navigator

**Quick Access**:
Folders pinned under Home in the Sidebar. Seeded from the user's shell folders;
the user adds more by dragging a folder onto the section.
_Avoid_: favourites, bookmarks, shortcuts

**Expand**:
Opening a Sidebar branch via its chevron, which lazily lists that folder's
subfolders. Only the chevron expands; navigating never does.
_Avoid_: drill down, open (open means Location change)

**Open**:
Hand an Entry to the OS default app (file) or make it the Current Folder
(directory).
_Avoid_: launch, execute, preview

**Properties**:
The modal showing one Entry's or Volume's type, size, mtime, and location. It
replaced the old preview pane; Ply never renders file contents.
_Avoid_: details pane, inspector, preview

**Hidden Entry**:
An Entry with the platform hidden attribute or a leading `.`. Always excluded
from the Listing — there is no Show Hidden control.
_Avoid_: dotfile, system file

## Design tokens

Zero-hue neutral scale, `radius: 0`, native UI font. Selection and active states
are carried by contrast and weight, never colour. The only hue in the palette is
`destructive`, used solely for Delete. Tokens are authored in OKLCH and
pre-converted to sRGB in `src/theme.rs`, which records the OKLCH source
alongside each value.
