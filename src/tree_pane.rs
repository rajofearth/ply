use std::path::{Path, PathBuf};

use gpui_component::tree::TreeItem;

use crate::listing::{list_dirs, Snapshot};

const PENDING_SUFFIX: &str = "::__pending";

pub fn is_pending_id(id: &str) -> bool {
    id.ends_with(PENDING_SUFFIX)
}

pub fn path_id(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn pending_child(parent: &Path) -> TreeItem {
    TreeItem::new(format!("{}{PENDING_SUFFIX}", parent.display()), "…").disabled(true)
}

fn folder_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Workspace (or any directory) node with a single pending child so the library
/// treats it as a folder without listing on the UI thread.
pub fn workspace_item(path: &Path) -> TreeItem {
    TreeItem::new(path_id(path), folder_label(path))
        .expanded(true)
        .child(pending_child(path))
}

pub fn folder_item(path: &Path, name: &str) -> TreeItem {
    TreeItem::new(path_id(path), name.to_string()).child(pending_child(path))
}

pub fn needs_children_load(item: &TreeItem) -> bool {
    item.children.len() == 1 && is_pending_id(item.children[0].id.as_ref())
}

pub fn item_needs_load(roots: &[TreeItem], id: &str) -> bool {
    find_item(roots, id).is_some_and(needs_children_load)
}

fn find_item<'a>(roots: &'a [TreeItem], id: &str) -> Option<&'a TreeItem> {
    for item in roots {
        if item.id.as_ref() == id {
            return Some(item);
        }
        if let Some(found) = find_item(&item.children, id) {
            return Some(found);
        }
    }
    None
}

pub fn set_children(roots: &mut [TreeItem], id: &str, children: Vec<TreeItem>) -> bool {
    fn rec(item: &mut TreeItem, id: &str, children: &mut Option<Vec<TreeItem>>) -> bool {
        if item.id.as_ref() == id {
            if let Some(next) = children.take() {
                item.children = next;
            }
            return true;
        }
        item.children.iter_mut().any(|child| rec(child, id, children))
    }

    let mut children = Some(children);
    roots.iter_mut().any(|root| rec(root, id, &mut children))
}

/// Directories only. Returns Send data so it can run on a background thread.
/// Does not follow symlinks (`list_dirs` reports them as links).
pub fn list_folder_children(path: &Path, show_hidden: bool) -> anyhow::Result<Snapshot> {
    list_dirs(path, show_hidden)
}

pub fn items_from_dir_snapshot(snapshot: &Snapshot) -> Vec<TreeItem> {
    snapshot
        .entries
        .iter()
        .map(|entry| folder_item(&entry.path, &entry.name))
        .collect()
}

pub fn path_from_tree_id(id: &str) -> Option<PathBuf> {
    if is_pending_id(id) {
        None
    } else {
        Some(PathBuf::from(id))
    }
}
