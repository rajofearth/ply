//! Ply's state and behaviour. Rendering lives in [`crate::ui`].

mod nav;
mod ops;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Pixels, Point, SharedString, Task, Window,
    prelude::*,
};
use gpui_component::input::{InputEvent, InputState};

use crate::listing::{Snapshot, SortKey};
use crate::theme::{Mode, Palette};
use crate::volumes::{self, Volume};
use crate::watch::FolderWatch;

pub enum LoadState<T> {
    Loading,
    Ready(T),
    Failed(SharedString),
}

/// Where the centre pane is pointed. Home is the idle Location, not a folder.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Location {
    Home,
    Folder(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    List,
    Grid,
}

/// A right-click menu: optional icon toolbar, then a vertical list.
pub struct Menu {
    pub at: Point<Pixels>,
    pub toolbar: Vec<ToolBtn>,
    pub rows: Vec<MenuRow>,
    pub flyout: Option<usize>,
}

#[derive(Clone)]
pub struct ToolBtn {
    pub icon: crate::icons::Ico,
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
    pub icon: Option<crate::icons::Ico>,
    pub action: Option<MenuAction>,
    pub children: Vec<MenuRow>,
    pub enabled: bool,
    pub danger: bool,
    pub strong: bool,
}

impl MenuItem {
    pub(super) fn new(
        label: impl Into<SharedString>,
        icon: Option<crate::icons::Ico>,
        action: Option<MenuAction>,
    ) -> Self {
        Self {
            label: label.into(),
            icon,
            action,
            children: Vec::new(),
            enabled: true,
            danger: false,
            strong: false,
        }
    }

    pub(super) fn off(self) -> Self {
        Self {
            enabled: false,
            ..self
        }
    }

    pub(super) fn on(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }

    pub(super) fn danger(self) -> Self {
        Self {
            danger: true,
            ..self
        }
    }
}

impl From<MenuItem> for MenuRow {
    fn from(item: MenuItem) -> Self {
        Self::Item(item)
    }
}

#[derive(Clone)]
pub enum MenuAction {
    Open(PathBuf),
    ChooseApp(PathBuf),
    RunAsAdmin(PathBuf),
    OpenInTerminal(PathBuf),
    Pin(PathBuf),
    Unpin(PathBuf),
    CopyPath(PathBuf),
    Cut,
    Copy,
    Paste,
    Rename(PathBuf),
    Delete(Vec<PathBuf>),
    Reveal(PathBuf),
    Properties(PathBuf),
    Refresh,
    SetView(ViewMode),
    SetSort(SortKey),
    NewFolder,
}

/// A row being renamed inline. The subscription commits on Enter or blur and
/// lives here so it dies with the edit.
pub struct Rename {
    pub path: PathBuf,
    pub input: Entity<InputState>,
    _commit: gpui::Subscription,
}

/// Snapshot of the facts the Properties dialog shows.
pub struct Properties {
    pub name: SharedString,
    pub kind: SharedString,
    pub size: SharedString,
    pub modified: SharedString,
    pub path: SharedString,
    /// Extra shell-sourced facts (author, title, created, ...) filled in
    /// asynchronously after the dialog opens.
    pub details: Vec<(SharedString, SharedString)>,
}

/// What confirming a [`ConfirmDialog`] runs.
pub enum ConfirmAction {
    DeletePermanently(Vec<PathBuf>),
}

/// A modal asking the user to confirm a potentially destructive action.
pub struct ConfirmDialog {
    pub title: SharedString,
    pub message: SharedString,
    pub confirm_text: SharedString,
    pub danger: bool,
    pub action: ConfirmAction,
}

pub struct Ply {
    pub mode: Mode,
    pub location: Location,
    history: Vec<Location>,
    history_ix: usize,

    pub listing: LoadState<Snapshot>,
    pub volumes: Vec<Volume>,
    pub quick_access: Vec<PathBuf>,

    /// Sidebar branches the user opened. Navigation never adds to this.
    pub expanded: HashSet<PathBuf>,
    pub children: HashMap<PathBuf, Vec<PathBuf>>,

    /// Display names for portable-device objects, whose paths hold opaque
    /// object IDs. Filled as folders are listed, which is also the only way to
    /// reach them, so breadcrumbs always find their ancestors here.
    mtp_names: HashMap<PathBuf, String>,

    /// Selection order (shift-select / activate last). Membership is mirrored in
    /// [`Self::selection_set`] for O(1) row checks while painting.
    pub selection: Vec<PathBuf>,
    selection_set: HashSet<PathBuf>,
    anchor: Option<usize>,
    pub view: ViewMode,
    pub sort: SortKey,

