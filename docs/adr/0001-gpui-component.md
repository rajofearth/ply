# Pin GPUI through gpui-component

Ply is a GPUI desktop app whose chrome should look like Zeron (dense, dark, solid surfaces) without vendoring Zed. We depend on `gpui-component` (and its pinned `gpui` git revision) for Root, Resizable, Tree, DataTable, Menu, Input, Switch, Alert, and Icon, then override theme tokens. Hand-rolling those controls would delay the explorer slice; copying Zed crates would pin us to an editor, not a file manager.
