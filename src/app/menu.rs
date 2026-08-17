//! Win11-style menu model: optional toolbar + vertical list, PathCaps-driven.

use std::path::PathBuf;

use gpui::{Pixels, Point, SharedString};

use crate::icons::Ico;
use crate::listing::{SortDir, SortKey, SortSpec, ViewMode};
use crate::open_with::AppHandler;
use crate::path_caps::PathCaps;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuKind {
    Entry,
    Empty,
}

pub struct Menu {
    pub at: Point<Pixels>,
    pub kind: MenuKind,
    pub toolbar: Vec<ToolBtn>,
    pub rows: Vec<MenuRow>,
    /// Index of the row whose submenu is open.
    pub flyout: Option<usize>,
}

#[derive(Clone)]
pub struct ToolBtn {
    pub icon: Ico,
    pub action: MenuAction,
    pub enabled: bool,
    pub danger: bool,
}

#[derive(Clone)]
pub enum MenuRow {
    Separator,
    Item(MenuItem),
}

#[derive(Clone)]
pub struct MenuItem {
    pub label: SharedString,
    pub icon: Option<Ico>,
    pub action: Option<MenuAction>,
    pub children: Vec<MenuRow>,
    pub enabled: bool,
    pub danger: bool,
}

#[derive(Clone)]
pub enum MenuAction {
    Open(PathBuf),
    OpenWith { path: PathBuf, app: AppHandler },
    ChooseApp(PathBuf),
    RunAsAdmin(PathBuf),
    OpenInTerminal(PathBuf),
    OpenInNewTab(PathBuf),
    OpenInNewWindow(PathBuf),
    Pin(PathBuf),
    Unpin(PathBuf),
    CopyPath(Vec<PathBuf>),
    Cut(Vec<PathBuf>),
    Copy(Vec<PathBuf>),
    Paste,
    Rename(PathBuf),
    Delete(Vec<PathBuf>),
    Properties(PathBuf),
    Refresh,
    SetView(ViewMode),
    SetSort(SortSpec),
    NewFolder,
    NewFile,
}

pub struct EntrySpec {
    pub path: PathBuf,
    pub targets: Vec<PathBuf>,
    pub caps: PathCaps,
    pub is_dir: bool,
    pub is_file: bool,
    pub handlers: Vec<AppHandler>,
}

pub struct EmptySpec {
    pub folder: PathBuf,
    pub caps: PathCaps,
    pub view: ViewMode,
    pub sort: SortSpec,
}

pub fn build_entry(at: Point<Pixels>, spec: EntrySpec) -> Menu {
    let toolbar = entry_toolbar(&spec);
    let rows = entry_rows(&spec);
    Menu {
        at,
        kind: MenuKind::Entry,
        toolbar,
        rows,
        flyout: None,
    }
}

pub fn build_empty(at: Point<Pixels>, spec: EmptySpec) -> Menu {
    Menu {
        at,
        kind: MenuKind::Empty,
        toolbar: Vec::new(),
        rows: empty_rows(&spec),
        flyout: None,
    }
}

fn entry_toolbar(spec: &EntrySpec) -> Vec<ToolBtn> {
    let mut tools = Vec::new();
    let multi = spec.targets.len() > 1;
    if !multi && spec.is_file && spec.caps.open.enable() {
        tools.push(tool(
            Ico::ExternalLink,
            MenuAction::Open(spec.path.clone()),
            true,
            false,
        ));
        if spec.caps.run_as_admin.enable() {
            tools.push(tool(
                Ico::Shield,
                MenuAction::RunAsAdmin(spec.path.clone()),
                true,
                false,
            ));
        }
    }
    if !multi && spec.is_dir && spec.caps.terminal.enable() {
        tools.push(tool(
            Ico::Terminal,
            MenuAction::OpenInTerminal(spec.path.clone()),
            true,
            false,
        ));
    }
    if spec.caps.cut.show() {
        tools.push(tool(
            Ico::Scissors,
            MenuAction::Cut(spec.targets.clone()),
            spec.caps.cut.enable(),
            false,
        ));
    }
    if spec.caps.copy.show() {
        tools.push(tool(
            Ico::Copy,
            MenuAction::Copy(spec.targets.clone()),
            spec.caps.copy.enable(),
            false,
        ));
    }
    if spec.caps.rename.show() {
        tools.push(tool(
            Ico::Pencil,
            MenuAction::Rename(spec.path.clone()),
            spec.caps.rename.enable(),
            false,
        ));
    }
    if spec.caps.delete.show() {
        tools.push(tool(
            Ico::Trash,
            MenuAction::Delete(spec.targets.clone()),
            spec.caps.delete.enable(),
            true,
        ));
    }
    tools
}

