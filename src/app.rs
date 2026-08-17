//! Ply's state and behaviour. Rendering lives in [`crate::ui`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Pixels, Point, SharedString, Task, Window,
    prelude::*,
};
use gpui_component::input::{InputEvent, InputState};

use crate::fs_ops;
use crate::listing::{Entry, Snapshot, list_dir, list_dirs};
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
    pub location: SharedString,
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

    pub selection: Vec<PathBuf>,
    anchor: Option<usize>,
    pub view: ViewMode,

    pub filter: Entity<InputState>,
    pub filter_text: String,
    /// Item count the filter placeholder was last written for.
    pub placeholder_for: Option<usize>,

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
                this.selection.clear();
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
            anchor: None,
            view: ViewMode::List,
            filter,
            filter_text: String::new(),
            placeholder_for: None,
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

    // ---- navigation -------------------------------------------------------

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
        self.selection.clear();
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
                self.refresh_volumes(cx);
            }
            Location::Folder(path) => {
                // Portable devices raise no filesystem events to watch for.
                self.watch = if crate::mtp::is_mtp(&path) {
                    None
                } else {
                    FolderWatch::current_folder(path).ok()
                };
                self.listing = LoadState::Loading;
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

    // ---- loading ----------------------------------------------------------

    pub fn refresh_volumes(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            // Discovery stats every drive and can block on network shares.
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
        self.list_generation += 1;
        let generation = self.list_generation;
        if !matches!(self.listing, LoadState::Ready(_)) {
            self.listing = LoadState::Loading;
        }
        self.list_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { list_dir(&folder) })
                .await;
            this.update(cx, |this, cx| {
                if this.list_generation != generation {
                    return;
                }
                match result {
                    Ok(snapshot) => {
                        // A watch-driven reload usually finds nothing new;
                        // replacing an identical listing only causes churn.
                        if let LoadState::Ready(current) = &this.listing
                            && current.fingerprint == snapshot.fingerprint
                        {
                            return;
                        }
                        this.remember_names(&snapshot);
                        this.listing = LoadState::Ready(snapshot);
                    }
                    Err(err) => this.listing = LoadState::Failed(err.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Portable-device paths are object IDs, so keep the names the listing
    /// reported; nothing else can recover them later.
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
                        let changed = this
                            .watch
                            .as_ref()
                            .is_some_and(|w| w.take_change_debounced(Duration::from_millis(75)));
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

    /// Notice drives appearing and disappearing. Windows delivers this as
    /// `WM_DEVICECHANGE`, which GPUI does not surface, so poll instead and
    /// redraw only when the set actually moved.
    fn start_volume_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                // Sequential: an unreachable network share can stall discovery,
                // and awaiting it keeps the polls from stacking up.
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
        let LoadState::Ready(snapshot) = &self.listing else {
            return Vec::new();
        };
        if self.filter_text.is_empty() {
            return snapshot.entries.iter().collect();
        }
        let needle = self.filter_text.to_lowercase();
        snapshot
            .entries
            .iter()
            .filter(|e| e.name.to_lowercase().contains(&needle))
            .collect()
    }

    /// Keep the filter's placeholder showing the folder's item count.
    ///
    /// Written from render because the count only settles once the listing
    /// lands, and writing input state needs a window; the stored count makes
    /// this a no-op on all the frames where nothing changed.
    pub fn sync_filter_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_home() {
            return;
        }
        let count = self.total_in_folder();
        if self.placeholder_for == Some(count) {
            return;
        }
        self.placeholder_for = Some(count);
        let text = format!("filter {count} items…");
        self.filter
            .update(cx, |input, cx| input.set_placeholder(text, window, cx));
    }

    pub fn total_in_folder(&self) -> usize {
        match &self.listing {
            LoadState::Ready(snapshot) => snapshot.entries.len(),
            _ => 0,
        }
    }

    // ---- selection --------------------------------------------------------

    pub fn is_selected(&self, path: &Path) -> bool {
        self.selection.iter().any(|p| p == path)
    }

    pub fn click_row(&mut self, ix: usize, extend: bool, toggle: bool, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.visible().iter().map(|e| e.path.clone()).collect();
        let Some(path) = paths.get(ix).cloned() else {
            return;
        };
        if extend && let Some(anchor) = self.anchor {
            let (lo, hi) = if anchor <= ix { (anchor, ix) } else { (ix, anchor) };
            self.selection = paths[lo..=hi].to_vec();
        } else if toggle {
            match self.selection.iter().position(|p| *p == path) {
                Some(at) => {
                    self.selection.remove(at);
                }
                None => self.selection.push(path),
            }
            self.anchor = Some(ix);
        } else {
            self.selection = vec![path];
            self.anchor = Some(ix);
        }
        cx.notify();
    }

    /// Arrow-key movement. `extend` grows the range from the anchor.
    pub fn move_selection(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.visible().iter().map(|e| e.path.clone()).collect();
        if paths.is_empty() {
            return;
        }
        let current = self
            .selection
            .last()
            .and_then(|last| paths.iter().position(|p| p == last))
            .unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, paths.len() as isize - 1) as usize;
        if extend {
            let anchor = self.anchor.unwrap_or(current as usize);
            let (lo, hi) = if anchor <= next {
                (anchor, next)
            } else {
                (next, anchor)
            };
            self.selection = paths[lo..=hi].to_vec();
            self.anchor = Some(anchor);
        } else {
            self.selection = vec![paths[next].clone()];
            self.anchor = Some(next);
        }
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selection.clear();
        self.anchor = None;
        cx.notify();
    }

    /// Open whatever is selected: folders navigate, files go to the OS.
    pub fn activate_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.selection.last().cloned() else {
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

    /// `is_dir` cannot answer for portable devices, so trust the listing that
    /// produced the path and fall back to the filesystem for everything else.
    fn is_folder(&self, path: &Path) -> bool {
        if let LoadState::Ready(snapshot) = &self.listing
            && let Some(entry) = snapshot.entries.iter().find(|e| e.path == path)
        {
            return entry.is_directory();
        }
        // Anything reached from the sidebar or This PC is already a container.
        crate::mtp::is_mtp(path) || path.is_dir()
    }

    /// Device data has no path, so copy the object out before handing it over.
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

    // ---- menu, properties, file operations --------------------------------

    pub fn open_menu(&mut self, at: Point<Pixels>, path: PathBuf, cx: &mut Context<Self>) {
        if !self.is_selected(&path) {
            self.selection = vec![path.clone()];
        }
        let pinned = self.quick_access.contains(&path);
        let is_volume = self.volumes.iter().any(|v| v.path == path);
        let targets = if self.selection.len() > 1 {
            self.selection.clone()
        } else {
            vec![path.clone()]
        };

        let mut items = vec![MenuItem {
            label: "Open".into(),
            action: MenuAction::Open(path.clone()),
            danger: false,
        }];
        if !is_volume {
            items.push(MenuItem {
                label: "Rename".into(),
                action: MenuAction::Rename(path.clone()),
                danger: false,
            });
        }
        items.push(MenuItem {
            label: "Copy path".into(),
            action: MenuAction::CopyPath(path.clone()),
            danger: false,
        });
        items.push(MenuItem {
            label: "Reveal in Explorer".into(),
            action: MenuAction::Reveal(path.clone()),
            danger: false,
        });
        if pinned {
            items.push(MenuItem {
                label: "Unpin from Home".into(),
                action: MenuAction::Unpin(path.clone()),
                danger: false,
            });
        }
        items.push(MenuItem {
            label: "Properties".into(),
            action: MenuAction::Properties(path),
            danger: false,
        });
        if !is_volume {
            let label = if targets.len() > 1 {
                format!("Delete {} items", targets.len()).into()
            } else {
                SharedString::from("Delete")
            };
            items.push(MenuItem {
                label,
                action: MenuAction::Delete(targets),
                danger: true,
            });
        }

        self.menu = Some(Menu { at, items });
        cx.notify();
    }

    pub fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    pub fn run(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        match action {
            MenuAction::Open(path) => self.activate(&path, window, cx),
            MenuAction::Rename(path) => self.begin_rename(path, window, cx),
            MenuAction::CopyPath(path) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                    path.to_string_lossy().into_owned(),
                ));
                self.note("Path copied.", cx);
            }
            MenuAction::Reveal(path) => {
                if let Err(err) = fs_ops::reveal(&path) {
                    self.fail(format!("Reveal failed: {err}"), cx);
                }
            }
            MenuAction::Properties(path) => self.show_properties(&path, cx),
            MenuAction::Delete(paths) => self.delete(paths, cx),
            MenuAction::Unpin(path) => self.unpin(&path, cx),
        }
        cx.notify();
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
        // Deferred so the edit (and this subscription) is not torn down from
        // inside its own callback.
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
                self.selection = vec![target];
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
                self.selection.clear();
                self.anchor = None;
                let noun = if count == 1 { "item" } else { "items" };
                self.note(format!("Moved {count} {noun} to the Recycle Bin."), cx);
                self.reload(cx);
            }
            Err(err) => self.fail(format!("Delete failed: {err}"), cx),
        }
    }

    pub fn delete_selection(&mut self, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            self.delete(self.selection.clone(), cx);
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

    pub fn set_view(&mut self, view: ViewMode, cx: &mut Context<Self>) {
        self.view = view;
        cx.notify();
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
