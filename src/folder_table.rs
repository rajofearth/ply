use gpui::{App, Context, IntoElement, ParentElement, SharedString, Styled, Window, div};
use gpui_component::label::Label;
use gpui_component::menu::PopupMenu;
use gpui_component::table::{Column, ColumnSort, TableDelegate, TableState};
use gpui_component::tag::Tag;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};

use crate::listing::{format_mtime, format_size, sort_snapshot, Entry, EntryKind, Snapshot};
use crate::{CopyPath, OpenSelection, Refresh, Reveal};

pub struct FolderDelegate {
    snapshot: Snapshot,
    columns: Vec<Column>,
    filter: String,
    visible: Vec<usize>,
    loading: bool,
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
            loading: false,
        }
    }

    pub fn set_snapshot(&mut self, snapshot: Snapshot) {
        self.snapshot = snapshot;
        self.loading = false;
        self.refilter();
    }

    pub fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
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

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn total_len(&self) -> usize {
        self.snapshot.entries.len()
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
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(entry) = self.entry(row_ix) else {
            return div().into_any_element();
        };
        match self.columns[col_ix].key.as_ref() {
            "name" => {
                let icon = match entry.kind {
                    EntryKind::Directory => IconName::Folder,
                    EntryKind::File => IconName::File,
                    EntryKind::Symlink { .. } => IconName::File,
                };
                let mut name = Label::new(entry.name.clone());
                if !self.filter.is_empty() {
                    name = name.highlights(self.filter.clone());
                }
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(
                        Icon::new(icon)
                            .small()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(name)
                    .into_any_element()
            }
            "kind" => match entry.kind {
                EntryKind::Directory => Tag::info().small().child("Folder").into_any_element(),
                EntryKind::File => Tag::secondary().small().child("File").into_any_element(),
                EntryKind::Symlink { .. } => {
                    Tag::secondary().small().child("Link").into_any_element()
                }
            },
            "size" => {
                if entry.is_directory() {
                    SharedString::from("—").into_any_element()
                } else {
                    SharedString::from(format_size(entry.size)).into_any_element()
                }
            }
            "modified" => SharedString::from(format_mtime(entry.modified)).into_any_element(),
            _ => div().into_any_element(),
        }
    }

    fn loading(&self, _: &App) -> bool {
        self.loading
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .text_color(cx.theme().muted_foreground)
            .child(Icon::new(IconName::Inbox).large())
            .child(Label::new("No entries").secondary("This folder is empty or the filter hid everything"))
    }
}