fn entry_rows(spec: &EntrySpec) -> Vec<MenuRow> {
    let mut rows = Vec::new();
    if spec.caps.open.show() {
        rows.push(item(
            "Open",
            Some(Ico::ExternalLink),
            Some(MenuAction::Open(spec.path.clone())),
            spec.caps.open.enable(),
            false,
        ));
    }
    if spec.caps.open_with.show() {
        let mut children: Vec<MenuRow> = spec
            .handlers
            .iter()
            .map(|app| {
                item(
                    app.name.clone(),
                    None,
                    Some(MenuAction::OpenWith {
                        path: spec.path.clone(),
                        app: app.clone(),
                    }),
                    true,
                    false,
                )
            })
            .collect();
        children.push(item(
            "Choose another app…",
            None,
            Some(MenuAction::ChooseApp(spec.path.clone())),
            true,
            false,
        ));
        rows.push(MenuRow::Item(MenuItem {
            label: "Open with".into(),
            icon: Some(Ico::AppWindow),
            action: None,
            children,
            enabled: spec.caps.open_with.enable(),
            danger: false,
        }));
    }
    if spec.caps.run_as_admin.show() {
        rows.push(item(
            "Run as administrator",
            Some(Ico::Shield),
            Some(MenuAction::RunAsAdmin(spec.path.clone())),
            spec.caps.run_as_admin.enable(),
            false,
        ));
    }
    if spec.caps.terminal.show() {
        rows.push(item(
            "Open in Terminal",
            Some(Ico::Terminal),
            Some(MenuAction::OpenInTerminal(spec.path.clone())),
            spec.caps.terminal.enable(),
            false,
        ));
    }
    if spec.caps.new_tab.show() {
        rows.push(item(
            "Open in new tab",
            None,
            Some(MenuAction::OpenInNewTab(spec.path.clone())),
            spec.caps.new_tab.enable(),
            false,
        ));
    }
    if spec.caps.new_window.show() {
        rows.push(item(
            "Open in new window",
            Some(Ico::AppWindow),
            Some(MenuAction::OpenInNewWindow(spec.path.clone())),
            spec.caps.new_window.enable(),
            false,
        ));
    }
    if spec.caps.pin.show() {
        rows.push(item(
            "Add to Quick Access",
            Some(Ico::Pin),
            Some(MenuAction::Pin(spec.path.clone())),
            spec.caps.pin.enable(),
            false,
        ));
    }
    if spec.caps.unpin.show() {
        rows.push(item(
            "Remove from Quick Access",
            Some(Ico::PinOff),
            Some(MenuAction::Unpin(spec.path.clone())),
            spec.caps.unpin.enable(),
            false,
        ));
    }
    if spec.caps.copy_path.show() {
        rows.push(item(
            "Copy path",
            None,
            Some(MenuAction::CopyPath(spec.targets.clone())),
            spec.caps.copy_path.enable(),
            false,
        ));
    }
    if spec.caps.properties.show() {
        rows.push(item(
            "Properties",
            Some(Ico::Info),
            Some(MenuAction::Properties(spec.path.clone())),
            spec.caps.properties.enable(),
            false,
        ));
    }
    if spec.caps.delete.show() {
        let label = if spec.targets.len() > 1 {
            format!("Delete {} items", spec.targets.len())
        } else {
            "Delete".into()
        };
        rows.push(item(
            label,
            Some(Ico::Trash),
            Some(MenuAction::Delete(spec.targets.clone())),
            spec.caps.delete.enable(),
            true,
        ));
    }
    rows
}

