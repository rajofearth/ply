//! Side-effecting commands: clipboard, New, tabs, Quick Look, Open with.

use std::path::{Path, PathBuf};

use gpui::{ClipboardItem, Context, Window};

use super::menu::MenuAction;
use super::{LoadState, Location, Ply};
use crate::file_clip::{self, ClipOp};
use crate::fs_ops;
use crate::listing::ViewMode;
use crate::preview;

impl Ply {
    pub fn run(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        match action {
            MenuAction::Open(path) => self.activate(&path, window, cx),
            MenuAction::OpenWith { path, app } => {
                if let Err(err) = crate::open_with::open_with(&path, &app) {
                    self.fail(format!("Open with failed: {err}"), cx);
                }
            }
            MenuAction::ChooseApp(path) => {
                if let Err(err) = crate::open_with::choose_another(&path) {
                    self.fail(format!("Choose app failed: {err}"), cx);
                }
            }
            MenuAction::RunAsAdmin(path) => {
                if let Err(err) = fs_ops::run_as_admin(&path) {
                    self.fail(format!("Could not elevate: {err}"), cx);
                }
            }
            MenuAction::OpenInTerminal(path) => {
                if let Err(err) = fs_ops::open_terminal(&path) {
                    self.fail(format!("Terminal failed: {err}"), cx);
                }
            }
            MenuAction::OpenInNewTab(path) => self.open_in_new_tab(path, window, cx),
            MenuAction::OpenInNewWindow(path) => self.open_in_new_window(path, cx),
            MenuAction::Pin(path) => self.pin(path, cx),
            MenuAction::Unpin(path) => self.unpin(&path, cx),
            MenuAction::CopyPath(paths) => self.copy_paths(&paths, cx),
            MenuAction::Cut(paths) => self.clip_files(paths, ClipOp::Cut, cx),
            MenuAction::Copy(paths) => self.clip_files(paths, ClipOp::Copy, cx),
            MenuAction::Paste => self.paste(cx),
            MenuAction::Rename(path) => self.begin_rename(path, window, cx),
            MenuAction::Delete(paths) => self.delete(paths, cx),
            MenuAction::Properties(path) => self.show_properties(&path, cx),
            MenuAction::Refresh => self.reload(cx),
            MenuAction::SetView(view) => self.set_view(view, cx),
            MenuAction::SetSort(spec) => {
                self.tab_mut().sort = spec;
                if let LoadState::Ready(snap) = &mut self.tab_mut().listing {
                    snap.resort(spec);
                }
                cx.notify();
            }
            MenuAction::NewFolder => self.new_folder(window, cx),
            MenuAction::NewFile => self.new_file(window, cx),
        }
        cx.notify();
    }

    pub fn clip_selection(&mut self, op: ClipOp, cx: &mut Context<Self>) {
        let paths = self.tab().selection.clone();
        if !paths.is_empty() {
            self.clip_files(paths, op, cx);
        }
    }

    fn clip_files(&mut self, paths: Vec<PathBuf>, op: ClipOp, cx: &mut Context<Self>) {
        match file_clip::set(paths, op) {
            Ok(()) => {
                let verb = match op {
                    ClipOp::Copy => "Copied",
                    ClipOp::Cut => "Cut",
                };
                self.note(format!("{verb} to the clipboard."), cx);
            }
            Err(err) => self.fail(err.to_string(), cx),
        }
    }

    pub fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.current_folder().map(Path::to_path_buf) else {
            return;
        };
        let Some(clip) = file_clip::get() else {
            self.note("Nothing to paste.", cx);
            return;
        };
        let result = match clip.op {
            ClipOp::Copy => fs_ops::copy_entries(&clip.paths, &folder),
            ClipOp::Cut => fs_ops::move_entries(&clip.paths, &folder),
        };
        match result {
            Ok(new_paths) => {
                if clip.op == ClipOp::Cut {
                    file_clip::take_if_cut();
                    file_clip::clear();
                }
                self.tab_mut().selection = new_paths;
                self.note("Pasted.", cx);
                self.reload(cx);
            }
            Err(err) => self.fail(format!("Paste failed: {err}"), cx),
        }
    }

    fn copy_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let text = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.note("Path copied.", cx);
    }

    pub fn copy_selected_path(&mut self, cx: &mut Context<Self>) {
        let paths = self.tab().selection.clone();
        if !paths.is_empty() {
            self.copy_paths(&paths, cx);
        }
    }

    fn new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self.current_folder().map(Path::to_path_buf) else {
            return;
        };
        match fs_ops::create_folder(&parent, "New folder") {
            Ok(path) => {
                self.reload(cx);
                self.begin_rename(path, window, cx);
            }
            Err(err) => self.fail(err.to_string(), cx),
        }
    }

    fn new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self.current_folder().map(Path::to_path_buf) else {
            return;
        };
        match fs_ops::create_file(&parent, "New Text Document.txt") {
            Ok(path) => {
                self.reload(cx);
                self.begin_rename(path, window, cx);
            }
            Err(err) => self.fail(err.to_string(), cx),
        }
    }

    pub fn new_folder_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_folder(window, cx);
    }

    pub fn toggle_quick_look(&mut self, cx: &mut Context<Self>) {
        if self.quick_look.take().is_some() {
            cx.notify();
            return;
        }
        let Some(path) = self.tab().selection.last().cloned() else {
            return;
        };
        self.quick_look = Some(super::QuickLook {
            path: path.clone(),
            preview: preview::for_path(&path),
        });
        cx.notify();
    }

    pub fn close_quick_look(&mut self, cx: &mut Context<Self>) {
        if self.quick_look.take().is_some() {
            cx.notify();
        }
    }

    pub fn refresh_quick_look(&mut self, cx: &mut Context<Self>) {
        if self.quick_look.is_none() {
            return;
        }
        let Some(path) = self.tab().selection.last().cloned() else {
            self.close_quick_look(cx);
            return;
        };
        self.quick_look = Some(super::QuickLook {
            path: path.clone(),
            preview: preview::for_path(&path),
        });
        cx.notify();
    }

    pub fn set_view(&mut self, view: ViewMode, cx: &mut Context<Self>) {
        let path = match (&self.tab().location, view) {
            (Location::Folder(path), ViewMode::Column) => Some(path.clone()),
            _ => None,
        };
        let tab = self.tab_mut();
        tab.view = view;
        if let Some(path) = path {
            tab.columns = vec![super::ColumnPane {
                path: path.clone(),
                listing: LoadState::Loading,
                selected: None,
            }];
            self.load_column(0, path, cx);
        }
        cx.notify();
    }
}