    pub filter: Entity<InputState>,
    pub filter_text: String,
    /// Item count the filter placeholder was last written for.
    pub placeholder_for: Option<usize>,
    /// Indices into the Ready listing that survive `filter_text`.
    /// Rebuilt when the listing or filter changes — not every frame.
    visible_indices: Vec<usize>,

    pub menu: Option<Menu>,
    pub properties: Option<Properties>,
    pub confirm: Option<ConfirmDialog>,
    pub rename: Option<Rename>,
    pub status: Option<SharedString>,

    list_generation: u64,
    list_task: Option<Task<()>>,
    watch: Option<FolderWatch>,
    pub focus: FocusHandle,

    /// Decoded media thumbnails, keyed by path + mtime. Dropped with the window.
    pub thumbs: Entity<crate::thumbs::ThumbCache>,
}

impl Ply {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe(&filter, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.filter_text = input.read(cx).value().to_string();
                this.rebuild_visible();
                this.clear_selection_paths();
                this.anchor = None;
                cx.notify();
            }
        })
        .detach();

        let mut ply = Self {
            mode: Mode::Dark,
            location: Location::Home,
            history: vec![Location::Home],
            history_ix: 0,
            listing: LoadState::Ready(Snapshot::default()),
            volumes: Vec::new(),
            quick_access: volumes::default_quick_access(),
            expanded: HashSet::new(),
            children: HashMap::new(),
            mtp_names: HashMap::new(),
            selection: Vec::new(),
            selection_set: HashSet::new(),
            anchor: None,
            view: ViewMode::List,
            sort: SortKey::default(),
            filter,
            filter_text: String::new(),
            placeholder_for: None,
            visible_indices: Vec::new(),
            menu: None,
            properties: None,
            confirm: None,
            rename: None,
            status: None,
            list_generation: 0,
            list_task: None,
            watch: None,
            focus: cx.focus_handle(),
            thumbs: cx.new(|_| crate::thumbs::ThumbCache::new()),
        };
        ply.refresh_volumes(cx);
        ply.start_watch_poll(cx);
        ply.start_volume_poll(cx);
        ply.start_lnk_refresh(cx);
        ply.update_window_title(window);
        window.focus(&ply.focus, cx);
        ply
    }

    pub fn palette(&self) -> Palette {
        self.mode.palette()
    }

    /// The window's thumbnail cache.
    pub fn thumb_cache(&self) -> Entity<crate::thumbs::ThumbCache> {
        self.thumbs.clone()
    }

    /// Whether a text field has focus, so bare-key shortcuts should stand down.
    pub fn typing(&self, window: &Window, cx: &App) -> bool {
        let focused = |input: &Entity<InputState>| input.focus_handle(cx).is_focused(window);
        focused(&self.filter) || self.rename.as_ref().is_some_and(|r| focused(&r.input))
    }

    /// The OS window title (taskbar / alt-tab): `<folder> - Ply`, or
    /// `Home - Ply`. The folder name is truncated from the middle when long.
    fn window_title(&self) -> String {
        let name = match &self.location {
            Location::Home => "Home".to_string(),
            Location::Folder(path) => crate::listing::truncate_middle(&self.display_name(path), 60),
        };
        format!("{name} - Ply")
    }

    /// Push the current location into the native window title. Called whenever
    /// the Location changes so the taskbar and alt-tab stay in sync.
    fn update_window_title(&self, window: &mut Window) {
        window.set_window_title(&self.window_title());
    }

    pub fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = self.mode.toggled();
        cx.notify();
    }

    pub fn is_home(&self) -> bool {
        self.location == Location::Home
    }

    pub fn current_folder(&self) -> Option<&Path> {
        match &self.location {
            Location::Home => None,
            Location::Folder(p) => Some(p),
        }
    }

    pub fn set_view(&mut self, view: ViewMode, cx: &mut Context<Self>) {
        self.view = view;
        cx.notify();
    }

    pub fn set_sort(&mut self, key: SortKey, cx: &mut Context<Self>) {
        self.sort = key;
        if let LoadState::Ready(snap) = &mut self.listing {
            snap.resort(key);
        }
        self.rebuild_visible();
        cx.notify();
    }

    pub fn note(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = Some(message.into());
        self.clear_status_later(cx);
    }

    pub fn fail(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.note(message, cx);
    }

    fn clear_status_later(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            this.update(cx, |this, cx| {
                this.status = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Escape closes whatever is on top, innermost first.
pub fn dismiss_topmost(ply: &mut Ply, cx: &mut Context<Ply>) {
    if ply.confirm.is_some() {
        ply.cancel_confirm(cx);
    } else if ply.properties.is_some() {
        ply.close_properties(cx);
    } else if ply.menu.is_some() {
        ply.close_menu(cx);
    } else if ply.rename.is_some() {
        ply.cancel_rename(cx);
    } else {
        ply.clear_selection(cx);
    }
}
