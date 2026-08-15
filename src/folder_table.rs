use gpui::{App, Context, IntoElement, SharedString, Window};
use gpui_component::menu::PopupMenu;
use gpui_component::table::{Column, ColumnSort, TableDelegate, TableState};

use crate::listing::{format_mtime, format_size, sort_snapshot, Entry, EntryKind, Snapshot};
use crate::{CopyPath, OpenSelection, Refresh, Reveal};

pub struct FolderDelegate {
    snapshot: Snapshot,
    columns: Vec<Column>,
    filter: String,
    visible: Vec<usize>,
}

impl FolderDelegate {
    pub fn new() -> Self {
        Self {
            snapshot: Snapshot::default(),
            columns: vec![
                Column::new("name", "Name").width(280.).sortable(),
                Column::new("kind", "Kind").width(90.).sortable(),
                Column::new("size", "Size").width(100.).sortable(),
                Column::new("modified", "Modified").width(180.).sortable(),
            ],
            filter: String::new(),
            visible: Vec::new(),
        }
    }

    pub fn set_snapshot(&mut self, snapshot: Snapshot) {
        self.snapshot = snapshot;
        self.refilter();
    }

    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.refilter();
    }

    fn refilter(&mut self) {
        let q = self.filter.to_lowercase();
        self.visible = self
            .snapshot
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| q.is_empty() || e.name.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();
    }

    pub fn entry(&self, row_ix: usize) -> Option<&Entry> {
        self.visible
            .get(row_ix)
            .and_then(|i| self.snapshot.entries.get(*i))
    }
}

impl TableDelegate for FolderDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.visible.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        let key = self.columns[col_ix].key.to_string();
        let ascending = !matches!(sort, ColumnSort::Descending);
        self.snapshot = sort_snapshot(std::mem::take(&mut self.snapshot), &key, ascending);
        self.refilter();
    }

    fn context_menu(
        &mut self,
        _: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        menu.menu("Open", Box::new(OpenSelection))
            .menu("Reveal", Box::new(Reveal))
            .menu("Copy path", Box::new(CopyPath))
            .separator()
            .menu("Refresh", Box::new(Refresh))
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        self.columns[col_ix].name.clone()
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(entry) = self.entry(row_ix) else {
            return SharedString::from("");
        };
        match self.columns[col_ix].key.as_ref() {
            "name" => SharedString::from(entry.name.clone()),
            "kind" => SharedString::from(match entry.kind {
                EntryKind::Directory => "Folder",
                EntryKind::File => "File",
                EntryKind::Symlink { .. } => "Link",
            }),
            "size" => {
                if entry.is_directory() {
                    SharedString::from("—")
                } else {
                    SharedString::from(format_size(entry.size))
                }
            }
            "modified" => SharedString::from(format_mtime(entry.modified)),
            _ => SharedString::from(""),
        }
    }
}
