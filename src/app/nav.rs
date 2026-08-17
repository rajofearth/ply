use std::path::{Path, PathBuf};

use gpui::{Context, SharedString, Window, prelude::*};

use crate::listing::list_dirs;
use crate::watch::FolderWatch;

use super::{LoadState, Location, Ply};

impl Ply {
    pub fn go(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        if location == self.location {
            return;
        }
        self.history.truncate(self.history_ix + 1);
        self.history.push(location.clone());
        self.history_ix = self.history.len() - 1;
        self.enter(location, window, cx);
    }

    pub fn open_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.go(Location::Folder(path), window, cx);
    }

    pub fn go_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.go(Location::Home, window, cx);
    }

    pub fn can_go_back(&self) -> bool {
        self.history_ix > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_ix + 1 < self.history.len()
    }

    pub fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_go_back() {
            self.history_ix -= 1;
            let location = self.history[self.history_ix].clone();
            self.enter(location, window, cx);
        }
    }

    pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_go_forward() {
            self.history_ix += 1;
            let location = self.history[self.history_ix].clone();
            self.enter(location, window, cx);
        }
    }

    pub fn go_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.current_folder().and_then(Path::parent) {
            Some(parent) => {
                let parent = parent.to_path_buf();
                self.open_folder(parent, window, cx);
            }
            None => self.go_home(window, cx),
        }
    }

    /// Apply a location without touching history.
    fn enter(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        self.location = location;
        self.clear_selection_paths();
        self.anchor = None;
        self.rename = None;
        self.menu = None;
        self.filter_text.clear();
        self.placeholder_for = None;
        self.filter.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        match self.location.clone() {
            Location::Home => {
                self.watch = None;
                self.visible_indices.clear();
                self.refresh_volumes(cx);
            }
            Location::Folder(path) => {
                // Portable devices raise no filesystem events to watch for.
                self.watch = if crate::path_caps::for_path(&path).watch {
                    FolderWatch::current_folder(path).ok()
                } else {
                    None
                };
                self.listing = LoadState::Loading;
                self.visible_indices.clear();
                self.reload(cx);
            }
        }
        cx.notify();
    }

    /// Breadcrumb trail: display name plus the folder each crumb opens.
    pub fn crumbs(&self) -> Vec<(SharedString, PathBuf)> {
        let Some(folder) = self.current_folder() else {
            return Vec::new();
        };
        let mut crumbs: Vec<(SharedString, PathBuf)> = folder
            .ancestors()
            .map(|a| (self.display_name(a), a.to_path_buf()))
            .collect();
        crumbs.reverse();
        crumbs
    }

    /// Volume label for a root, otherwise the folder's own name.
    pub fn display_name(&self, path: &Path) -> SharedString {
        if let Some(v) = self.volumes.iter().find(|v| v.path == path) {
            return v.name.clone().into();
        }
        if let Some(name) = self.mtp_names.get(path) {
            return name.clone().into();
        }
        match path.file_name() {
            Some(name) => name.to_string_lossy().into_owned().into(),
            None => path.to_string_lossy().into_owned().into(),
        }
    }

    // ---- sidebar ----------------------------------------------------------

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    /// Expand or collapse a branch, loading its subfolders the first time.
    pub fn toggle_expanded(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.expanded.remove(&path) {
            cx.notify();
            return;
        }
        self.expanded.insert(path.clone());
        cx.notify();
        if self.children.contains_key(&path) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let path = path.clone();
                    async move { list_dirs(&path) }
                })
                .await;
            this.update(cx, |this, cx| {
                let dirs = result
                    .map(|s| {
                        this.remember_names(&s);
                        s.entries.into_iter().map(|e| e.path).collect()
                    })
                    .unwrap_or_default();
                this.children.insert(path, dirs);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn child_folders(&self, path: &Path) -> &[PathBuf] {
        self.children.get(path).map_or(&[], Vec::as_slice)
    }

    pub fn pin(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if path.is_dir() && !self.quick_access.contains(&path) {
            self.quick_access.push(path);
            cx.notify();
        }
    }

    pub fn unpin(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.quick_access.retain(|p| p != path);
        cx.notify();
    }
}
