# Win11-style single context menu; PathCaps decide visibility

The shell menu is one surface: a toolbar of icon commands, then a vertical
list. There is no "Show more options" overflow. The toolbar is present only
when the click landed on an Entry; empty-space clicks get the list alone.

Icons are monochrome lucide glyphs. Delete is the only command that uses the
`destructive` token.

Toolbar contents are fixed then smart-promoted:

- Always (when possible): Cut, Copy, Rename, Delete. Multi-select drops Rename.
- Single file: promote Open; exe/scripts also promote Run as administrator.
- Single folder: promote Open in Terminal.
- Multi-select: no promote.

List contents are locked in the explorer-shell epic. Compress, Share, Group by,
and Select all stay out of the menu. Pin labels are "Add to Quick Access" /
"Remove from Quick Access".

**PathCaps** (hybrid): a command that can never apply to this path is omitted
(MTP writes, renaming a Volume). A command that applies but is idle is shown
disabled (Paste with an empty file clipboard).

File Cut/Copy/Paste talk to the OS clipboard so other apps can paste; Copy path
is a separate list row and a separate shortcut.
