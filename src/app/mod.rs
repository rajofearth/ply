//! Ply's state and behaviour. Rendering lives in [`crate::ui`].

pub mod menu;
pub mod ops;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Pixels, Point, SharedString, Task, Window,
    WindowBounds, WindowOptions, point, prelude::*, px, size,
};
use gpui_component::input::{InputEvent, InputState};

use crate::file_clip;
use crate::fs_ops;
use crate::listing::{Entry, Snapshot, SortSpec, list_dir, list_dirs};
use crate::path_caps::{CapsCtx, PathCaps, is_admin_target};
use crate::preview;
use crate::theme::{Mode, Palette};
use crate::volumes::{self, Volume};
use crate::watch::FolderWatch;

pub use crate::listing::ViewMode;
pub use menu::{Menu, MenuAction};

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

pub struct ColumnPane {
    pub path: PathBuf,
    pub listing: LoadState<Snapshot>,
    pub selected: Option<PathBuf>,
}

/// One independent Location stack inside a window.
pub struct Tab {
    pub id: u64,
    pub location: Location,
    history: Vec<Location>,
    history_ix: usize,
    pub listing: LoadState<Snapshot>,
    pub selection: Vec<PathBuf>,
    pub anchor: Option<usize>,
    pub view: ViewMode,
    pub sort: SortSpec,
    pub columns: Vec<ColumnPane>,
    pub filter: Entity<InputState>,
    pub filter_text: String,
    pub placeholder_for: Option<usize>,
    list_generation: u64,
    list_task: Option<Task<()>>,
    watch: Option<FolderWatch>,
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
    pub location: SharedString,
}

pub struct QuickLook {
    pub path: PathBuf,
    pub preview: preview::Preview,
}

pub struct Ply {
    pub mode: Mode,
    tabs: Vec<Tab>,
    active: usize,
    next_tab_id: u64,

    pub volumes: Vec<Volume>,
    pub quick_access: Vec<PathBuf>,

    /// Sidebar branches the user opened. Navigation never adds to this.
    pub expanded: HashSet<PathBuf>,
    pub children: HashMap<PathBuf, Vec<PathBuf>>,

    /// Display names for portable-device objects, whose paths hold opaque
    /// object IDs. Filled as folders are listed, which is also the only way to
    /// reach them, so breadcrumbs always find their ancestors here.
    mtp_names: HashMap<PathBuf, String>,

    pub menu: Option<Menu>,
    pub properties: Option<Properties>,
    pub rename: Option<Rename>,
    pub quick_look: Option<QuickLook>,
    pub status: Option<SharedString>,

    pub focus: FocusHandle,
}

pub fn window_options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(gpui::Bounds {
            origin: point(px(80.), px(80.)),
            size: size(px(1280.), px(800.)),
        })),
        app_id: Some("app.ply.explorer".into()),
        window_decorations: Some(gpui::WindowDecorations::Client),
        titlebar: None,
        ..Default::default()
    }
}