fn empty_rows(spec: &EmptySpec) -> Vec<MenuRow> {
    let mut rows = Vec::new();
    if spec.caps.view.show() {
        rows.push(MenuRow::Item(MenuItem {
            label: "View".into(),
            icon: None,
            action: None,
            enabled: spec.caps.view.enable(),
            danger: false,
            children: vec![
                view_item("List", ViewMode::List, spec.view),
                view_item("Grid", ViewMode::Grid, spec.view),
                view_item("Column", ViewMode::Column, spec.view),
            ],
        }));
    }
    if spec.caps.sort.show() {
        rows.push(MenuRow::Item(MenuItem {
            label: "Sort by".into(),
            icon: None,
            action: None,
            enabled: spec.caps.sort.enable(),
            danger: false,
            children: vec![
                sort_item("Name", SortKey::Name, spec.sort),
                sort_item("Date modified", SortKey::Modified, spec.sort),
                sort_item("Type", SortKey::Kind, spec.sort),
                sort_item("Size", SortKey::Size, spec.sort),
            ],
        }));
    }
    if spec.caps.new_item.show() {
        rows.push(MenuRow::Item(MenuItem {
            label: "New".into(),
            icon: Some(Ico::FilePlus),
            action: None,
            enabled: spec.caps.new_item.enable(),
            danger: false,
            children: vec![
                item(
                    "Folder",
                    Some(Ico::FolderPlus),
                    Some(MenuAction::NewFolder),
                    true,
                    false,
                ),
                item(
                    "Text Document",
                    Some(Ico::FileText),
                    Some(MenuAction::NewFile),
                    true,
                    false,
                ),
            ],
        }));
    }
    if spec.caps.paste.show() {
        rows.push(item(
            "Paste",
            Some(Ico::ClipboardPaste),
            Some(MenuAction::Paste),
            spec.caps.paste.enable(),
            false,
        ));
    }
    if spec.caps.terminal.show() {
        rows.push(item(
            "Open in Terminal here",
            Some(Ico::Terminal),
            Some(MenuAction::OpenInTerminal(spec.folder.clone())),
            spec.caps.terminal.enable(),
            false,
        ));
    }
    if spec.caps.refresh.show() {
        rows.push(item(
            "Refresh",
            Some(Ico::Refresh),
            Some(MenuAction::Refresh),
            spec.caps.refresh.enable(),
            false,
        ));
    }
    if spec.caps.properties.show() {
        rows.push(item(
            "Properties",
            Some(Ico::Info),
            Some(MenuAction::Properties(spec.folder.clone())),
            spec.caps.properties.enable(),
            false,
        ));
    }
    rows
}

fn view_item(label: &str, mode: ViewMode, current: ViewMode) -> MenuRow {
    let mark = if mode == current { "✓ " } else { "" };
    item(
        format!("{mark}{label}"),
        None,
        Some(MenuAction::SetView(mode)),
        true,
        false,
    )
}

fn sort_item(label: &str, key: SortKey, current: SortSpec) -> MenuRow {
    let mark = if current.key == key { "✓ " } else { "" };
    let dir = if current.key == key && current.dir == SortDir::Asc {
        SortDir::Desc
    } else {
        SortDir::Asc
    };
    item(
        format!("{mark}{label}"),
        None,
        Some(MenuAction::SetSort(SortSpec { key, dir })),
        true,
        false,
    )
}

fn tool(icon: Ico, action: MenuAction, enabled: bool, danger: bool) -> ToolBtn {
    ToolBtn {
        icon,
        action,
        enabled,
        danger,
    }
}

fn item(
    label: impl Into<SharedString>,
    icon: Option<Ico>,
    action: Option<MenuAction>,
    enabled: bool,
    danger: bool,
) -> MenuRow {
    MenuRow::Item(MenuItem {
        label: label.into(),
        icon,
        action,
        children: Vec::new(),
        enabled,
        danger,
    })
}

