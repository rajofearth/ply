# Pin GPUI through gpui-component

> Largely superseded by [0003](0003-hand-rolled-explorer-shell.md): the shell is
> now hand-rolled and only `Input` is still used from gpui-component. We keep
> depending on it for the pinned `gpui` revision (icons are vendored; see 0003).

Ply is a GPUI desktop app whose chrome should look like Zeron (dense, dark, solid surfaces) without vendoring Zed. We depend on `gpui-component` (and its pinned `gpui` git revision) for Root, Resizable, Tree, DataTable, Menu, Input, Switch, Alert, and Icon, then override theme tokens. Hand-rolling those controls would delay the explorer slice; copying Zed crates would pin us to an editor, not a file manager.
