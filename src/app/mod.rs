//! Ply's state and behaviour. Rendering lives in [`crate::ui`].

mod nav;
mod ops;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Pixels, Point, SharedString, Task, Window,
    prelude::*,
};
use gpui_component::input::{InputEvent, InputState};

use crate::listing::{Entry, Snapshot, SortKey};
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

/// A right-click menu: a vertical list of rows plus flyout state.
pub struct Menu {
    pub at: Point<Pixels>,
    pub rows: Vec<MenuRow>,
    pub flyout: Option<usize>,
    /// Keyboard-selected row, driven by the UI via [`Menu::move_selection`].
    /// `None` means nothing is highlighted yet.
    pub selected: Option<usize>,
}

impl Menu {
    /// Step the keyboard selection by `delta`, wrapping around. Only enabled
    /// [`MenuRow::Item`] rows are stops; separators and disabled rows are
    /// skipped. A stale or missing selection restarts from the nearest end.
    /// With no selectable row the selection is cleared. Pure.
    /// UI contract: the overlay wires arrow keys to this.
    pub fn move_selection(&mut self, delta: isize) {
        let n = self.rows.len();
        let selectable = |row: &MenuRow| matches!(row, MenuRow::Item(item) if item.enabled);
        if n == 0 || !self.rows.iter().any(&selectable) {
            self.selected = None;
            return;
        }
        let step = if delta == 0 { 1 } else { delta };
        let mut ix = self
            .selected
            .filter(|&i| i < n)
            .map(|i| i as isize)
            .unwrap_or(if step > 0 { -1 } else { n as isize });
        loop {
            ix = (ix + step).rem_euclid(n as isize);
            if selectable(&self.rows[ix as usize]) {
                self.selected = Some(ix as usize);
                return;
            }
        }
    }
}

#[derive(Clone)]
pub enum MenuRow {
    Separator,
    Item(MenuItem),
}

/// Where a menu row icon comes from. The backend (`thumbs.rs`) resolves these
/// to shell rasters; `icon` on [`MenuItem`] stays as the lucide fallback glyph.
/// This is the single contract the UI reads. `thumbs.rs` keeps its own
/// worker-side `StockIcon` plus a generic stock probe; the two stay in sync by
/// name (Shield, Folder, FolderOpen, Info, RecycleBin, Delete, MixedFiles).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuIconSource {
    /// Shell icon for a real path (folder, executable, file).
    Path(std::path::PathBuf),
    /// Per-extension class icon, lowercased without the dot (for example `"txt"`).
    Class(String),
    /// Fixed stock icon.
    Stock(MenuStock),
}

/// Fixed shell icons used by menu rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuStock {
    Shield,
    Folder,
    FolderOpen,
    // No menu builder constructs these today (Properties and Delete are
    // glyph-only); they stay because `ui/overlay.rs` maps every variant to
    // a worker `StockIcon` and tests that mapping.
    #[allow(dead_code)]
    Info,
    #[allow(dead_code)]
    RecycleBin,
    #[allow(dead_code)]
    Delete,
    MixedFiles,
}