impl Ply {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_at(Location::Home, window, cx)
    }

    pub fn new_at(location: Location, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut ply = Self {
            mode: Mode::Dark,
            tabs: Vec::new(),
            active: 0,
            next_tab_id: 1,
            volumes: Vec::new(),
            quick_access: volumes::default_quick_access(),
            expanded: HashSet::new(),
            children: HashMap::new(),
            mtp_names: HashMap::new(),
            menu: None,
            properties: None,
            rename: None,
            quick_look: None,
            status: None,
            focus: cx.focus_handle(),
        };
        let tab = ply.make_tab(location, window, cx);
        ply.tabs.push(tab);
        ply.refresh_volumes(cx);
        ply.start_watch_poll(cx);
        ply.start_volume_poll(cx);
        ply.enter_active(window, cx);
        window.focus(&ply.focus, cx);
        ply
    }

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn view(&self) -> ViewMode {
        self.tab().view
    }

    pub fn listing(&self) -> &LoadState<Snapshot> {
        &self.tab().listing
    }

    pub fn filter(&self) -> &Entity<InputState> {
        &self.tab().filter
    }

    pub fn filter_text(&self) -> &str {
        &self.tab().filter_text
    }

    pub fn columns(&self) -> &[ColumnPane] {
        &self.tab().columns
    }

    pub fn palette(&self) -> Palette {
        self.mode.palette()
    }

    /// Whether a text field has focus, so bare-key shortcuts should stand down.
    pub fn typing(&self, window: &Window, cx: &App) -> bool {
        let focused = |input: &Entity<InputState>| input.focus_handle(cx).is_focused(window);
        focused(self.filter()) || self.rename.as_ref().is_some_and(|r| focused(&r.input))
    }

    pub fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = self.mode.toggled();
        cx.notify();
    }

    pub fn is_home(&self) -> bool {
        self.tab().location == Location::Home
    }

    pub fn current_folder(&self) -> Option<&Path> {
        match &self.tab().location {
            Location::Home => None,
            Location::Folder(p) => Some(p),
        }
    }

    fn make_tab(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) -> Tab {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let filter = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe(&filter, move |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                if let Some(tab) = this.tabs.iter_mut().find(|t| t.id == id) {
                    tab.filter_text = input.read(cx).value().to_string();
                    tab.selection.clear();
                    tab.anchor = None;
                }
                cx.notify();
            }
        })
        .detach();
        Tab {
            id,
            location: location.clone(),
            history: vec![location],
            history_ix: 0,
            listing: LoadState::Ready(Snapshot::default()),
            selection: Vec::new(),
            anchor: None,
            view: ViewMode::List,
            sort: SortSpec::default(),
            columns: Vec::new(),
            filter,
            filter_text: String::new(),
            placeholder_for: None,
            list_generation: 0,
            list_task: None,
            watch: None,
        }
    }

    // ---- tabs / windows ---------------------------------------------------

    pub fn open_in_new_tab(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.make_tab(Location::Folder(path), window, cx);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.enter_active(window, cx);
    }

    pub fn shortcut_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let location = self.tab().location.clone();
        let tab = self.make_tab(location, window, cx);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.enter_active(window, cx);
    }

    pub fn activate_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            self.active = ix;
            self.menu = None;
            cx.notify();
        }
    }

    pub fn close_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() == 1 {
            self.go_home(window, cx);
            return;
        }
        let Some(ix) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        self.tabs.remove(ix);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        cx.notify();
    }

    pub fn open_in_new_window(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let mode = self.mode;
        let _ = cx.open_window(window_options(), move |window, cx| {
            cx.new(|cx| {
                let mut ply = Ply::new_at(Location::Folder(path.clone()), window, cx);
                ply.mode = mode;
                ply
            })
        });
    }

    pub fn shortcut_new_window(&mut self, cx: &mut Context<Self>) {
        let location = self.tab().location.clone();
        let mode = self.mode;
        let _ = cx.open_window(window_options(), move |window, cx| {
            cx.new(|cx| {
                let mut ply = Ply::new_at(location.clone(), window, cx);
                ply.mode = mode;
                ply
            })
        });
    }

    // ---- navigation -------------------------------------------------------

    pub fn go(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        if location == self.tab().location {
            return;
        }
        {
            let tab = self.tab_mut();
            tab.history.truncate(tab.history_ix + 1);
            tab.history.push(location.clone());
            tab.history_ix = tab.history.len() - 1;
        }
        self.enter(location, window, cx);
    }

    pub fn open_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.go(Location::Folder(path), window, cx);
    }

    pub fn go_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.go(Location::Home, window, cx);
    }

    pub fn can_go_back(&self) -> bool {
        self.tab().history_ix > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.tab().history_ix + 1 < self.tab().history.len()
    }

    pub fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_go_back() {
            self.tab_mut().history_ix -= 1;
            let location = self.tab().history[self.tab().history_ix].clone();
            self.enter(location, window, cx);
        }
    }

    pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_go_forward() {
            self.tab_mut().history_ix += 1;
            let location = self.tab().history[self.tab().history_ix].clone();
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

    fn enter_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let location = self.tab().location.clone();
        self.enter(location, window, cx);
    }

    /// Apply a location without touching history.
    fn enter(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        {
            let tab = self.tab_mut();
            tab.location = location.clone();
            tab.selection.clear();
            tab.anchor = None;
            tab.filter_text.clear();
            tab.placeholder_for = None;
            tab.columns.clear();
        }
        self.rename = None;
        self.menu = None;
        self.quick_look = None;
        let filter = self.filter().clone();
        filter.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        match location {
            Location::Home => {
                self.tab_mut().watch = None;
                self.refresh_volumes(cx);
            }
            Location::Folder(path) => {
                self.tab_mut().watch = if crate::mtp::is_mtp(&path) {
                    None
                } else {
                    FolderWatch::current_folder(path).ok()
                };
                self.tab_mut().listing = LoadState::Loading;
                self.reload(cx);
                if self.view() == ViewMode::Column
                    && let Location::Folder(p) = &self.tab().location
                {
                    let p = p.clone();
                    self.tab_mut().columns = vec![ColumnPane {
                        path: p.clone(),
                        listing: LoadState::Loading,
                        selected: None,
                    }];
                    self.load_column(0, p, cx);
                }
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

    pub fn tab_title(&self, tab: &Tab) -> SharedString {
        match &tab.location {
            Location::Home => "Home".into(),
            Location::Folder(p) => self.display_name(p),
        }
    }

    // ---- loading ----------------------------------------------------------

    pub fn refresh_volumes(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async { volumes::discover() }).await;
            this.update(cx, |this, cx| {
                this.volumes = found;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.current_folder().map(Path::to_path_buf) else {
            return;
        };
        let tab_id = self.tab().id;
        let sort = self.tab().sort;
        self.tab_mut().list_generation += 1;
        let generation = self.tab().list_generation;
        if !matches!(self.tab().listing, LoadState::Ready(_)) {
            self.tab_mut().listing = LoadState::Loading;
        }
        self.tab_mut().list_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { list_dir(&folder) }).await;
            this.update(cx, |this, cx| {
                if this.tab().id != tab_id || this.tab().list_generation != generation {
                    return;
                }
                match result {
                    Ok(mut snapshot) => {
                        snapshot.resort(sort);
                        if let LoadState::Ready(current) = &this.tab().listing
                            && current.fingerprint == snapshot.fingerprint
                        {
                            return;
                        }
                        this.remember_names(&snapshot);
                        this.tab_mut().listing = LoadState::Ready(snapshot);
                    }
                    Err(err) => this.tab_mut().listing = LoadState::Failed(err.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn load_column(&mut self, index: usize, path: PathBuf, cx: &mut Context<Self>) {
        let tab_id = self.tab().id;
        let sort = self.tab().sort;
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { list_dir(&path) }).await;
            this.update(cx, |this, cx| {
                if this.tab().id != tab_id {
                    return;
                }
                match result {
                    Ok(mut snapshot) => {
                        snapshot.resort(sort);
                        this.remember_names(&snapshot);
                        if let Some(col) = this.tab_mut().columns.get_mut(index) {
                            col.listing = LoadState::Ready(snapshot);
                        }
                    }
                    Err(err) => {
                        if let Some(col) = this.tab_mut().columns.get_mut(index) {
                            col.listing = LoadState::Failed(err.to_string().into());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn drill_column(
        &mut self,
        col_ix: usize,
        path: PathBuf,
        is_dir: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        {
            let tab = self.tab_mut();
            tab.selection = vec![path.clone()];
            if let Some(col) = tab.columns.get_mut(col_ix) {
                col.selected = Some(path.clone());
            }
            tab.columns.truncate(col_ix + 1);
        }
        if is_dir {
            let location = Location::Folder(path.clone());
            {
                let tab = self.tab_mut();
                if location != tab.location {
                    tab.history.truncate(tab.history_ix + 1);
                    tab.history.push(location.clone());
                    tab.history_ix = tab.history.len() - 1;
                    tab.location = location;
                }
                tab.columns.push(ColumnPane {
                    path: path.clone(),
                    listing: LoadState::Loading,
                    selected: None,
                });
                tab.watch = if crate::mtp::is_mtp(&path) {
                    None
                } else {
                    FolderWatch::current_folder(path.clone()).ok()
                };
            }
            let new_ix = self.tab().columns.len() - 1;
            self.load_column(new_ix, path, cx);
            // Keep list/grid/status in sync so leaving Column is not stale.
            self.reload(cx);
        }
        cx.notify();
    }

    fn remember_names(&mut self, snapshot: &Snapshot) {
        for entry in &snapshot.entries {
            if crate::mtp::is_mtp(&entry.path) {
                self.mtp_names
                    .insert(entry.path.clone(), entry.name.clone());
            }
        }
    }

    fn start_watch_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        let changed =
                            this.tab().watch.as_ref().is_some_and(|w| {
                                w.take_change_debounced(Duration::from_millis(75))
                            });
                        if changed {
                            this.reload(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn start_volume_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                let found = cx.background_spawn(async { volumes::discover() }).await;
                if this
                    .update(cx, |this, cx| {
                        if this.volumes != found {
                            this.volumes = found;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Entries in the current folder that survive the filter box.
    pub fn visible(&self) -> Vec<&Entry> {
        let LoadState::Ready(snapshot) = &self.tab().listing else {
            return Vec::new();
        };
        if self.tab().filter_text.is_empty() {
            return snapshot.entries.iter().collect();
        }
        let needle = self.tab().filter_text.to_lowercase();
        snapshot
            .entries
            .iter()
            .filter(|e| e.name.to_lowercase().contains(&needle))
            .collect()
    }

    pub fn sync_filter_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_home() {
            return;
        }
        let count = self.total_in_folder();
        if self.tab().placeholder_for == Some(count) {
            return;
        }
        self.tab_mut().placeholder_for = Some(count);
        let text = format!("filter {count} items…");
        let filter = self.filter().clone();
        filter.update(cx, |input, cx| input.set_placeholder(text, window, cx));
    }

    pub fn total_in_folder(&self) -> usize {
        match &self.tab().listing {
            LoadState::Ready(snapshot) => snapshot.entries.len(),
            _ => 0,
        }
    }

    // ---- selection --------------------------------------------------------

    pub fn is_selected(&self, path: &Path) -> bool {
        self.tab().selection.iter().any(|p| p == path)
    }

    pub fn click_row(&mut self, ix: usize, extend: bool, toggle: bool, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.visible().iter().map(|e| e.path.clone()).collect();
        let Some(path) = paths.get(ix).cloned() else {
            return;
        };
        let tab = self.tab_mut();
        if extend && let Some(anchor) = tab.anchor {
            let (lo, hi) = if anchor <= ix {
                (anchor, ix)
            } else {
                (ix, anchor)
            };
            tab.selection = paths[lo..=hi].to_vec();
        } else if toggle {
            match tab.selection.iter().position(|p| *p == path) {
                Some(at) => {
                    tab.selection.remove(at);
                }
                None => tab.selection.push(path),
            }
            tab.anchor = Some(ix);
        } else {
            tab.selection = vec![path];
            tab.anchor = Some(ix);
        }
        cx.notify();
    }

    pub fn move_selection(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.visible().iter().map(|e| e.path.clone()).collect();
        if paths.is_empty() {
            return;
        }
        let current = self
            .tab()
            .selection
            .last()
            .and_then(|last| paths.iter().position(|p| p == last))
            .unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, paths.len() as isize - 1) as usize;
        let tab = self.tab_mut();
        if extend {
            let anchor = tab.anchor.unwrap_or(current as usize);
            let (lo, hi) = if anchor <= next {
                (anchor, next)
            } else {
                (next, anchor)
            };
            tab.selection = paths[lo..=hi].to_vec();
            tab.anchor = Some(anchor);
        } else {
            tab.selection = vec![paths[next].clone()];
            tab.anchor = Some(next);
        }
        cx.notify();
        self.refresh_quick_look(cx);
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.tab_mut().selection.clear();
        self.tab_mut().anchor = None;
        cx.notify();
    }

    pub fn activate_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.tab().selection.last().cloned() else {
            return;
        };
        self.activate(&path, window, cx);
    }

    pub fn activate(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_folder(path) {
            self.open_folder(path.to_path_buf(), window, cx);
        } else if crate::mtp::is_mtp(path) {
            self.open_from_device(path.to_path_buf(), cx);
        } else if let Err(err) = fs_ops::open_with_os(path) {
            self.fail(format!("Could not open: {err}"), cx);
        }
    }

    fn is_folder(&self, path: &Path) -> bool {
        if let LoadState::Ready(snapshot) = &self.tab().listing
            && let Some(entry) = snapshot.entries.iter().find(|e| e.path == path)
        {
            return entry.is_directory();
        }
        for col in &self.tab().columns {
            if let LoadState::Ready(snapshot) = &col.listing
                && let Some(entry) = snapshot.entries.iter().find(|e| e.path == path)
            {
                return entry.is_directory();
            }
        }
        crate::mtp::is_mtp(path) || path.is_dir()
    }

    fn open_from_device(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.note("Copying from the device…", cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let local = crate::mtp::fetch(&path)?;
                    fs_ops::open_with_os(&local)
                })
                .await;
            this.update(cx, |this, cx| match result {
                Ok(()) => this.note("Opened a copy from the device.", cx),
                Err(err) => this.fail(format!("Could not open: {err}"), cx),
            })
            .ok();
        })
        .detach();
    }

    // ---- sidebar ----------------------------------------------------------

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

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

    // ---- menu -------------------------------------------------------------

    fn caps_ctx(&self, path: &Path, is_dir: bool, is_file: bool, is_multi: bool) -> CapsCtx<'_> {
        CapsCtx {
            clipboard_empty: file_clip::is_empty(),
            is_volume: self.volumes.iter().any(|v| v.path == path),
            pinned: self.quick_access.iter().any(|p| p == path),
            is_dir,
            is_file,
            is_multi,
            run_as_admin: is_file && is_admin_target(path),
            folder: self.current_folder(),
        }
    }

    pub fn open_menu(&mut self, at: Point<Pixels>, path: PathBuf, cx: &mut Context<Self>) {
        if !self.is_selected(&path) {
            self.tab_mut().selection = vec![path.clone()];
        }
        let is_dir = self.is_folder(&path);
        let is_file = !is_dir;
        let targets = if self.tab().selection.len() > 1 {
            self.tab().selection.clone()
        } else {
            vec![path.clone()]
        };
        let is_multi = targets.len() > 1;
        let ctx = self.caps_ctx(&path, is_dir, is_file, is_multi);
        let caps = PathCaps::for_entry(&path, ctx);
        let handlers = if caps.open_with.show() {
            crate::open_with::handlers_for(&path)
        } else {
            Vec::new()
        };
        self.menu = Some(menu::build_entry(
            at,
            menu::EntrySpec {
                path,
                targets,
                caps,
                is_dir,
                is_file,
                handlers,
            },
        ));
        cx.notify();
    }

    pub fn open_empty_menu(&mut self, at: Point<Pixels>, folder: PathBuf, cx: &mut Context<Self>) {
        self.tab_mut().selection.clear();
        self.tab_mut().anchor = None;
        if self.tab().location != Location::Folder(folder.clone())
            && self.view() == ViewMode::Column
        {
            self.tab_mut().location = Location::Folder(folder.clone());
        }
        let ctx = CapsCtx {
            clipboard_empty: file_clip::is_empty(),
            is_volume: self.volumes.iter().any(|v| v.path == folder),
            pinned: false,
            is_dir: true,
            is_file: false,
            is_multi: false,
            run_as_admin: false,
            folder: Some(&folder),
        };
        let caps = PathCaps::for_background(&folder, ctx);
        self.menu = Some(menu::build_empty(
            at,
            menu::EmptySpec {
                folder,
                caps,
                view: self.view(),
                sort: self.tab().sort,
            },
        ));
        cx.notify();
    }

    pub fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    pub fn set_flyout(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu
            && menu.flyout != ix
        {
            menu.flyout = ix;
            cx.notify();
        }
    }

    pub fn begin_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        let commit = cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                let ply = cx.entity();
                cx.defer(move |cx| {
                    ply.update(cx, |this, cx| this.commit_rename(cx));
                });
            }
        });
        self.rename = Some(Rename {
            path,
            input,
            _commit: commit,
        });
        cx.notify();
    }

    pub fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        let value = rename.input.read(cx).value().to_string();
        match fs_ops::rename(&rename.path, &value) {
            Ok(target) => {
                self.tab_mut().selection = vec![target];
                self.reload(cx);
            }
            Err(err) => self.fail(err.to_string(), cx),
        }
        cx.notify();
    }

    pub fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        if self.rename.take().is_some() {
            cx.notify();
        }
    }

    pub fn delete(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let count = paths.len();
        match fs_ops::delete_to_trash(&paths) {
            Ok(()) => {
                self.tab_mut().selection.clear();
                self.tab_mut().anchor = None;
                let noun = if count == 1 { "item" } else { "items" };
                self.note(format!("Moved {count} {noun} to the Recycle Bin."), cx);
                self.reload(cx);
            }
            Err(err) => self.fail(format!("Delete failed: {err}"), cx),
        }
    }

    pub fn delete_selection(&mut self, cx: &mut Context<Self>) {
        if !self.tab().selection.is_empty() {
            self.delete(self.tab().selection.clone(), cx);
        }
    }

    pub fn show_properties(&mut self, path: &Path, cx: &mut Context<Self>) {
        let meta = std::fs::metadata(path).ok();
        let volume = self.volumes.iter().find(|v| v.path == path);
        let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());

        let size = match (volume, &meta) {
            (Some(v), _) => format!(
                "{} free of {}",
                crate::listing::format_size(v.free),
                crate::listing::format_size(v.total)
            ),
            (None, Some(m)) if !is_dir => crate::listing::format_size(m.len()),
            _ => "—".into(),
        };
        let kind = match volume.map(|v| v.kind) {
            Some(volumes::VolumeKind::Drive) => "Local Drive".to_string(),
            Some(volumes::VolumeKind::Device) => "Removable Device".to_string(),
            Some(volumes::VolumeKind::Network) => "Network Drive".to_string(),
            None if is_dir => "Folder".to_string(),
            None => crate::listing::kind_label(&Entry {
                path: path.to_path_buf(),
                name: String::new(),
                kind: crate::listing::EntryKind::File,
                size: 0,
                modified: None,
                hidden: false,
            })
            .to_string(),
        };

        self.properties = Some(Properties {
            name: self.display_name(path),
            kind: kind.into(),
            size: size.into(),
            modified: crate::listing::format_mtime(meta.and_then(|m| m.modified().ok())).into(),
            location: path.to_string_lossy().into_owned().into(),
        });
        cx.notify();
    }

    pub fn close_properties(&mut self, cx: &mut Context<Self>) {
        if self.properties.take().is_some() {
            cx.notify();
        }
    }

    pub fn note(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = Some(message.into());
        self.clear_status_later(cx);
    }

    pub fn fail(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = Some(message.into());
        self.clear_status_later(cx);
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
    if ply.properties.is_some() {
        ply.close_properties(cx);
    } else if ply.quick_look.is_some() {
        ply.close_quick_look(cx);
    } else if ply.menu.is_some() {
        ply.close_menu(cx);
    } else if ply.rename.is_some() {
        ply.cancel_rename(cx);
    } else {
        ply.clear_selection(cx);
    }
}
