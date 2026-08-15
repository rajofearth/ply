# Ply

A read-only desktop file explorer: one Workspace, a directory Tree, a Current Folder listing, and a Preview.

## Language

**Workspace**:
The single folder Ply is allowed to walk. The tree is descendants of this folder.
_Avoid_: root, project, volume, This PC

**Current Folder**:
The directory whose children appear in the list pane. Set by choosing a directory in the Tree or opening a directory in the list.
_Avoid_: cwd, selected folder, path (alone)

**Entry**:
A file, directory, or symlink listed under the Workspace.
_Avoid_: item, node, inode, document

**Selected Entry**:
The list-pane row Preview follows. Independent of Current Folder.
_Avoid_: focus, highlight, current file

**Preview**:
The right pane for the Selected Entry: metadata always; UTF-8 text or image when cheap to produce off the UI thread.
_Avoid_: viewer, editor, details pane

**Open**:
Hand the Selected Entry to the OS default app (file) or set Current Folder (directory). Not Preview.
_Avoid_: launch, execute, edit

**Listing**:
The ordered vector of Entries in Current Folder. Source of truth for the list pane and column sort.
_Avoid_: cache, index, worktree

**Snapshot**:
A Listing plus a fingerprint (names, kinds, sizes, mtimes, attrs). Equal snapshots must not notify the UI.
_Avoid_: state, cache

**Hidden Entry**:
An Entry with the platform hidden attribute or a name starting with `.`. Hidden unless Show Hidden is on.
_Avoid_: dotfile, system file
