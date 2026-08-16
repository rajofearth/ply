//! Sidebar folder tree: expansion state plus lazily loaded child folders.
//!
//! The tree only ever holds directories, keyed by their path string, so a row can
//! be rendered without touching the filesystem on the UI thread.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::listing::list_dirs;

/// Stable row id for a folder (also the sidebar tree key).
pub fn path_id(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Row label for a folder: the file name, or the whole path for roots like `/`.
pub fn folder_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// A flattened sidebar row: a folder plus its indent depth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub path: PathBuf,
    pub depth: usize,
}

#[derive(Default)]
pub struct SidebarTree {
    expanded: HashSet<String>,
    children: HashMap<String, Vec<PathBuf>>,
}

impl SidebarTree {
    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(&path_id(path))
    }

    pub fn set_expanded(&mut self, path: &Path, expanded: bool) {
        if expanded {
            self.expanded.insert(path_id(path));
        } else {
            self.expanded.remove(&path_id(path));
        }
    }

    /// Flip a row open/closed and report the new state.
    pub fn toggle(&mut self, path: &Path) -> bool {
        let expanded = !self.is_expanded(path);
        self.set_expanded(path, expanded);
        expanded
    }

    pub fn children(&self, path: &Path) -> Option<&[PathBuf]> {
        self.children.get(&path_id(path)).map(|c| c.as_slice())
    }

    /// True when the row is open but its children have not been listed yet.
    pub fn needs_children(&self, path: &Path) -> bool {
        let id = path_id(path);
        self.expanded.contains(&id) && !self.children.contains_key(&id)
    }

    pub fn set_children(&mut self, path: &Path, children: Vec<PathBuf>) {
        self.children.insert(path_id(path), children);
    }

    /// Drop every cached listing, keeping expansion state, so open rows reload.
    pub fn forget_children(&mut self) {
        self.children.clear();
    }

    /// Paths of open rows, so a refresh can re-list exactly what is visible.
    pub fn expanded_paths(&self) -> Vec<PathBuf> {
        self.expanded.iter().map(PathBuf::from).collect()
    }

    /// Open every ancestor of `path` down from `root` so the row becomes visible.
    pub fn reveal(&mut self, root: &Path, path: &Path) {
        self.set_expanded(root, true);
        let Ok(rel) = path.strip_prefix(root) else {
            return;
        };
        let mut acc = root.to_path_buf();
        for part in rel.components() {
            acc.push(part.as_os_str());
            self.set_expanded(&acc, true);
        }
    }

    /// Visible descendants of `root`, depth-first, excluding `root` itself.
    pub fn rows_under(&self, root: &Path, depth: usize) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        self.collect(root, depth, &mut rows);
        rows
    }

    fn collect(&self, folder: &Path, depth: usize, out: &mut Vec<TreeRow>) {
        if !self.is_expanded(folder) {
            return;
        }
        let Some(children) = self.children(folder) else {
            return;
        };
        for child in children {
            out.push(TreeRow {
                path: child.clone(),
                depth,
            });
            self.collect(child, depth + 1, out);
        }
    }
}

/// Direct child folders, sorted, for a sidebar row. Runs off the UI thread.
/// Does not follow symlinks (`list_dirs` reports those as links, not folders).
pub fn list_child_folders(path: &Path, show_hidden: bool) -> anyhow::Result<Vec<PathBuf>> {
    Ok(list_dirs(path, show_hidden)?
        .entries
        .into_iter()
        .map(|entry| entry.path)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_and_children_round_trip() {
        let mut tree = SidebarTree::default();
        let root = PathBuf::from("/tmp");
        let child = PathBuf::from("/tmp/a");

        assert!(!tree.is_expanded(&root));
        assert!(tree.toggle(&root));
        assert!(tree.is_expanded(&root));
        assert!(tree.needs_children(&root));

        tree.set_children(&root, vec![child.clone()]);
        assert!(!tree.needs_children(&root));
        assert_eq!(tree.children(&root), Some([child.clone()].as_slice()));
        assert_eq!(
            tree.rows_under(&root, 0),
            vec![TreeRow {
                path: child.clone(),
                depth: 0
            }]
        );

        tree.forget_children();
        assert!(tree.needs_children(&root));
        assert_eq!(tree.rows_under(&root, 0), Vec::new());
        assert!(!tree.toggle(&root));
        assert_eq!(tree.rows_under(&root, 0), Vec::new());
    }

    #[test]
    fn nested_rows_carry_depth() {
        let mut tree = SidebarTree::default();
        let root = PathBuf::from("/tmp");
        let a = PathBuf::from("/tmp/a");
        let b = PathBuf::from("/tmp/a/b");
        tree.set_expanded(&root, true);
        tree.set_children(&root, vec![a.clone()]);
        tree.set_expanded(&a, true);
        tree.set_children(&a, vec![b.clone()]);

        assert_eq!(
            tree.rows_under(&root, 1),
            vec![TreeRow { path: a, depth: 1 }, TreeRow { path: b, depth: 2 },]
        );
    }

    #[test]
    fn reveal_expands_ancestors_only() {
        let mut tree = SidebarTree::default();
        let root = PathBuf::from("/tmp");
        let deep = PathBuf::from("/tmp/a/b");
        tree.reveal(&root, &deep);

        assert!(tree.is_expanded(&root));
        assert!(tree.is_expanded(Path::new("/tmp/a")));
        assert!(tree.is_expanded(&deep));
        assert!(!tree.is_expanded(Path::new("/other")));

        let mut expanded = tree.expanded_paths();
        expanded.sort();
        assert_eq!(
            expanded,
            vec![
                PathBuf::from("/tmp"),
                PathBuf::from("/tmp/a"),
                PathBuf::from("/tmp/a/b")
            ]
        );
    }

    #[test]
    fn folder_label_falls_back_to_path() {
        assert_eq!(folder_label(Path::new("/tmp/docs")), "docs");
        assert_eq!(folder_label(Path::new("/")), "/");
        assert_eq!(path_id(Path::new("/tmp/docs")), "/tmp/docs");
    }
}
