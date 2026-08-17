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

use crate::listing::Snapshot;
use crate::theme::{Mode, Palette};
use crate::volumes::{self, Volume};
use crate::watch::FolderWatch;

pub enum LoadState<T> {
    Loading,
    Ready(T),
    Failed(SharedString),
}

/// Where the centre pane is pointed. Home is the drive dashboard, not a folder.
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

/// A right-click menu, positioned in window space.
pub struct Menu {
    pub at: Point<Pixels>,
    pub items: Vec<MenuItem>,
}

#[derive(Clone)]
pub struct MenuItem {
    pub label: SharedString,
    pub action: MenuAction,
    pub danger: bool,
}

#[derive(Clone)]
pub enum MenuAction {
    Open(PathBuf),
    Rename(PathBuf),
    CopyPath(PathBuf),
    Reveal(PathBuf),
    Properties(PathBuf),
    Delete(Vec<PathBuf>),
    Unpin(PathBuf),
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

    pub filter: Entity<InputState>,
    pub filter_text: String,
    /// Item count the filter placeholder was last written for.
    pub placeholder_for: Option<usize>,
    /// Indices into the Ready listing that survive `filter_text`.
    /// Rebuilt when the listing or filter changes — not every frame.
    visible_indices: Vec<usize>,

    pub menu: Option<Menu>,
    pub properties: Option<Properties>,
    pub rename: Option<Rename>,
    pub status: Option<SharedString>,

    list_generation: u64,
    list_task: Option<Task<()>>,
    watch: Option<FolderWatch>,
    pub focus: FocusHandle,
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
            filter,
            filter_text: String::new(),
            placeholder_for: None,
            visible_indices: Vec::new(),
            menu: None,
            properties: None,
            rename: None,
            status: None,
            list_generation: 0,
            list_task: None,
            watch: None,
            focus: cx.focus_handle(),
        };
        ply.refresh_volumes(cx);
        ply.start_watch_poll(cx);
        ply.start_volume_poll(cx);
        window.focus(&ply.focus, cx);
        ply
    }

    pub fn palette(&self) -> Palette {
        self.mode.palette()
    }

    /// Whether a text field has focus, so bare-key shortcuts should stand down.
    pub fn typing(&self, window: &Window, cx: &App) -> bool {
        let focused = |input: &Entity<InputState>| input.focus_handle(cx).is_focused(window);
        focused(&self.filter) || self.rename.as_ref().is_some_and(|r| focused(&r.input))
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
            cx.background_executor()
                .timer(Duration::from_secs(4))
                .await;
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
    if ply.properties.is_some() {
        ply.close_properties(cx);
    } else if ply.menu.is_some() {
        ply.close_menu(cx);
    } else if ply.rename.is_some() {
        ply.cancel_rename(cx);
    } else {
        ply.clear_selection(cx);
    }
}