pub fn labels(rows: &[MenuRow]) -> Vec<String> {
    rows.iter()
        .filter_map(|r| match r {
            MenuRow::Separator => None,
            MenuRow::Item(i) => Some(i.label.to_string()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path_caps::{CapsCtx, PathCaps};
    use gpui::px;
    use std::path::Path;

    fn at() -> Point<Pixels> {
        Point::new(px(0.), px(0.))
    }

    #[test]
    fn empty_menu_has_no_toolbar_and_no_select_all() {
        let folder = PathBuf::from("/tmp");
        let caps = PathCaps::for_background(
            &folder,
            CapsCtx {
                clipboard_empty: true,
                is_volume: false,
                pinned: false,
                is_dir: true,
                is_file: false,
                is_multi: false,
                run_as_admin: false,
                folder: Some(&folder),
            },
        );
        let menu = build_empty(
            at(),
            EmptySpec {
                folder,
                caps,
                view: ViewMode::List,
                sort: SortSpec::default(),
            },
        );
        assert!(menu.toolbar.is_empty());
        assert_eq!(menu.kind, MenuKind::Empty);
        let names = labels(&menu.rows);
        assert!(names.contains(&"View".into()));
        assert!(names.contains(&"Sort by".into()));
        assert!(names.contains(&"New".into()));
        assert!(names.contains(&"Paste".into()));
        assert!(names.contains(&"Open in Terminal here".into()));
        assert!(names.contains(&"Refresh".into()));
        assert!(names.contains(&"Properties".into()));
        assert!(
            !names
                .iter()
                .any(|n| n.to_lowercase().contains("select all"))
        );
        assert!(!names.iter().any(|n| n.to_lowercase().contains("group")));
        let paste = menu.rows.iter().find_map(|r| match r {
            MenuRow::Item(i) if i.label.as_ref() == "Paste" => Some(i),
            _ => None,
        });
        assert!(!paste.unwrap().enabled);
    }

    #[test]
    fn entry_toolbar_drops_rename_on_multi() {
        let path = PathBuf::from("/tmp/a.txt");
        let folder = Path::new("/tmp");
        let caps = PathCaps::for_entry(
            &path,
            CapsCtx {
                clipboard_empty: true,
                is_volume: false,
                pinned: false,
                is_dir: false,
                is_file: true,
                is_multi: true,
                run_as_admin: false,
                folder: Some(folder),
            },
        );
        let menu = build_entry(
            at(),
            EntrySpec {
                path: path.clone(),
                targets: vec![path.clone(), PathBuf::from("/tmp/b.txt")],
                caps,
                is_dir: false,
                is_file: true,
                handlers: Vec::new(),
            },
        );
        assert!(menu.toolbar.iter().any(|t| matches!(t.icon, Ico::Scissors)));
        assert!(menu.toolbar.iter().any(|t| matches!(t.icon, Ico::Copy)));
        assert!(menu.toolbar.iter().any(|t| matches!(t.icon, Ico::Trash)));
        assert!(!menu.toolbar.iter().any(|t| matches!(t.icon, Ico::Pencil)));
        assert!(
            !menu
                .toolbar
                .iter()
                .any(|t| matches!(t.icon, Ico::ExternalLink))
        );
    }

    #[test]
    fn folder_promotes_terminal_and_uses_quick_access_labels() {
        let path = PathBuf::from("/tmp/docs");
        let folder = Path::new("/tmp");
        let caps = PathCaps::for_entry(
            &path,
            CapsCtx {
                clipboard_empty: true,
                is_volume: false,
                pinned: false,
                is_dir: true,
                is_file: false,
                is_multi: false,
                run_as_admin: false,
                folder: Some(folder),
            },
        );
        let menu = build_entry(
            at(),
            EntrySpec {
                path: path.clone(),
                targets: vec![path],
                caps,
                is_dir: true,
                is_file: false,
                handlers: Vec::new(),
            },
        );
        assert!(menu.toolbar.iter().any(|t| matches!(t.icon, Ico::Terminal)));
        let names = labels(&menu.rows);
        assert!(names.contains(&"Add to Quick Access".into()));
        assert!(names.contains(&"Open in new tab".into()));
        assert!(names.contains(&"Open in new window".into()));
        assert!(!names.iter().any(|n| n.contains("Unpin")));
        assert!(!names.iter().any(|n| n.contains("Show more")));
    }

    #[test]
    fn open_with_ends_with_choose_another() {
        let path = PathBuf::from("/tmp/a.txt");
        let folder = Path::new("/tmp");
        let caps = PathCaps::for_entry(
            &path,
            CapsCtx {
                clipboard_empty: true,
                is_volume: false,
                pinned: false,
                is_dir: false,
                is_file: true,
                is_multi: false,
                run_as_admin: false,
                folder: Some(folder),
            },
        );
        let menu = build_entry(
            at(),
            EntrySpec {
                path: path.clone(),
                targets: vec![path],
                caps,
                is_dir: false,
                is_file: true,
                handlers: vec![AppHandler {
                    id: "gedit.desktop".into(),
                    name: "Text Editor".into(),
                    exec: "gedit".into(),
                }],
            },
        );
        let open_with = menu.rows.iter().find_map(|r| match r {
            MenuRow::Item(i) if i.label.as_ref() == "Open with" => Some(i),
            _ => None,
        });
        let kids = labels(&open_with.unwrap().children);
        assert_eq!(kids.last().unwrap(), "Choose another app…");
        assert!(kids.contains(&"Text Editor".into()));
    }
}
