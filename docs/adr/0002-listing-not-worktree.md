# Enumerate Current Folder; do not index the Workspace

Zed keeps a scanned worktree SumTree so search and git can run. Explorer stays fast by listing the folder you are looking at. Ply is an explorer: the Tree loads directory children only when expanded; the list pane holds a full Vec of Current Folder Entries and DataTable virtualizes paint. We do not background-index the Workspace. Native watches, when added, cover Current Folder only, debounce, and skip `cx.notify()` when the Snapshot fingerprint is unchanged. Sort and IO run off the UI thread.