#[derive(Clone)]
pub struct MenuItem {
    pub label: SharedString,
    pub icon: Option<crate::icons::Ico>,
    /// Shell icon source. `None` means lucide only (Ply chrome rows).
    pub shell: Option<MenuIconSource>,
    pub action: Option<MenuAction>,
    pub children: Vec<MenuRow>,
    pub enabled: bool,
    pub danger: bool,
    pub strong: bool,
    /// Right-aligned accelerator hint the overlay paints (`"Enter"`, `"Del"`).
    pub shortcut: Option<SharedString>,
    /// Segoe MDL2 Symbols codepoint the overlay paints for this row
    /// (`'\u{E890}'` View, `'\u{E8CB}'` Sort by, `'\u{E8F4}'` New folder,
    /// `'\u{E7AC}'` Open with / Choose another app, `'\u{E8AC}'` Rename,
    /// `'\u{E8C8}'` Copy as path, `'\u{E838}'` Reveal, `'\u{E946}'` Properties,
    /// `'\u{E74D}'` Delete, `'\u{E72C}'` Refresh, `'\u{E756}'` Open in
    /// Terminal, `'\u{E8E5}'` Open). `None` means lucide `icon` only.
    /// UI contract: `ui/overlay.rs` reads this. Rows without a mapped
    /// codepoint (Run as admin, Pin rows, handler and sort children,
    /// List/Grid checks) leave it `None`.
    /// Properties and Delete are glyph-only: they carry no `shell` stock
    /// source, only the codepoint above plus the lucide `icon` fallback.
    pub glyph: Option<char>,
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
            shell: None,
            action,
            children: Vec::new(),
            enabled: true,
            danger: false,
            strong: false,
            shortcut: None,
            glyph: None,
        }
    }

    pub(super) fn with_shell(self, source: MenuIconSource) -> Self {
        Self {
            shell: Some(source),
            ..self
        }
    }

    pub(super) fn danger(self) -> Self {
        Self {
            danger: true,
            ..self
        }
    }

    pub(super) fn with_shortcut(self, shortcut: impl Into<SharedString>) -> Self {
        Self {
            shortcut: Some(shortcut.into()),
            ..self
        }
    }

    pub(super) fn with_glyph(self, glyph: char) -> Self {
        Self {
            glyph: Some(glyph),
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
    /// Open with a specific handler from `fs_ops::list_open_with_apps`. The display
    /// name is carried for the status line; shell `Invoke` is deferred, so the
    /// run path currently falls back to the Choose-app picker.
    OpenWithHandler(PathBuf, String),
    RunAsAdmin(PathBuf),
    OpenInTerminal(PathBuf),
    // Kept for the clipboard/pin engine that does not exist yet: no menu
    // builds these rows today, `run` still honours them when it returns.
    #[allow(dead_code)]
    Pin(PathBuf),
    #[allow(dead_code)]
    Unpin(PathBuf),
    CopyPath(PathBuf),
    #[allow(dead_code)]
    Cut,
    #[allow(dead_code)]
    Copy,
    #[allow(dead_code)]
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

/// A row being renamed inline. The subscription commits on Enter and
/// cancels on blur or Esc, and lives here so it dies with the edit.
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
    /// Byte-exact suffix for files, e.g. `"(580,833,358 bytes)"`.
    pub size_detail: SharedString,
    /// Cluster-rounded size via `fs_ops::size_on_disk`.
    pub size_on_disk: SharedString,
    /// Byte-exact `size_on_disk` suffix, e.g. `"(118,784 bytes)"`. `""` when
    /// the on-disk size is unknown (volumes, portable paths, failed reads).
    /// The overlay paints it next to [`Self::size_on_disk`].
    pub size_on_disk_detail: SharedString,
    /// `"N Files, M Folders"` for directories; `""` for files and volumes.
    pub contains: SharedString,
    pub modified: SharedString,
    /// Full-date stamps (`listing::format_full_datetime`).
    pub created: SharedString,
    pub accessed: SharedString,
    pub path: SharedString,
    /// Parent folder display (Location). Split from the name where trivial;
    /// the full path stays in [`Self::path`] for copy.
    pub location: SharedString,
    /// Friendly app name for the Opens-with row (`AssocQueryString`, falling
    /// back to the kind label when the shell has nothing). Empty for
    /// directories and volumes, where the overlay hides the row.
    pub opens_with: SharedString,
    /// Attribute checkboxes. `None` means mixed or unavailable (and, for
    /// `readonly` on directories, Explorer's tri-state note instead).
    pub readonly: Option<bool>,
    pub hidden: Option<bool>,
    /// Directories show Explorer's "applies to folder only" note; the
    /// read-only box itself stays untouched (`readonly` is `None`).
    pub attrs_note: bool,
    /// Masked attribute bits (`fs_ops::ATTR_*`) the dialog opened with, for
    /// [`Ply::properties_dirty`]. `None` when the bits could not be read.
    pub attr_orig: Option<u32>,
    /// Extra shell-sourced facts (author, title, created, ...) filled in
    /// asynchronously after the dialog opens.
    pub details: Vec<(SharedString, SharedString)>,
}

/// Arguments for [`props_from_fields`]: the facts the Properties dialog
/// shows, gathered from a real Entry or Volume — never a stub.
pub struct PropsFields {
    pub name: SharedString,
    pub kind: SharedString,
    pub size: SharedString,
    pub size_detail: SharedString,
    pub size_on_disk: SharedString,
    /// Byte-exact `size_on_disk` suffix; `""` when unknown. Converted to
    /// [`SharedString`] at the boundary like the other size strings.
    pub size_on_disk_detail: SharedString,
    pub contains: SharedString,
    pub modified: SharedString,
    pub created: SharedString,
    pub accessed: SharedString,
    pub path: SharedString,
    pub location: SharedString,
    /// Friendly app name; converted to [`SharedString`] at the boundary.
    /// Empty for directories and volumes.
    pub opens_with: String,
    pub readonly: Option<bool>,
    pub hidden: Option<bool>,
    pub attrs_note: bool,
    pub attr_orig: Option<u32>,
}

/// Build a [`Properties`] snapshot from [`PropsFields`]. Pure, so tests cover
/// the mapping without a GPUI context; `details` start empty and
/// [`Ply::fill_properties`] enriches them asynchronously.
pub fn props_from_fields(fields: PropsFields) -> Properties {
    Properties {
        name: fields.name,
        kind: fields.kind,
        size: fields.size,
        size_detail: fields.size_detail,
        size_on_disk: fields.size_on_disk,
        size_on_disk_detail: fields.size_on_disk_detail,
        contains: fields.contains,
        modified: fields.modified,
        created: fields.created,
        accessed: fields.accessed,
        path: fields.path,
        location: fields.location,
        opens_with: fields.opens_with.into(),
        readonly: fields.readonly,
        hidden: fields.hidden,
        attrs_note: fields.attrs_note,
        attr_orig: fields.attr_orig,
        details: Vec::new(),
    }
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
    /// Owned clones of `visible_indices`, kept in the same order, so the hot
    /// render path can hand out `&[Entry]` without allocating a fresh `Vec`
    /// every frame. Rebuilt alongside `visible_indices`; valid only when the
    /// listing is `Ready` and a filter is active (the unfiltered case serves
    /// the snapshot's own slice directly).
    visible_entries: Vec<Entry>,

    pub menu: Option<Menu>,
    pub properties: Option<Properties>,
    pub confirm: Option<ConfirmDialog>,
    pub rename: Option<Rename>,
    pub status: Option<SharedString>,

    pub(crate) list_generation: u64,
    list_task: Option<Task<()>>,
    /// Properties folder-walk generation: each directory dialog bumps it, and
    /// stale walk completions are dropped. The matching cancel flag aborts
    /// the previous walk's I/O early.
    props_generation: u64,
    props_walk_cancel: Arc<AtomicBool>,
    watch: Option<FolderWatch>,
    pub focus: FocusHandle,

    /// Decoded media thumbnails, keyed by path + mtime. Dropped with the window.
    pub thumbs: Entity<crate::thumbs::ThumbCache>,

    /// A thumbnail/icon completion set this when it wants a repaint. Coalesced:
    /// many completions within a short window produce at most one repaint.
    thumbs_dirty: bool,
    /// A flush timer is already scheduled; don't spawn another.
    thumbs_flush_pending: bool,
    /// A storm-settle repaint is already scheduled; don't spawn another.
    pub(crate) storm_settle_pending: bool,
    /// Paint storm detector: while the viewport travels at fling speed,
    /// listing cells paint placeholder slots instead of content
    /// thumbnails, so a fling past hundreds of files doesn't upload hundreds
    /// of GPU tiles that are visible for a frame each. Shared class icons
    /// still paint (their tiles upload once and dedupe). Updated in
    /// `Render::render`, read by the browser cell painters.
    pub(crate) thumb_storm: bool,
    storm_gate: StormGate,
    /// Entry-index range actually painted last frame (union of the
    /// virtualized rows the list/grid processors ran for), plus the listing
    /// generation it belongs to. Prefetch and the thumbnail working-set lock
    /// use this viewport window instead of the whole listing: locking 15k
    /// keys spills the LOCK_CAP and churns on-screen tiles, and rebuilding
    /// that key vector every frame is the largest main-thread cost in the
    /// app. One frame stale by construction; overscan covers the lag. The
    /// row processors overwrite it (last wins); a generation mismatch or an
    /// empty range falls back to the top of the listing.
    pub(crate) last_viewport: std::ops::Range<usize>,
    pub(crate) last_viewport_gen: u64,
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
            quick_access: volumes::load_or_seed(),
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
            visible_entries: Vec::new(),
            menu: None,
            properties: None,
            confirm: None,
            rename: None,
            status: None,
            list_generation: 0,
            list_task: None,
            props_generation: 0,
            props_walk_cancel: Arc::new(AtomicBool::new(false)),
            watch: None,
            focus: cx.focus_handle(),
            thumbs: cx.new(|_| crate::thumbs::ThumbCache::new()),
            thumbs_dirty: false,
            thumbs_flush_pending: false,
            storm_settle_pending: false,
            thumb_storm: false,
            storm_gate: StormGate::new(),
            last_viewport: 0..0,
            last_viewport_gen: 0,
        };
        ply.refresh_volumes(cx);
        ply.start_watch_poll(cx);
        ply.start_volume_poll(cx);
        ply.start_lnk_refresh(cx);
        // `gpui_component::init` (main.rs) pins the library theme to Light;
        // push Ply's opening mode into it so filter/rename inputs paint a
        // matching caret and selection from the first frame.
        sync_library_theme(ply.mode, cx);
        cx.spawn(async move |_, cx| {
            cx.background_spawn(async move { crate::thumbs::warm_shell() })
                .await;
        })
        .detach();
        // Best-effort bound on the on-disk thumbnail cache: one pass at
        // launch, off the UI thread, evicting oldest files past the cap.
        cx.spawn(async move |_, cx| {
            cx.background_spawn(async move {
                crate::cache::evict(crate::cache::DISK_CACHE_MAX_BYTES);
            })
            .await;
        })
        .detach();
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

    /// Update the paint-storm detector; called at the top of every render.
    /// A storm is a viewport jump past `TRIP` entries between two renders:
    /// a scroll fling. Comparing consecutive renders (instead of anchoring
    /// to history) means the flag can never latch on: a stationary viewport
    /// always reads no-travel on the next render, so stopping clears it on
    /// the very next paint, with or without timers. A fast extraction
    /// trickle repainting a stationary viewport never trips it, so
    /// progressive fill-in is never blanked; slow scrolls, arrow-key
    /// stepping and typing move less and stay progressive.
    pub(crate) fn note_paint(&mut self) {
        self.thumb_storm = self
            .storm_gate
            .update(self.last_viewport.start, self.list_generation);
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
        sync_library_theme(self.mode, cx);
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

    /// Set the dirty flag. Call from inside an `update` closure where `self`
    /// is already mutably borrowed.
    pub(crate) fn mark_thumbs_dirty(&mut self) {
        self.thumbs_dirty = true;
    }

    /// Schedule a flush if one is not already pending. Call from inside an
    /// `update` closure where `cx` is available.
    pub(crate) fn schedule_thumbs_flush(&mut self, cx: &mut Context<Self>) {
        if !self.thumbs_flush_pending {
            self.thumbs_flush_pending = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.thumbs_flush_pending = false;
                    if this.thumbs_dirty {
                        this.thumbs_dirty = false;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }
}

/// Fling detector with no latching state. Fed the painted viewport start
/// plus the listing generation on every render; reports whether the
/// viewport is currently whipping past content. Comparing consecutive
/// renders (instead of anchoring to history) is the whole safety story:
/// a stationary viewport always reads no-travel on the next render, so
/// stopping clears the storm on the very next paint with or without
/// timers, and a generation change resets without tripping, so navigation
/// and filtering never inherit a storm. Pure logic, no clock: unit-tested
/// below, including the no-latch regression test.
#[derive(Debug, Default)]
struct StormGate {
    prev_start: Option<usize>,
    generation: u64,
}

impl StormGate {
    fn new() -> Self {
        Self::default()
    }

    /// Feed one render. Returns true while flinging: the viewport jumped
    /// more than `TRIP` entries since the previous render.
    fn update(&mut self, vp_start: usize, generation: u64) -> bool {
        /// Entries jumped between two renders that counts as a fling
        /// rather than stepping, slow rolls, or repaint churn.
        const TRIP: usize = 40;
        let trip = match (self.prev_start, self.generation == generation) {
            (Some(prev), true) => vp_start.abs_diff(prev) > TRIP,
            // First sighting, or the listing changed under us (new folder,
            // filter rebuild): never storm on a jump we didn't observe
            // both ends of.
            _ => false,
        };
        self.prev_start = Some(vp_start);
        self.generation = generation;
        trip
    }
}

/// Library theme mode matching Ply's [`Mode`]. Pure, so tests cover the
/// mapping without a GPUI context.
pub fn library_theme_mode(mode: Mode) -> gpui_component::ThemeMode {
    match mode {
        Mode::Light => gpui_component::ThemeMode::Light,
        Mode::Dark => gpui_component::ThemeMode::Dark,
    }
}

/// Caret and selection colours the library theme should use for [`Mode`]:
/// caret tracks the Ply foreground, selection tracks `select_strong`. Pure,
/// so tests cover both modes without a GPUI context.
pub fn library_caret_selection(mode: Mode) -> (gpui::Hsla, gpui::Hsla) {
    let palette = mode.palette();
    (palette.foreground, palette.select_strong)
}

/// Push Ply's [`Mode`] into the `gpui_component` library theme. The library
/// `Input` paints its caret and selection from `cx.theme().caret` /
/// `cx.theme().selection` (see `crates/ui/src/input/input.rs` in the pinned
/// source, keys `caret` and `selection.background` in `default-theme.json`),
/// and `gpui_component::init` pins those to Light. So every Ply mode change
/// re-applies the library mode via [`gpui_component::Theme::change`] and then
/// overrides caret/selection from the Ply palette. Callers: [`Ply::new`]
/// (init) and [`Ply::toggle_mode`].
pub fn sync_library_theme(mode: Mode, cx: &mut gpui::App) {
    gpui_component::Theme::change(library_theme_mode(mode), None, cx);
    let (caret, selection) = library_caret_selection(mode);
    let theme = gpui_component::Theme::global_mut(cx);
    theme.caret = caret;
    theme.selection = selection;
    theme.tokens.caret = caret.into();
    theme.tokens.selection = selection.into();
}

/// Escape closes whatever is on top, innermost first. A menu flyout closes
/// before the menu itself.
pub fn dismiss_topmost(ply: &mut Ply, cx: &mut Context<Ply>) {
    if ply.confirm.is_some() {
        ply.cancel_confirm(cx);
    } else if ply.properties.is_some() {
        ply.close_properties(cx);
    } else if ply.menu.is_some() {
        if ply.menu.as_ref().is_some_and(|menu| menu.flyout.is_some()) {
            ply.set_flyout(None, cx);
        } else {
            ply.close_menu(cx);
        }
    } else if ply.rename.is_some() {
        ply.cancel_rename(cx);
    } else {
        ply.clear_selection(cx);
    }
}

#[cfg(test)]
mod tests {
    /// Simulates the coalescing logic outside a live GPUI context: dirty + flush
    /// flags with manual flush, matching what `thumbs_updated` / `mark_thumbs_dirty`
    /// / `schedule_thumbs_flush` do on the real model.
    struct Coalescer {
        dirty: bool,
        flush_pending: bool,
        notify_count: u32,
    }

    impl Coalescer {
        fn new() -> Self {
            Self {
                dirty: false,
                flush_pending: false,
                notify_count: 0,
            }
        }

        fn mark_dirty(&mut self) {
            self.dirty = true;
        }

        fn schedule_flush(&mut self) {
            if !self.flush_pending {
                self.flush_pending = true;
            }
        }

        fn flush(&mut self) -> bool {
            self.flush_pending = false;
            if self.dirty {
                self.dirty = false;
                self.notify_count += 1;
                true
            } else {
                false
            }
        }
    }

    #[test]
    fn single_completion_flushes() {
        let mut c = Coalescer::new();
        c.mark_dirty();
        c.schedule_flush();
        assert!(c.flush(), "dirty flag must produce a notify");
        assert_eq!(c.notify_count, 1);
        assert!(!c.dirty);
        assert!(!c.flush_pending);
    }

    #[test]
    fn many_completions_between_flushes_produce_one_notify() {
        let mut c = Coalescer::new();
        for _ in 0..100 {
            c.mark_dirty();
            c.schedule_flush();
        }
        assert!(c.flush());
        assert_eq!(c.notify_count, 1);
    }

    #[test]
    fn flush_when_clean_is_a_noop() {
        let mut c = Coalescer::new();
        assert!(!c.flush(), "no notify when nothing is dirty");
        assert_eq!(c.notify_count, 0);
    }

    #[test]
    fn flag_resets_after_flush_allows_next_cycle() {
        let mut c = Coalescer::new();
        c.mark_dirty();
        c.schedule_flush();
        c.flush();
        // Second batch
        c.mark_dirty();
        c.schedule_flush();
        assert!(c.flush());
        assert_eq!(c.notify_count, 2);
    }

    #[test]
    fn schedule_is_idempotent_while_pending() {
        let mut c = Coalescer::new();
        c.mark_dirty();
        c.schedule_flush();
        c.schedule_flush();
        c.schedule_flush();
        c.flush();
        assert_eq!(
            c.notify_count, 1,
            "only one flush even with multiple schedule calls"
        );
    }

    #[test]
    fn storm_gate_first_sighting_never_trips() {
        let mut g = super::StormGate::new();
        assert!(!g.update(800, 5), "no previous render to compare against");
    }

    #[test]
    fn storm_gate_ignores_small_steps() {
        let mut g = super::StormGate::new();
        g.update(0, 1);
        // Key-repeat stepping and slow rolls: 10 entries per render.
        for i in 1..=20 {
            assert!(!g.update(i * 10, 1), "slow motion must stay progressive");
        }
    }

    #[test]
    fn storm_gate_trips_on_jump_and_clears_on_stop() {
        let mut g = super::StormGate::new();
        g.update(0, 1);
        assert!(g.update(500, 1), "fling jump must trip");
        // Stopped dead: the very next render reads no travel and clears.
        // This is the no-latch regression test — the old sticky reference
        // kept tripping forever on a parked far viewport.
        assert!(
            !g.update(500, 1),
            "a stationary viewport must clear on the next render"
        );
        assert!(!g.update(500, 1), "stays clear while parked");
    }

    #[test]
    fn storm_gate_resets_on_generation_change() {
        let mut g = super::StormGate::new();
        g.update(0, 1);
        assert!(g.update(900, 1), "fling trips");
        // New folder (or filter rebuild): the jump is meaningless, and the
        // flag must not carry over.
        assert!(!g.update(0, 2), "generation change resets without tripping");
        assert!(!g.update(0, 2), "fresh listing starts quiet");
    }

    #[test]
    fn storm_gate_keeps_tripping_while_moving() {
        let mut g = super::StormGate::new();
        g.update(0, 1);
        assert!(g.update(200, 1));
        assert!(g.update(400, 1), "sustained fling stays gated");
        assert!(g.update(600, 1));
    }

    fn menu_item(label: &str, enabled: bool) -> super::MenuRow {
        super::MenuItem {
            enabled,
            ..super::MenuItem::new(label, None, None)
        }
        .into()
    }

    fn menu_with(rows: Vec<super::MenuRow>) -> super::Menu {
        super::Menu {
            at: gpui::Point::new(gpui::px(0.), gpui::px(0.)),
            rows,
            flyout: None,
            selected: None,
        }
    }

    #[test]
    fn menu_item_shortcut_defaults_none_and_builder_sets() {
        let plain = super::MenuItem::new("Open", None, None);
        assert!(plain.shortcut.is_none());
        let with = super::MenuItem::new("Delete", None, None).with_shortcut("Del");
        assert_eq!(with.shortcut, Some(gpui::SharedString::from("Del")));
    }

    #[test]
    fn menu_item_glyph_defaults_none_and_builder_sets() {
        let plain = super::MenuItem::new("Open", None, None);
        assert!(plain.glyph.is_none());
        let with = super::MenuItem::new("View", None, None).with_glyph('\u{E890}');
        assert_eq!(with.glyph, Some('\u{E890}'));
    }

    #[test]
    fn menu_move_selection_skips_separators_and_disabled_and_wraps() {
        let mut menu = menu_with(vec![
            menu_item("Open", true),
            super::MenuRow::Separator,
            menu_item("Gone", false),
            menu_item("Properties", true),
        ]);
        assert!(menu.selected.is_none());
        menu.move_selection(1);
        assert_eq!(menu.selected, Some(0));
        menu.move_selection(1);
        assert_eq!(menu.selected, Some(3), "must skip separator + disabled");
        menu.move_selection(1);
        assert_eq!(menu.selected, Some(0), "must wrap past the end");
        menu.move_selection(-1);
        assert_eq!(menu.selected, Some(3), "must wrap past the start");
        menu.move_selection(-1);
        assert_eq!(menu.selected, Some(0), "backwards must skip too");
    }

    #[test]
    fn menu_move_selection_clears_when_nothing_is_selectable() {
        let mut empty = menu_with(Vec::new());
        empty.move_selection(1);
        assert!(empty.selected.is_none());
        let mut all_off = menu_with(vec![super::MenuRow::Separator, menu_item("Gone", false)]);
        all_off.move_selection(1);
        assert!(all_off.selected.is_none());
        // A stale index past a rebuilt shorter menu restarts cleanly.
        let mut stale = menu_with(vec![menu_item("Only", true)]);
        stale.selected = Some(7);
        stale.move_selection(1);
        assert_eq!(stale.selected, Some(0));
    }

    #[test]
    fn library_theme_mode_follows_ply_mode() {
        assert_eq!(
            super::library_theme_mode(crate::theme::Mode::Light),
            gpui_component::ThemeMode::Light
        );
        assert_eq!(
            super::library_theme_mode(crate::theme::Mode::Dark),
            gpui_component::ThemeMode::Dark
        );
    }

    #[test]
    fn library_caret_selection_tracks_palette_in_both_modes() {
        for mode in [crate::theme::Mode::Light, crate::theme::Mode::Dark] {
            let palette = mode.palette();
            assert_eq!(
                super::library_caret_selection(mode),
                (palette.foreground, palette.select_strong),
                "caret must equal foreground and selection must equal select_strong in {mode:?}"
            );
        }
        // The sync must actually flip something, or toggling would be a no-op.
        assert_ne!(
            super::library_caret_selection(crate::theme::Mode::Light),
            super::library_caret_selection(crate::theme::Mode::Dark)
        );
    }
}
