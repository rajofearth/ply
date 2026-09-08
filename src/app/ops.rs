use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime};

use gpui::{Context, Pixels, Point, SharedString, Window, prelude::*};
use gpui_component::input::{InputEvent, InputState};

use crate::fs_ops;
use crate::icons::Ico;
use crate::listing::{Entry, Snapshot, SortKey, list_sorted};
use crate::volumes;

use super::{
    ConfirmAction, ConfirmDialog, LoadState, Menu, MenuAction, MenuIconSource, MenuItem, MenuRow,
    MenuStock, Ply, PropsFields, Rename, ViewMode, props_from_fields,
};

/// Facts gathered for a Properties dialog before the async fills land:
/// display name, kind label, whether it is a directory, file bytes when
/// known, and modified/created/accessed stamps.
type PropFacts = (
    SharedString,
    String,
    bool,
    Option<u64>,
    Option<SystemTime>,
    Option<SystemTime>,
    Option<SystemTime>,
);

/// Segoe MDL2 Symbols codepoints the overlay paints per menu row. This is the
/// `MenuItem::glyph` contract `ui/overlay.rs` reads: rows without a mapped
/// codepoint here (Run as admin, Pin rows, handler/sort children, List/Grid
/// checks) carry `None`.
const GLYPH_OPEN: char = '\u{E8E5}';
const GLYPH_OPEN_WITH: char = '\u{E7AC}';
const GLYPH_TERMINAL: char = '\u{E756}';
const GLYPH_RENAME: char = '\u{E8AC}';
const GLYPH_COPY_PATH: char = '\u{E8C8}';
const GLYPH_REVEAL: char = '\u{E838}';
const GLYPH_PROPERTIES: char = '\u{E946}';
const GLYPH_DELETE: char = '\u{E74D}';
const GLYPH_VIEW: char = '\u{E890}';
const GLYPH_SORT_BY: char = '\u{E8CB}';
const GLYPH_NEW_FOLDER: char = '\u{E8F4}';
const GLYPH_REFRESH: char = '\u{E72C}';

impl Ply {
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
        let key = self.sort;
        if !matches!(self.listing, LoadState::Ready(_)) {
            self.listing = LoadState::Loading;
        }
        self.list_task = Some(cx.spawn(async move |this, cx| {
            let scan_folder = folder.clone();
            let snapshot = cx
                .background_spawn(async move { list_sorted(&scan_folder, key) })
                .await;
            this.update(cx, |this, cx| {
                if this.list_generation != generation {
                    return;
                }
                match snapshot {
                    Ok(snapshot) => {
                        // A watch-driven reload usually finds nothing new;
                        // replacing an identical listing only causes churn.
                        if let LoadState::Ready(current) = &this.listing
                            && current.same_contents(&snapshot)
                        {
                            return;
                        }
                        let batch_entries: Vec<_> = snapshot
                            .entries
                            .iter()
                            .take(crate::thumbs::TYPE_ICON_BATCH_CAP)
                            .cloned()
                            .collect();
                        this.remember_names(&snapshot);
                        // Commit names now. Type icons are pre-resolved by the
                        // detached batch below and steered in when they land;
                        // they never gate this paint (a stuck thumbnail can
                        // no longer hold the listing at "Loading").
                        this.listing = LoadState::Ready(snapshot);
                        this.rebuild_visible();
                        let folder = folder.clone();
                        cx.spawn(async move |this, cx| {
                            let icons = cx
                                .background_spawn(async move {
                                    crate::thumbs::resolve_listing_type_icons(&batch_entries)
                                })
                                .await;
                            let _ = this.update(cx, |this, cx| {
                                if this.list_generation != generation
                                    || this.current_folder() != Some(folder.as_path())
                                {
                                    return;
                                }
                                if let Some(icons) = icons
                                    && let LoadState::Ready(snapshot) = &this.listing
                                {
                                    this.thumb_cache().update(cx, |c, _| {
                                        c.apply_listing_icons(&snapshot.entries, &icons);
                                    });
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                    }
                    Err(err) => {
                        this.listing = LoadState::Failed(err.to_string().into());
                        this.visible_indices.clear();
                        this.visible_entries.clear();
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Portable-device paths are object IDs, so keep the names the listing
    /// reported; nothing else can recover them later.
    pub(super) fn remember_names(&mut self, snapshot: &Snapshot) {
        for entry in &snapshot.entries {
            if crate::mtp::is_mtp(&entry.path) {
                self.mtp_names
                    .insert(entry.path.clone(), entry.name.clone());
            }
        }
    }

    pub(super) fn start_watch_poll(&mut self, cx: &mut Context<Self>) {
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

    /// Periodically re-resolve `.lnk` icon sources so a rebuilt target or a
    /// replaced icon file refreshes on its own, without the link's mtime
    /// changing or the folder being touched.
    pub(super) fn start_lnk_refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(4000))
                    .await;
                let lnks = this
                    .update(cx, |this, _| {
                        this.visible()
                            .iter()
                            .filter(|e| {
                                Path::new(&e.name)
                                    .extension()
                                    .is_some_and(|x| x.eq_ignore_ascii_case("lnk"))
                            })
                            .map(|e| e.path.clone())
                            .collect::<Vec<_>>()
                    })
                    .ok();
                let Some(lnks) = lnks else {
                    break;
                };
                if lnks.is_empty() {
                    continue;
                }
                let _ = this.update(cx, |_this, cx| crate::thumbs::refresh_lnk(&lnks, cx));
            }
        })
        .detach();
    }

    /// Notice drives appearing and disappearing. Windows delivers this as
    /// `WM_DEVICECHANGE`, which GPUI does not surface, so poll instead.
    /// Lettered discover runs only when `GetLogicalDrives` changes; MTP refreshes
    /// on a slower cadence so an expensive WPD scan never rides every tick.
    pub(super) fn start_volume_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut last_mask = volumes::logical_drives_mask();
            let mut ticks_since_sizes: u32 = 0;
            let mut ticks_since_mtp: u32 = 0;
            const SIZES_EVERY_TICKS: u32 = 2; // ~3s at 1.5s/tick, home only
            const MTP_EVERY_TICKS: u32 = 7; // ~10.5s at 1.5s/tick
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;

                let mask = volumes::logical_drives_mask();
                if mask != last_mask {
                    // Sequential: an unreachable network share can stall lettered
                    // discovery; awaiting keeps lettered polls from stacking.
                    let lettered = cx
                        .background_spawn(async { volumes::discover_lettered() })
                        .await;
                    if this
                        .update(cx, |this, cx| {
                            let mtp: Vec<_> = this
                                .volumes
                                .iter()
                                .filter(|v| crate::mtp::is_mtp(&v.path))
                                .cloned()
                                .collect();
                            let found = volumes::merge_lettered_and_mtp(lettered, mtp);
                            if this.volumes != found {
                                this.volumes = found;
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                    last_mask = mask;
                }

                // Keep local free-space sizes live while Home is showing, without
                // re-querying network/MTP (which keep their own cadence below).
                ticks_since_sizes = ticks_since_sizes.saturating_add(1);
                if ticks_since_sizes >= SIZES_EVERY_TICKS {
                    ticks_since_sizes = 0;
                    let volumes = this
                        .update(cx, |this, _| this.is_home().then(|| this.volumes.clone()))
                        .ok()
                        .flatten();
                    if let Some(volumes) = volumes {
                        let updated = cx
                            .background_spawn(async move { volumes::refresh_local_sizes(&volumes) })
                            .await;
                        this.update(cx, |this, cx| {
                            // `refresh_local_sizes` returns only changed volumes;
                            // nothing to paint when it's empty, so skip the
                            // re-render (Home should stay idle otherwise).
                            if updated.is_empty() {
                                return;
                            }
                            for v in updated {
                                if let Some(slot) = this
                                    .volumes
                                    .iter_mut()
                                    .find(|s| s.path == v.path && s.kind == v.kind)
                                {
                                    slot.free = v.free;
                                    slot.total = v.total;
                                }
                            }
                            cx.notify();
                        })
                        .ok();
                    }
                }

                ticks_since_mtp = ticks_since_mtp.saturating_add(1);
                if ticks_since_mtp < MTP_EVERY_TICKS {
                    continue;
                }
                ticks_since_mtp = 0;
                let mtp = cx
                    .background_spawn(async { volumes::discover_mtp_devices() })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        let lettered: Vec<_> = this
                            .volumes
                            .iter()
                            .filter(|v| !crate::mtp::is_mtp(&v.path))
                            .cloned()
                            .collect();
                        let found = volumes::merge_lettered_and_mtp(lettered, mtp);
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

    /// Rebuild [`Ply::visible_indices`] from the Ready listing and filter, and
    /// mirror the result into the owned [`Ply::visible_entries`] cache so the
    /// render path never allocates a fresh `Vec` per frame.
    pub(super) fn rebuild_visible(&mut self) {
        self.visible_indices = Vec::new();
        self.visible_entries = Vec::new();
        let LoadState::Ready(snapshot) = &self.listing else {
            return;
        };
        let indices = filter_indices(&snapshot.entries, &self.filter_text);
        if self.filter_text.is_empty() {
            // Unfiltered: `visible()` serves the snapshot's own slice, so no
            // clone cache is kept here; only the index set needs rebuilding.
            self.visible_indices = indices;
            return;
        }
        self.visible_entries = indices
            .iter()
            .filter_map(|&i| snapshot.entries.get(i))
            .cloned()
            .collect();
        self.visible_indices = indices;
    }

    /// Entries in the current folder that survive the filter box, as a slice in
    /// `visible_indices` order. No per-call allocation: the unfiltered case
    /// reborrows the snapshot's own entries, and the filtered case returns the
    /// owned cache rebuilt by [`Self::rebuild_visible`].
    pub fn visible(&self) -> &[Entry] {
        match &self.listing {
            LoadState::Ready(snapshot) if self.filter_text.is_empty() => &snapshot.entries,
            LoadState::Ready(_) => &self.visible_entries,
            _ => &[],
        }
    }

    /// Count of filtered entries, matching the slice `visible()` returns.
    pub fn visible_len(&self) -> usize {
        self.visible().len()
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
        let text = crate::ui::filter_placeholder(count);
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

    pub(super) fn clear_selection_paths(&mut self) {
        self.selection.clear();
        self.selection_set.clear();
    }

    fn replace_selection(&mut self, paths: Vec<PathBuf>) {
        self.selection_set = paths.iter().cloned().collect();
        self.selection = paths;
    }

    pub fn is_selected(&self, path: &Path) -> bool {
        self.selection_set.contains(path)
    }

    pub fn click_row(&mut self, ix: usize, extend: bool, toggle: bool, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.visible().iter().map(|e| e.path.clone()).collect();
        let Some(path) = paths.get(ix).cloned() else {
            return;
        };
        if extend && let Some(anchor) = self.anchor {
            let (lo, hi) = if anchor <= ix {
                (anchor, ix)
            } else {
                (ix, anchor)
            };
            self.replace_selection(paths[lo..=hi].to_vec());
        } else if toggle {
            match self.selection.iter().position(|p| *p == path) {
                Some(at) => {
                    let removed = self.selection.remove(at);
                    self.selection_set.remove(&removed);
                }
                None => {
                    self.selection_set.insert(path.clone());
                    self.selection.push(path);
                }
            }
            self.anchor = Some(ix);
        } else {
            self.replace_selection(vec![path]);
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
            self.replace_selection(paths[lo..=hi].to_vec());
            self.anchor = Some(anchor);
        } else {
            self.replace_selection(vec![paths[next].clone()]);
            self.anchor = Some(next);
        }
        cx.notify();
    }

    /// Grid-aware arrow-key movement. `cols` is the estimated column count.
    pub fn move_grid_selection(
        &mut self,
        cols: usize,
        right: isize,
        down: isize,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        let paths: Vec<PathBuf> = self.visible().iter().map(|e| e.path.clone()).collect();
        if paths.is_empty() || cols == 0 {
            return;
        }
        let cur = self
            .selection
            .last()
            .and_then(|last| paths.iter().position(|p| p == last))
            .unwrap_or(0);
        let row = cur / cols;
        let col = cur % cols;
        let total = paths.len();
        let last_row_len = total.saturating_sub((total / cols) * cols);
        let max_row = if last_row_len == 0 {
            total / cols - 1
        } else {
            total / cols
        };

        // Horizontal first, then vertical.
        let new_col = (col as isize + right).clamp(0, cols as isize - 1) as usize;
        let new_row = (row as isize + down).clamp(0, max_row as isize) as usize;

        // Clamp to actual row length (last row may be partial).
        let row_len = if new_row == max_row {
            let r = total - new_row * cols;
            if r == 0 { cols } else { r }
        } else {
            cols
        };
        let new_col = new_col.min(row_len - 1);
        let next = new_row * cols + new_col;

        if extend {
            let anchor = self.anchor.unwrap_or(cur);
            let (lo, hi) = if anchor <= next {
                (anchor, next)
            } else {
                (next, anchor)
            };
            self.replace_selection(paths[lo..=hi].to_vec());
            self.anchor = Some(anchor);
        } else {
            self.replace_selection(vec![paths[next].clone()]);
            self.anchor = Some(next);
        }
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.clear_selection_paths();
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
        if self
            .current_folder()
            .is_some_and(crate::recycle_bin::is_recycle_bin)
        {
            // The Recycle Bin is browse-only: items have no openable path.
            return;
        }
        if self.is_folder(path) {
            self.open_folder(path.to_path_buf(), window, cx);
        } else if !crate::path_caps::for_path(path).open_direct {
            self.open_from_device(path.to_path_buf(), cx);
        } else if let Err(err) = fs_ops::open_with_os(path) {
            self.fail(format!("Could not open: {err}"), cx);
        }
    }

    /// `is_dir` cannot answer for portable devices, so trust the listing that
    /// produced the path and fall back to the filesystem for everything else.
    fn is_folder(&self, path: &Path) -> bool {
        if let Some(entry) = self.listing_entry(path) {
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

    // ---- menu, properties, file operations --------------------------------

    pub fn open_menu(&mut self, at: Point<Pixels>, path: PathBuf, cx: &mut Context<Self>) {
        if !self.is_selected(&path) {
            self.replace_selection(vec![path.clone()]);
        }
        let is_volume = self.volumes.iter().any(|v| v.path == path);
        let caps = crate::path_caps::for_path(&path);
        let writable = caps.rename;
        let targets = if self.selection.len() > 1 {
            self.selection.clone()
        } else {
            vec![path.clone()]
        };
        let multi = targets.len() > 1;
        let is_dir = self
            .listing_entry(&path)
            .map(Entry::is_directory)
            .unwrap_or_else(|| path.is_dir());
        let is_file = !is_dir && !is_volume;
        let admin = !multi && is_file && fs_ops::is_admin_target(&path) && writable;

        // The Recycle Bin is browse-only: its items carry shell parsing IDs, so
        // none of the mutating actions (cut/copy/rename/delete) apply there.
        let browse_only = self
            .current_folder()
            .is_some_and(crate::recycle_bin::is_recycle_bin);

        // Every row lives in the list, and rows that cannot fire are
        // omitted instead of disabled. Cut/Copy/Paste stay out entirely
        // until a clipboard engine exists.

        // `None` means Open-with is not offered (not a file);
        // `Some` (possibly empty) drives the collapse in `build_item_menu`.
        // Every file is offered Open-with, writable or not; locals resolve
        // handlers while portable paths fall back to the Choose-app row.
        let open_with = is_file.then(|| {
            fs_ops::list_open_with_apps(&path)
                .into_iter()
                .take(fs_ops::OPEN_WITH_CAP)
                .collect::<Vec<_>>()
        });
        let rows = build_item_menu(&ItemMenuSpec {
            path: path.clone(),
            targets,
            multi,
            is_dir,
            is_volume,
            admin,
            writable,
            browse_only,
            pinned: self.quick_access.contains(&path),
            reveal: caps.reveal,
            trash: caps.trash,
            open_with,
        });

        self.show_menu(at, rows, cx);
    }

    pub fn open_empty_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(folder) = self.current_folder().map(Path::to_path_buf) else {
            return;
        };
        let writable = crate::path_caps::for_path(&folder).rename;
        let view = self.view;
        let sort = self.sort;

        let rows = build_empty_menu(view, sort, writable, folder);
        self.show_menu(at, rows, cx);
    }

    /// Open the right-click menu for a sidebar row. Unlike [`Ply::open_menu`]
    /// this never touches the listing selection: the menu targets the single
    /// row path, so right-clicking the rail cannot steal the centre pane's
    /// selection. Home and the Recycle Bin attach no menu. Sidebar wiring
    /// (which row calls this) lives with the parent.
    // Allowed until the parent wires the rail to this entry point.
    #[allow(dead_code)]
    pub fn open_sidebar_menu(&mut self, at: Point<Pixels>, path: PathBuf, cx: &mut Context<Self>) {
        let Some(spec) = classify_sidebar(&path, &self.volumes, &self.quick_access) else {
            return;
        };
        let rows = build_sidebar_menu(&spec);
        if rows.is_empty() {
            return;
        }
        self.show_menu(at, rows, cx);
    }

    fn show_menu(&mut self, at: Point<Pixels>, rows: Vec<MenuRow>, cx: &mut Context<Self>) {
        self.menu = Some(Menu {
            at,
            rows,
            flyout: None,
            selected: None,
        });
        cx.notify();
    }

    fn listing_entry(&self, path: &Path) -> Option<&Entry> {
        match &self.listing {
            LoadState::Ready(snap) => snap.entries.iter().find(|e| e.path == path),
            _ => None,
        }
    }

    pub fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    pub fn set_flyout(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            menu.flyout = if menu.flyout == ix { None } else { ix };
            cx.notify();
        }
    }

    pub fn run(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        match action {
            MenuAction::Open(path) => self.activate(&path, window, cx),
            MenuAction::ChooseApp(path) => {
                self.try_fs(fs_ops::choose_another(&path), "Choose app failed", cx)
            }
            MenuAction::OpenWithHandler(path, name) => {
                // Shell Invoke is deferred, so fall back to the picker with a
                // note naming the chosen handler.
                self.note(
                    format!("Opening with \"{name}\" needs shell Invoke; showing picker."),
                    cx,
                );
                self.try_fs(fs_ops::choose_another(&path), "Choose app failed", cx)
            }
            MenuAction::RunAsAdmin(path) => {
                self.try_fs(fs_ops::run_as_admin(&path), "Could not elevate", cx)
            }
            MenuAction::OpenInTerminal(path) => {
                self.try_fs(fs_ops::open_terminal(&path), "Terminal failed", cx)
            }
            MenuAction::Pin(path) => self.pin(path, cx),
            MenuAction::Unpin(path) => self.unpin(&path, cx),
            MenuAction::CopyPath(path) => {
                // Single-path copy today; `join_paths_for_copy` degrades to a
                // quoted single path and keeps the multi groundwork live.
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                    fs_ops::join_paths_for_copy(std::slice::from_ref(&path)),
                ));
                self.note("Path copied.", cx);
            }
            MenuAction::Cut | MenuAction::Copy | MenuAction::Paste => {}
            MenuAction::Rename(path) => self.begin_rename(path, window, cx),
            MenuAction::Delete(paths) => self.delete(paths, cx),
            MenuAction::Reveal(path) => self.try_fs(fs_ops::reveal(&path), "Reveal failed", cx),
            MenuAction::Properties(path) => self.show_properties(&path, cx),
            MenuAction::Refresh => self.reload(cx),
            MenuAction::SetView(view) => self.set_view(view, cx),
            MenuAction::SetSort(key) => self.set_sort(key, cx),
            MenuAction::NewFolder => self.new_folder(window, cx),
        }
        cx.notify();
    }

    fn try_fs(&mut self, result: anyhow::Result<()>, prefix: &str, cx: &mut Context<Self>) {
        if let Err(err) = result {
            self.fail(format!("{prefix}: {err}"), cx);
        }
    }

    fn new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self.current_folder().map(Path::to_path_buf) else {
            return;
        };
        match fs_ops::create_folder(&parent, "New folder") {
            Ok(path) => {
                self.reload(cx);
                self.replace_selection(vec![path.clone()]);
                self.begin_rename(path, window, cx);
            }
            Err(err) => self.fail(err.to_string(), cx),
        }
    }

    pub fn begin_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Explorer match: the stem is pre-selected so typing replaces the
        // name but keeps the extension. Set through the wrapper (so its
        // cached value stays correct for commit) and then select on the
        // base state: `prepare` re-applies a builder default_value on first
        // paint and `set_value` resets the selection, which would wipe a
        // range set any earlier.
        let stem = rename_select_range(&name);
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.set_value(name, window, cx);
            let base = input.base_state().clone();
            base.update(cx, |base, cx| base.set_selected_range(stem, cx));
        });
        // Deferred so the edit (and this subscription) is not torn down from
        // inside its own callback. Enter commits; losing focus cancels, like
        // Explorer. Esc reaches `dismiss_topmost`, which cancels too.
        let commit = cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
            let Some(action) = rename_event_action(event) else {
                return;
            };
            let ply = cx.entity();
            cx.defer(move |cx| {
                ply.update(cx, |this, cx| match action {
                    RenameEventAction::Commit => this.commit_rename(cx),
                    RenameEventAction::Cancel => this.cancel_rename(cx),
                });
            });
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
                self.replace_selection(vec![target]);
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

    /// Delete selected entries. On a volume that supports a Recycle Bin this
    /// moves them to trash; anywhere else (removable/CD/network devices) it
    /// asks the user to confirm a permanent delete first, like Explorer.
    /// Drive/device/Recycle-Bin roots are refused outright.
    pub fn delete(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        // Browsing the Recycle Bin is read-only: its items carry shell parsing IDs,
        // not trashable filesystem paths, so going through the normal delete would
        // mis-target them.
        if self
            .current_folder()
            .is_some_and(crate::recycle_bin::is_recycle_bin)
        {
            self.note("The Recycle Bin is browse-only.", cx);
            return;
        }
        let (trash, permanent) = match fs_ops::plan_delete(&paths) {
            Err(e) => {
                // Refused: a drive/device root is in the batch. Fail closed, no dialog,
                // nothing is deleted.
                self.note(format!("{e}"), cx);
                return;
            }
            Ok(pair) => pair,
        };
        if !trash.is_empty() {
            self.delete_to_trash(trash, cx);
        }
        if !permanent.is_empty() {
            self.request_confirm_delete(permanent, cx);
        }
    }

    fn finish_delete_ok(&mut self, note: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.clear_selection_paths();
        self.anchor = None;
        self.note(note, cx);
        self.reload(cx);
    }

    fn finish_delete_err(&mut self, err: anyhow::Error, cx: &mut Context<Self>) {
        self.fail(format!("Delete failed: {err}"), cx);
    }

    fn delete_to_trash(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let count = paths.len();
        match fs_ops::delete_to_trash(&paths) {
            Ok(()) => {
                let note = if count == 1 {
                    "Moved to the Recycle Bin.".to_string()
                } else {
                    format!("Moved {count} to the Recycle Bin.")
                };
                self.finish_delete_ok(note, cx);
            }
            Err(err) => self.finish_delete_err(err, cx),
        }
    }

    /// Show a permanent-delete confirmation for volumes with no Recycle Bin.
    fn request_confirm_delete(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let (message, confirm_text) = if paths.len() == 1 {
            (
                format!(
                    "\"{}\" will be permanently deleted.\nThis can't be undone.",
                    paths[0]
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| paths[0].to_string_lossy().into_owned())
                ),
                "Delete forever".to_string(),
            )
        } else {
            (
                format!(
                    "{} items will be permanently deleted.\nThis can't be undone.",
                    paths.len()
                ),
                "Delete forever".to_string(),
            )
        };
        self.confirm = Some(ConfirmDialog {
            title: "Delete permanently?".into(),
            message: message.into(),
            confirm_text: confirm_text.into(),
            danger: true,
            action: ConfirmAction::DeletePermanently(paths),
        });
        cx.notify();
    }

    /// Run the confirmed action. Close the dialog first so state is clean.
    pub fn run_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.confirm.take() else {
            return;
        };
        match dialog.action {
            ConfirmAction::DeletePermanently(paths) => match fs_ops::delete_permanently(&paths) {
                Ok(()) => {
                    let note = if paths.len() == 1 {
                        "Deleted permanently.".to_string()
                    } else {
                        format!("Deleted {} permanently.", paths.len())
                    };
                    self.finish_delete_ok(note, cx);
                }
                Err(err) => self.finish_delete_err(err, cx),
            },
        }
        cx.notify();
    }

    pub fn cancel_confirm(&mut self, cx: &mut Context<Self>) {
        if self.confirm.take().is_some() {
            cx.notify();
        }
    }

    pub fn delete_selection(&mut self, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            self.delete(self.selection.clone(), cx);
        }
    }

    pub fn show_properties(&mut self, path: &Path, cx: &mut Context<Self>) {
        let path_display: SharedString = path.to_string_lossy().into_owned().into();
        let location: SharedString = split_location(path).into();
        let now = chrono::Local::now();

        if let Some(volume) = self.volumes.iter().find(|v| v.path == path) {
            let kind = match volume.kind {
                volumes::VolumeKind::Drive => "Local Drive",
                volumes::VolumeKind::Device => "Removable Device",
                volumes::VolumeKind::Network => "Network Drive",
            };
            self.set_props(
                PropsFields {
                    name: volume.name.clone().into(),
                    kind: kind.into(),
                    size: format!(
                        "{} free of {}",
                        crate::listing::format_size(volume.free),
                        crate::listing::format_size(volume.total)
                    )
                    .into(),
                    size_detail: "".into(),
                    size_on_disk: "".into(),
                    size_on_disk_detail: "".into(),
                    contains: "".into(),
                    modified: "—".into(),
                    created: "—".into(),
                    accessed: "—".into(),
                    path: path_display,
                    location,
                    // Volumes have no Opens-with target; the overlay hides
                    // the row when this is empty.
                    opens_with: String::new(),
                    readonly: None,
                    hidden: None,
                    attrs_note: false,
                    attr_orig: None,
                },
                cx,
            );
            return;
        }

        // Facts first: prefer the listing entry that produced the path, fall
        // back to a direct stat. Dates are full stamps; the attributes come
        // from the OS agent's bit reader below.
        let (name, kind, is_dir, bytes, modified, created, accessed): PropFacts =
            if let Some(entry) = self.listing_entry(path) {
                let is_dir = entry.is_directory();
                let (created, accessed) = meta_dates(path);
                (
                    entry.name.clone().into(),
                    crate::listing::kind_label(entry).to_string(),
                    is_dir,
                    (!is_dir).then_some(entry.size),
                    entry.modified,
                    created,
                    accessed,
                )
            } else {
                let meta = if crate::path_caps::is_portable(path) {
                    None
                } else {
                    std::fs::metadata(path).ok()
                };
                let name = self.display_name(path);
                match &meta {
                    Some(m) if m.is_dir() => (
                        name,
                        "Folder".to_string(),
                        true,
                        None,
                        m.modified().ok(),
                        m.created().ok(),
                        m.accessed().ok(),
                    ),
                    Some(m) => {
                        let file_name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| name.to_string());
                        (
                            name,
                            crate::listing::kind_label_for_name(&file_name).to_string(),
                            false,
                            Some(m.len()),
                            m.modified().ok(),
                            m.created().ok(),
                            m.accessed().ok(),
                        )
                    }
                    None => (name, "—".to_string(), false, None, None, None, None),
                }
            };

        let opens_with = opens_with_display(path, &kind, is_dir);
        let bits = fs_ops::attr_bits(path);
        // Directories show Explorer's folder note instead of a read-only box;
        // the hidden bit still reflects the directory itself.
        let readonly = if is_dir {
            None
        } else {
            bits.map(|b| b & fs_ops::ATTR_READONLY != 0)
        };
        let hidden = bits.map(|b| b & fs_ops::ATTR_HIDDEN != 0);
        let (size, size_detail, size_on_disk, size_on_disk_detail, contains) = if is_dir {
            let calculating: SharedString = "Calculating…".into();
            (
                calculating.clone(),
                calculating.clone(),
                calculating.clone(),
                "".into(),
                calculating,
            )
        } else {
            match bytes {
                Some(logical) => {
                    let (on_disk, on_disk_detail) =
                        size_on_disk_strings(fs_ops::size_on_disk(path, logical));
                    (
                        crate::listing::format_size(logical).into(),
                        format_byte_detail(logical).into(),
                        on_disk,
                        on_disk_detail,
                        "".into(),
                    )
                }
                None => ("—".into(), "—".into(), "—".into(), "".into(), "".into()),
            }
        };
        self.set_props(
            PropsFields {
                name,
                kind: kind.into(),
                size,
                size_detail,
                size_on_disk,
                size_on_disk_detail,
                contains,
                modified: crate::listing::format_full_datetime(modified, now).into(),
                created: crate::listing::format_full_datetime(created, now).into(),
                accessed: crate::listing::format_full_datetime(accessed, now).into(),
                path: path_display,
                location,
                opens_with,
                readonly,
                hidden,
                attrs_note: is_dir,
                attr_orig: bits,
            },
            cx,
        );
        if is_dir {
            self.spawn_props_walk(path.to_path_buf(), cx);
        }
        self.fill_properties(path, cx);
    }

    /// Total a directory off the UI thread, then fill Size / Contains. The
    /// generation guard drops results from a dialog that has moved on, and
    /// the same-path check drops results for a dialog that reopened
    /// elsewhere. Mirrors the [`Self::fill_properties`] spawn pattern.
    fn spawn_props_walk(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.props_walk_cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.props_walk_cancel = cancel.clone();
        self.props_generation += 1;
        let generation = self.props_generation;
        cx.spawn(async move |this, cx| {
            let check = path.clone();
            let walked = cx
                .background_spawn(async move {
                    fs_ops::walk_folder(&path, &cancel)
                        .map(|summary| (summary, fs_ops::size_on_disk(&path, summary.bytes)))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.props_generation != generation {
                    return;
                }
                let still_open = this
                    .properties
                    .as_ref()
                    .is_some_and(|p| Path::new(p.path.as_str()) == check.as_path());
                if !still_open {
                    return;
                }
                if let Some(props) = this.properties.as_mut() {
                    match walked {
                        Some((summary, on_disk)) => {
                            props.size = crate::listing::format_size(summary.bytes).into();
                            props.size_detail = format_byte_detail(summary.bytes).into();
                            let (disk, disk_detail) = size_on_disk_strings(on_disk);
                            props.size_on_disk = disk;
                            props.size_on_disk_detail = disk_detail;
                            props.contains = format_contains(summary.files, summary.folders).into();
                        }
                        None => {
                            let unavailable: SharedString = "Unavailable".into();
                            props.size = unavailable.clone();
                            props.size_detail = unavailable.clone();
                            props.size_on_disk = unavailable.clone();
                            props.size_on_disk_detail = "".into();
                            props.contains = unavailable;
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn set_props(&mut self, fields: PropsFields, cx: &mut Context<Self>) {
        self.properties = Some(props_from_fields(fields));
        cx.notify();
    }

    /// Asynchronously enrich an open Properties dialog with shell-sourced
    /// facts (author, title, created, ...) read via `IPropertyStore` on the
    /// STA worker, so the dialog never blocks on the shell.
    pub fn fill_properties(&mut self, path: &Path, cx: &mut Context<Self>) {
        if crate::path_caps::is_portable(path) {
            return;
        }
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let rows = cx
                .background_spawn(async move { crate::thumbs::read_properties(&path) })
                .await;
            if rows.is_empty() {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                if let Some(props) = this.properties.as_mut() {
                    props.details = filtered_details(rows);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Write the dialog's attribute checkboxes back with one
    /// `fs_ops::set_attr_bits` call. `None` boxes are left alone. Notes on
    /// failure; always notifies so the footer can repaint.
    pub fn apply_properties(&mut self, cx: &mut Context<Self>) {
        let Some(props) = self.properties.as_ref() else {
            return;
        };
        let path = PathBuf::from(props.path.to_string());
        let orig = props.attr_orig.unwrap_or(0);
        let mut set_mask = 0u32;
        let mut clear_mask = 0u32;
        match props.readonly {
            Some(true) => set_mask |= fs_ops::ATTR_READONLY,
            Some(false) => clear_mask |= fs_ops::ATTR_READONLY,
            None => {}
        }
        match props.hidden {
            Some(true) => set_mask |= fs_ops::ATTR_HIDDEN,
            Some(false) => clear_mask |= fs_ops::ATTR_HIDDEN,
            None => {}
        }
        if set_mask == 0 && clear_mask == 0 {
            return;
        }
        match fs_ops::set_attr_bits(&path, set_mask, clear_mask) {
            Ok(()) => {
                if let Some(props) = self.properties.as_mut() {
                    props.attr_orig = Some((orig | set_mask) & !clear_mask);
                }
            }
            Err(err) => self.fail(format!("Could not set attributes: {err}"), cx),
        }
        cx.notify();
    }

    /// Whether the dialog's attribute boxes differ from the bits it opened
    /// with. Pure logic lives in [`attrs_dirty`] so tests skip the context.
    pub fn properties_dirty(&self) -> bool {
        let Some(props) = self.properties.as_ref() else {
            return false;
        };
        attrs_dirty(props.attr_orig, props.readonly, props.hidden)
    }

    pub fn close_properties(&mut self, cx: &mut Context<Self>) {
        if self.properties.take().is_some() {
            cx.notify();
        }
    }
}

/// What a rename-edit input event does: Enter commits the new name, losing
/// focus cancels (Explorer match; Esc cancels via `dismiss_topmost`). Pure,
/// so tests cover the mapping without a GPUI context.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RenameEventAction {
    Commit,
    Cancel,
}

fn rename_event_action(event: &InputEvent) -> Option<RenameEventAction> {
    match event {
        InputEvent::PressEnter { .. } => Some(RenameEventAction::Commit),
        InputEvent::Blur => Some(RenameEventAction::Cancel),
        _ => None,
    }
}

/// Byte range to pre-select when a rename edit opens: the stem up to the
/// last dot, so typing replaces the name but keeps the extension.
/// Extensionless names and dotfiles (a leading dot) select all. Pure: the
/// dot is ASCII, so the split is always a UTF-8 boundary.
fn rename_select_range(name: &str) -> std::ops::Range<usize> {
    if name.starts_with('.') {
        return 0..name.len();
    }
    match name.rfind('.') {
        Some(dot) => 0..dot,
        None => 0..name.len(),
    }
}

fn row(
    label: impl Into<SharedString>,
    icon: Ico,
    action: MenuAction,
    glyph: Option<char>,
) -> MenuRow {
    let item = MenuItem::new(label, Some(icon), Some(action));
    match glyph {
        Some(g) => item.with_glyph(g).into(),
        None => item.into(),
    }
}

fn row_short(
    label: impl Into<SharedString>,
    icon: Ico,
    action: MenuAction,
    shortcut: &str,
    glyph: Option<char>,
) -> MenuRow {
    let item = MenuItem::new(label, Some(icon), Some(action)).with_shortcut(shortcut);
    match glyph {
        Some(g) => item.with_glyph(g).into(),
        None => item.into(),
    }
}

fn row_with_shell(
    label: impl Into<SharedString>,
    icon: Ico,
    action: MenuAction,
    shell: Option<MenuIconSource>,
    glyph: Option<char>,
) -> MenuRow {
    let item = MenuItem::new(label, Some(icon), Some(action));
    let item = match glyph {
        Some(g) => item.with_glyph(g),
        None => item,
    };
    match shell {
        Some(source) => item.with_shell(source).into(),
        None => item.into(),
    }
}

/// Shell source for an Open row: real path for folders, class for files with
/// an extension, path otherwise. Pure, so the menu never blocks on the shell.
fn shell_for_open_target(path: &Path, is_dir: bool) -> MenuIconSource {
    if is_dir {
        MenuIconSource::Path(path.to_path_buf())
    } else if let Some(ext) = path
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty())
    {
        MenuIconSource::Class(ext.to_ascii_lowercase())
    } else {
        MenuIconSource::Path(path.to_path_buf())
    }
}

/// Open row source, with multi-select collapsing to the mixed-files stock.
fn open_shell_source(path: &Path, is_dir: bool, multi: bool) -> MenuIconSource {
    if multi {
        MenuIconSource::Stock(MenuStock::MixedFiles)
    } else {
        shell_for_open_target(path, is_dir)
    }
}

/// Parent folder display for the Location row. Where trivial (a parent
/// exists) this is the dir; volume roots and parentless paths fall back to
/// the full path.
fn split_location(path: &Path) -> String {
    path.parent()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Opens-with display name: shell friendly app name, falling back to the kind
/// label the listing already shows.
fn opens_with_for(path: &Path, kind_fallback: &str) -> String {
    fs_ops::friendly_app_name(path).unwrap_or_else(|| kind_fallback.to_string())
}

/// Opens-with value for the dialog: directories and volumes carry none (the
/// overlay hides the row when this is empty), files resolve as before.
fn opens_with_display(path: &Path, kind: &str, is_dir: bool) -> String {
    if is_dir {
        String::new()
    } else {
        opens_with_for(path, kind)
    }
}

/// Created/accessed stamps without following the dialog into the shell: a
/// single stat on the calling thread. Portable paths have no statable
/// metadata, so both are unknown.
fn meta_dates(path: &Path) -> (Option<SystemTime>, Option<SystemTime>) {
    if crate::path_caps::is_portable(path) {
        return (None, None);
    }
    match std::fs::metadata(path) {
        Ok(meta) => (meta.created().ok(), meta.accessed().ok()),
        Err(_) => (None, None),
    }
}

/// Byte-exact size suffix, e.g. `"(580,833,358 bytes)"`. Pure.
fn format_byte_detail(n: u64) -> String {
    let digits = n.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("({grouped} bytes)")
}

/// Display pair for a cluster-rounded on-disk size: the Explorer value plus
/// its byte-exact detail, e.g. `("4.0 KB", "(4,096 bytes)")`. Unknown
/// (volumes, portable paths, failed reads) is `("—", "")`: the value keeps
/// its dash while the detail stays empty. Pure.
fn size_on_disk_strings(on_disk: Option<u64>) -> (SharedString, SharedString) {
    match on_disk {
        Some(n) => (
            crate::listing::format_size(n).into(),
            format_byte_detail(n).into(),
        ),
        None => ("—".into(), "".into()),
    }
}

/// Explorer-style Contains line for a finished folder walk. Pure.
fn format_contains(files: u64, folders: u64) -> String {
    let files = if files == 1 {
        "1 File".to_string()
    } else {
        format!("{files} Files")
    };
    let folders = if folders == 1 {
        "1 Folder".to_string()
    } else {
        format!("{folders} Folders")
    };
    format!("{files}, {folders}")
}

/// Backend detail labels that duplicate first-class Properties rows. The
/// shell worker reports these too; the dialog keeps only the enriched rest.
fn is_duplicate_detail(label: &str) -> bool {
    matches!(
        label,
        "Type"
            | "Size"
            | "Size on disk"
            | "Created"
            | "Modified"
            | "Accessed"
            | "Attributes"
            | "Contains"
    )
}

/// Map backend rows into dialog details, dropping the duplicates above.
/// Pure, so the filter is testable without a GPUI context.
fn filtered_details(rows: Vec<(String, String)>) -> Vec<(SharedString, SharedString)> {
    rows.into_iter()
        .filter(|(label, _)| !is_duplicate_detail(label))
        .map(|(k, v)| (k.into(), v.into()))
        .collect()
}

/// Whether `readonly`/`hidden` boxes differ from the opening bits. `None`
/// boxes keep the opening bit. Pure.
fn attrs_dirty(orig: Option<u32>, readonly: Option<bool>, hidden: Option<bool>) -> bool {
    const KNOWN: u32 = fs_ops::ATTR_READONLY | fs_ops::ATTR_HIDDEN;
    let orig = orig.unwrap_or(0) & KNOWN;
    let mut current = orig;
    match readonly {
        Some(true) => current |= fs_ops::ATTR_READONLY,
        Some(false) => current &= !fs_ops::ATTR_READONLY,
        None => {}
    }
    match hidden {
        Some(true) => current |= fs_ops::ATTR_HIDDEN,
        Some(false) => current &= !fs_ops::ATTR_HIDDEN,
        None => {}
    }
    current != orig
}

/// Pure inputs for the item (right-click) menu. Grouping, separators, glyphs,
/// the Pin row and the Open-with collapse live in [`build_item_menu`], which
/// takes this spec so tests cover them without a GPUI context. Shell queries
/// (`list_open_with_apps`, `terminal_exe_path`, caps) stay in
/// [`Ply::open_menu`], which fills the spec.
struct ItemMenuSpec {
    path: PathBuf,
    targets: Vec<PathBuf>,
    multi: bool,
    is_dir: bool,
    is_volume: bool,
    admin: bool,
    writable: bool,
    browse_only: bool,
    pinned: bool,
    reveal: bool,
    trash: bool,
    /// `None` means Open-with is not offered (not a file);
    /// `Some` apps (possibly empty) drive the collapse below.
    open_with: Option<Vec<fs_ops::OpenWithApp>>,
}

/// Build the item menu rows: `[Open, Open with>] / [Open in Terminal, Pin,
/// Rename, Copy as path, Reveal] / [Properties, Delete]` with exactly two
/// separators. Shortcuts and shell sources are unchanged from the old inline
/// build. Pure.
fn build_item_menu(spec: &ItemMenuSpec) -> Vec<MenuRow> {
    let path = &spec.path;
    let mut rows = vec![
        MenuItem {
            strong: true,
            shell: Some(open_shell_source(path, spec.is_dir, spec.multi)),
            ..MenuItem::new(
                "Open",
                Some(Ico::ExternalLink),
                Some(MenuAction::Open(path.clone())),
            )
            .with_shortcut("Enter")
            .with_glyph(GLYPH_OPEN)
        }
        .into(),
    ];
    if let Some(handlers) = &spec.open_with {
        if handlers.is_empty() {
            // Single-child collapse: a direct row firing ChooseApp instead
            // of a one-child flyout.
            rows.push(
                MenuItem::new(
                    "Open with…",
                    Some(Ico::ExternalLink),
                    Some(MenuAction::ChooseApp(path.clone())),
                )
                .with_glyph(GLYPH_OPEN_WITH)
                .into(),
            );
        } else {
            let mut kids: Vec<MenuRow> = Vec::new();
            for app in handlers.iter().take(fs_ops::OPEN_WITH_CAP) {
                // Handler shell icon: the exe path when the shell resolved
                // one, else the lucide fallback (`shell: None`).
                let shell = (!app.icon.as_os_str().is_empty())
                    .then(|| MenuIconSource::Path(app.icon.clone()));
                let item = MenuItem::new(
                    app.name.clone(),
                    None,
                    Some(MenuAction::OpenWithHandler(path.clone(), app.name.clone())),
                );
                kids.push(match shell {
                    Some(source) => item.with_shell(source).into(),
                    None => item.into(),
                });
            }
            kids.push(
                MenuItem::new(
                    "Choose another app…",
                    None,
                    Some(MenuAction::ChooseApp(path.clone())),
                )
                .with_glyph(GLYPH_OPEN_WITH)
                .into(),
            );
            rows.push(flyout(
                "Open with",
                Ico::ExternalLink,
                kids,
                Some(GLYPH_OPEN_WITH),
            ));
        }
    }
    if spec.admin {
        rows.push(row_with_shell(
            "Run as admin",
            Ico::Shield,
            MenuAction::RunAsAdmin(path.clone()),
            Some(MenuIconSource::Stock(MenuStock::Shield)),
            None,
        ));
    }
    rows.push(MenuRow::Separator);
    // Files open the terminal at their parent dir; `fs_ops::open_terminal`
    // falls back to the parent for file paths, so both kinds share this row.
    // The bin is browse-only, so its items keep no Terminal row, as before.
    if !spec.multi && spec.writable && (spec.is_dir || !spec.is_volume) && !spec.browse_only {
        rows.push(row_with_shell(
            "Open in Terminal",
            Ico::Terminal,
            MenuAction::OpenInTerminal(path.clone()),
            Some(MenuIconSource::Path(fs_ops::terminal_exe_path())),
            Some(GLYPH_TERMINAL),
        ));
    }
    if spec.is_dir && !spec.is_volume {
        rows.push(if spec.pinned {
            row(
                "Remove from Quick Access",
                Ico::PinOff,
                MenuAction::Unpin(path.clone()),
                None,
            )
        } else {
            row(
                "Add to Quick Access",
                Ico::Pin,
                MenuAction::Pin(path.clone()),
                None,
            )
        });
    }
    if !spec.multi && spec.writable && !spec.is_volume && !spec.browse_only {
        rows.push(row_short(
            "Rename",
            Ico::Pencil,
            MenuAction::Rename(path.clone()),
            "F2",
            Some(GLYPH_RENAME),
        ));
    }
    rows.push(row_short(
        "Copy as path",
        Ico::Copy,
        MenuAction::CopyPath(path.clone()),
        "Ctrl+Shift+C",
        Some(GLYPH_COPY_PATH),
    ));
    if spec.reveal {
        rows.push(row_with_shell(
            "Reveal in Explorer",
            Ico::Folder,
            MenuAction::Reveal(path.clone()),
            Some(MenuIconSource::Stock(MenuStock::FolderOpen)),
            Some(GLYPH_REVEAL),
        ));
    }
    rows.push(MenuRow::Separator);
    // Glyph-only: no shell stock source, just the MDL2 codepoint plus the
    // lucide fallback.
    rows.push(row_short(
        "Properties",
        Ico::Info,
        MenuAction::Properties(path.clone()),
        "Alt+Enter",
        Some(GLYPH_PROPERTIES),
    ));
    if !spec.is_volume && !spec.browse_only && spec.trash {
        let label = if spec.targets.len() > 1 {
            format!("Delete {}", spec.targets.len())
        } else {
            "Delete".into()
        };
        // Glyph-only like Properties above: no recycle-bin/delete stock.
        rows.push(
            MenuItem::new(
                label,
                Some(Ico::Trash),
                Some(MenuAction::Delete(spec.targets.clone())),
            )
            .danger()
            .with_shortcut("Del")
            .with_glyph(GLYPH_DELETE)
            .into(),
        );
    }
    rows
}

/// Build the empty-space menu rows: `[View>, Sort by>, Refresh] /
/// [New folder] / [Open in Terminal, Properties]`. List/Grid children keep
/// their checks and carry no glyph. Pure.
fn build_empty_menu(
    view: ViewMode,
    sort: SortKey,
    writable: bool,
    folder: PathBuf,
) -> Vec<MenuRow> {
    let mut rows = vec![
        flyout(
            "View",
            Ico::LayoutGrid,
            [
                ("List", Ico::List, ViewMode::List),
                ("Grid", Ico::LayoutGrid, ViewMode::Grid),
            ]
            .into_iter()
            .map(|(label, ico, mode)| {
                marked(label, Some(ico), MenuAction::SetView(mode), view == mode)
            })
            .collect(),
            Some(GLYPH_VIEW),
        ),
        flyout(
            "Sort by",
            Ico::ArrowUpDown,
            [
                ("Name", SortKey::Name),
                ("Date modified", SortKey::Modified),
                ("Type", SortKey::Kind),
                ("Size", SortKey::Size),
            ]
            .into_iter()
            .map(|(label, key)| marked(label, None, MenuAction::SetSort(key), sort == key))
            .collect(),
            Some(GLYPH_SORT_BY),
        ),
        row(
            "Refresh",
            Ico::Refresh,
            MenuAction::Refresh,
            Some(GLYPH_REFRESH),
        ),
    ];
    // "New" was a flyout with a single child; a direct row says the same.
    if writable {
        rows.push(MenuRow::Separator);
        rows.push(row_with_shell(
            "New folder",
            Ico::FolderPlus,
            MenuAction::NewFolder,
            Some(MenuIconSource::Stock(MenuStock::Folder)),
            Some(GLYPH_NEW_FOLDER),
        ));
    }
    rows.push(MenuRow::Separator);
    if writable {
        rows.push(row_with_shell(
            "Open in Terminal",
            Ico::Terminal,
            MenuAction::OpenInTerminal(folder.clone()),
            Some(MenuIconSource::Path(fs_ops::terminal_exe_path())),
            Some(GLYPH_TERMINAL),
        ));
    }
    rows.push(row(
        "Properties",
        Ico::Info,
        MenuAction::Properties(folder),
        Some(GLYPH_PROPERTIES),
    ));
    rows
}

/// Which sidebar section a right-clicked row belongs to. Pinned folders and
/// expanded subfolders share the Folder rows; lettered and network volumes
/// share the Volume rows; portable devices get the Device rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SidebarKind {
    Folder,
    Volume,
    Device,
}

/// Pure inputs for the sidebar menu. See [`build_sidebar_menu`].
struct SidebarMenuSpec {
    path: PathBuf,
    kind: SidebarKind,
    /// Only meaningful for [`SidebarKind::Folder`]: toggles the Pin row.
    pinned: bool,
}

/// Classify a sidebar path for its right-click menu. `None` means no menu:
/// Home has no path to reach here, and the Recycle Bin row stays bare.
/// A volume whose kind is portable (MTP devices surface as
/// [`volumes::VolumeKind::Device`]) is a Device; lettered and network
/// volumes are Volumes; anything else on the rail is a Folder. Pure, so
/// tests cover the mapping without a GPUI context.
fn classify_sidebar(
    path: &Path,
    volumes: &[crate::volumes::Volume],
    quick_access: &[PathBuf],
) -> Option<SidebarMenuSpec> {
    if crate::recycle_bin::is_recycle_bin(path) {
        return None;
    }
    if let Some(volume) = volumes.iter().find(|v| v.path == path) {
        let kind = if crate::mtp::is_mtp(path) || volume.kind == crate::volumes::VolumeKind::Device
        {
            SidebarKind::Device
        } else {
            SidebarKind::Volume
        };
        return Some(SidebarMenuSpec {
            path: path.to_path_buf(),
            kind,
            pinned: false,
        });
    }
    Some(SidebarMenuSpec {
        path: path.to_path_buf(),
        kind: SidebarKind::Folder,
        pinned: quick_access.iter().any(|p| p.as_path() == path),
    })
}

/// Build the sidebar menu rows from a [`SidebarMenuSpec`]. Folder rows are
/// `[Open] / [Open in Terminal, Pin, Copy as path, Reveal] / [Properties,
/// Delete]`; Volume rows drop Pin and Delete (`Copy path` keeps its old
/// label); Device rows keep only `[Open, Copy path] / [Properties]`.
/// Properties and Delete are glyph-only like the item menu. Pure.
fn build_sidebar_menu(spec: &SidebarMenuSpec) -> Vec<MenuRow> {
    let path = &spec.path;
    let mut rows = vec![
        MenuItem {
            strong: true,
            shell: Some(open_shell_source(path, true, false)),
            ..MenuItem::new(
                "Open",
                Some(Ico::ExternalLink),
                Some(MenuAction::Open(path.clone())),
            )
            .with_shortcut("Enter")
            .with_glyph(GLYPH_OPEN)
        }
        .into(),
    ];
    let copy_label = match spec.kind {
        SidebarKind::Folder => "Copy as path",
        SidebarKind::Volume | SidebarKind::Device => "Copy path",
    };
    match spec.kind {
        SidebarKind::Folder | SidebarKind::Volume => {
            rows.push(MenuRow::Separator);
            rows.push(row_with_shell(
                "Open in Terminal",
                Ico::Terminal,
                MenuAction::OpenInTerminal(path.clone()),
                Some(MenuIconSource::Path(fs_ops::terminal_exe_path())),
                Some(GLYPH_TERMINAL),
            ));
            if spec.kind == SidebarKind::Folder {
                rows.push(if spec.pinned {
                    row(
                        "Remove from Quick Access",
                        Ico::PinOff,
                        MenuAction::Unpin(path.clone()),
                        None,
                    )
                } else {
                    row(
                        "Add to Quick Access",
                        Ico::Pin,
                        MenuAction::Pin(path.clone()),
                        None,
                    )
                });
            }
            rows.push(row_short(
                copy_label,
                Ico::Copy,
                MenuAction::CopyPath(path.clone()),
                "Ctrl+Shift+C",
                Some(GLYPH_COPY_PATH),
            ));
            rows.push(row_with_shell(
                "Reveal in Explorer",
                Ico::Folder,
                MenuAction::Reveal(path.clone()),
                Some(MenuIconSource::Stock(MenuStock::FolderOpen)),
                Some(GLYPH_REVEAL),
            ));
            rows.push(MenuRow::Separator);
            rows.push(row_short(
                "Properties",
                Ico::Info,
                MenuAction::Properties(path.clone()),
                "Alt+Enter",
                Some(GLYPH_PROPERTIES),
            ));
            if spec.kind == SidebarKind::Folder {
                rows.push(
                    MenuItem::new(
                        "Delete",
                        Some(Ico::Trash),
                        Some(MenuAction::Delete(vec![path.clone()])),
                    )
                    .danger()
                    .with_shortcut("Del")
                    .with_glyph(GLYPH_DELETE)
                    .into(),
                );
            }
        }
        SidebarKind::Device => {
            rows.push(row_short(
                copy_label,
                Ico::Copy,
                MenuAction::CopyPath(path.clone()),
                "Ctrl+Shift+C",
                Some(GLYPH_COPY_PATH),
            ));
            rows.push(MenuRow::Separator);
            rows.push(row_short(
                "Properties",
                Ico::Info,
                MenuAction::Properties(path.clone()),
                "Alt+Enter",
                Some(GLYPH_PROPERTIES),
            ));
        }
    }
    rows
}

fn flyout(
    label: impl Into<SharedString>,
    icon: Ico,
    children: Vec<MenuRow>,
    glyph: Option<char>,
) -> MenuRow {
    let item = MenuItem::new(label, Some(icon), None);
    let item = match glyph {
        Some(g) => item.with_glyph(g),
        None => item,
    };
    MenuItem { children, ..item }.into()
}

fn marked(
    label: impl Into<SharedString>,
    icon: Option<Ico>,
    action: MenuAction,
    on: bool,
) -> MenuRow {
    MenuItem {
        strong: on,
        ..MenuItem::new(label, icon, Some(action))
    }
    .into()
}

/// Indices into `entries` that survive the filter, in listing order. An empty
/// filter keeps every index. Pure and ordered, so the owned `visible_entries`
/// cache and the index set stay identical to the old `Vec<&Entry>` slice.
fn filter_indices(entries: &[Entry], filter: &str) -> Vec<usize> {
    if filter.is_empty() {
        return (0..entries.len()).collect();
    }
    let needle = filter.to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.name.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::listing::EntryKind;

    fn file(name: &str) -> Entry {
        Entry {
            path: PathBuf::from(name),
            name: name.into(),
            kind: EntryKind::File,
            size: 0,
            modified: None,
            hidden: false,
        }
    }

    type FilterFn = fn(&[Entry], &str) -> Vec<usize>;

    /// The plain, always-correct filter: substring match in listing order. The
    /// cache builder must match this on every input.
    fn reference_filter(entries: &[Entry], filter: &str) -> Vec<usize> {
        if filter.is_empty() {
            return (0..entries.len()).collect();
        }
        let needle = filter.to_lowercase();
        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn empty_filter_keeps_every_index_in_order() {
        let entries = [file("b.txt"), file("A.txt"), file("c.txt")];
        assert_eq!(filter_indices(&entries, ""), [0, 1, 2]);
    }

    #[test]
    fn filtered_index_set_matches_reference_and_keeps_order() {
        let entries: FilterFn = filter_indices;
        let reference: FilterFn = reference_filter;
        let names = ["alpha.txt", "beta.mp4", "almanac.png", "delta.log"];
        for filter in ["", "a", "AL", "pha", "mp4", "xyz", "m", "l"] {
            let list: Vec<Entry> = names.iter().map(|n| file(n)).collect();
            assert_eq!(
                entries(&list, filter),
                reference(&list, filter),
                "filter {filter:?} must match the reference subset"
            );
        }
    }

    #[test]
    fn filtered_cache_is_in_display_when_resolved_by_index() {
        // What `rebuild_visible` does: clone entries at the returned indices
        // and serve them in the same order as the old index-mapped slice.
        let entries = vec![
            file("alpha.txt"),
            file("beta.mp4"),
            file("almanac.png"),
            file("delta.log"),
        ];
        let idx = filter_indices(&entries, "al");
        assert_eq!(idx, [0, 2]);
        let cached: Vec<Entry> = idx
            .iter()
            .filter_map(|&i| entries.get(i))
            .cloned()
            .collect();
        assert_eq!(cached.len(), 2);
        assert_eq!(cached[0].name, "alpha.txt");
        assert_eq!(cached[1].name, "almanac.png");
    }

    #[test]
    fn open_shell_uses_path_for_dirs_and_class_for_files() {
        assert_eq!(
            shell_for_open_target(Path::new(r"C:\pics"), true),
            MenuIconSource::Path(PathBuf::from(r"C:\pics"))
        );
        assert_eq!(
            shell_for_open_target(Path::new(r"C:\a\notes.txt"), false),
            MenuIconSource::Class("txt".into())
        );
        assert_eq!(
            shell_for_open_target(Path::new(r"C:\a\Makefile"), false),
            MenuIconSource::Path(PathBuf::from(r"C:\a\Makefile"))
        );
    }

    #[test]
    fn open_shell_multi_select_uses_mixed_stock() {
        assert_eq!(
            open_shell_source(Path::new(r"C:\a.txt"), false, true),
            MenuIconSource::Stock(MenuStock::MixedFiles)
        );
        assert!(matches!(
            open_shell_source(Path::new(r"C:\a.txt"), false, false),
            MenuIconSource::Class(_)
        ));
    }

    fn file_spec() -> ItemMenuSpec {
        let path = PathBuf::from(r"C:\a\notes.txt");
        ItemMenuSpec {
            targets: vec![path.clone()],
            path,
            multi: false,
            is_dir: false,
            is_volume: false,
            admin: false,
            writable: true,
            browse_only: false,
            pinned: false,
            reveal: true,
            trash: true,
            open_with: Some(Vec::new()),
        }
    }

    fn dir_spec() -> ItemMenuSpec {
        let path = PathBuf::from(r"C:\pics");
        ItemMenuSpec {
            targets: vec![path.clone()],
            path,
            multi: false,
            is_dir: true,
            is_volume: false,
            admin: false,
            writable: true,
            browse_only: false,
            pinned: false,
            reveal: true,
            trash: true,
            open_with: None,
        }
    }

    fn row_labels(rows: &[MenuRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                MenuRow::Item(item) => item.label.to_string(),
                MenuRow::Separator => "<sep>".into(),
            })
            .collect()
    }

    fn find_item<'a>(rows: &'a [MenuRow], label: &str) -> &'a MenuItem {
        rows.iter()
            .filter_map(|row| match row {
                MenuRow::Item(item) => Some(item),
                MenuRow::Separator => None,
            })
            .find(|item| item.label.as_ref() == label)
            .unwrap_or_else(|| panic!("menu row {label:?} missing"))
    }

    #[test]
    fn item_menu_file_order_is_open_first_delete_last_with_two_seps() {
        let rows = build_item_menu(&file_spec());
        assert_eq!(
            row_labels(&rows),
            [
                "Open",
                "Open with…",
                "<sep>",
                "Open in Terminal",
                "Rename",
                "Copy as path",
                "Reveal in Explorer",
                "<sep>",
                "Properties",
                "Delete",
            ]
        );
    }

    #[test]
    fn item_menu_dir_order_groups_terminal_pin_with_two_seps() {
        let rows = build_item_menu(&dir_spec());
        assert_eq!(
            row_labels(&rows),
            [
                "Open",
                "<sep>",
                "Open in Terminal",
                "Add to Quick Access",
                "Rename",
                "Copy as path",
                "Reveal in Explorer",
                "<sep>",
                "Properties",
                "Delete",
            ]
        );
    }

    #[test]
    fn item_menu_glyphs_on_key_rows() {
        let file_rows = build_item_menu(&file_spec());
        let glyph = |rows: &Vec<MenuRow>, label| find_item(rows, label).glyph;
        assert_eq!(glyph(&file_rows, "Open"), Some('\u{E8E5}'));
        assert_eq!(glyph(&file_rows, "Open with…"), Some('\u{E7AC}'));
        assert_eq!(glyph(&file_rows, "Rename"), Some('\u{E8AC}'));
        assert_eq!(glyph(&file_rows, "Copy as path"), Some('\u{E8C8}'));
        assert_eq!(glyph(&file_rows, "Reveal in Explorer"), Some('\u{E838}'));
        assert_eq!(glyph(&file_rows, "Properties"), Some('\u{E946}'));
        assert_eq!(glyph(&file_rows, "Delete"), Some('\u{E74D}'));
        let dir_rows = build_item_menu(&dir_spec());
        assert_eq!(glyph(&dir_rows, "Open in Terminal"), Some('\u{E756}'));
        let empty = build_empty_menu(
            ViewMode::List,
            SortKey::default(),
            true,
            PathBuf::from(r"C:\pics"),
        );
        assert_eq!(glyph(&empty, "View"), Some('\u{E890}'));
        assert_eq!(glyph(&empty, "Sort by"), Some('\u{E8CB}'));
        assert_eq!(glyph(&empty, "Refresh"), Some('\u{E72C}'));
        assert_eq!(glyph(&empty, "New folder"), Some('\u{E8F4}'));
        assert_eq!(glyph(&empty, "Open in Terminal"), Some('\u{E756}'));
        assert_eq!(glyph(&empty, "Properties"), Some('\u{E946}'));
    }

    #[test]
    fn empty_menu_groups_view_sort_refresh_then_new_folder() {
        let rows = build_empty_menu(
            ViewMode::List,
            SortKey::default(),
            true,
            PathBuf::from(r"C:\pics"),
        );
        assert_eq!(
            row_labels(&rows),
            [
                "View",
                "Sort by",
                "Refresh",
                "<sep>",
                "New folder",
                "<sep>",
                "Open in Terminal",
                "Properties",
            ]
        );
    }

    #[test]
    fn open_with_collapses_to_direct_row_when_no_handlers() {
        let rows = build_item_menu(&file_spec());
        let direct = find_item(&rows, "Open with…");
        assert!(
            matches!(direct.action, Some(MenuAction::ChooseApp(_))),
            "collapsed row must fire ChooseApp"
        );
        assert!(
            rows.iter().all(|row| match row {
                MenuRow::Item(item) => item.label.as_ref() != "Open with",
                MenuRow::Separator => true,
            }),
            "no one-child Open-with flyout may remain"
        );
    }

    #[test]
    fn open_with_flyout_lists_handlers_then_picker() {
        let mut spec = file_spec();
        spec.open_with = Some(vec![
            fs_ops::OpenWithApp {
                name: "FooApp".to_string(),
                icon: PathBuf::from(r"C:\Program Files\Foo\foo.exe"),
            },
            fs_ops::OpenWithApp {
                name: "NoIcon".to_string(),
                icon: PathBuf::new(),
            },
        ]);
        let rows = build_item_menu(&spec);
        let fly = find_item(&rows, "Open with");
        assert_eq!(fly.glyph, Some('\u{E7AC}'));
        assert!(fly.action.is_none());
        let kids: Vec<String> = fly
            .children
            .iter()
            .map(|row| match row {
                MenuRow::Item(item) => item.label.to_string(),
                MenuRow::Separator => "<sep>".into(),
            })
            .collect();
        assert_eq!(kids, ["FooApp", "NoIcon", "Choose another app…"]);
        let MenuRow::Item(handler) = &fly.children[0] else {
            panic!("first flyout child must be the handler");
        };
        assert!(
            matches!(&handler.action, Some(MenuAction::OpenWithHandler(_, n)) if n == "FooApp")
        );
        // Resolved exe path becomes the shell source.
        assert_eq!(
            handler.shell,
            Some(MenuIconSource::Path(PathBuf::from(
                r"C:\Program Files\Foo\foo.exe"
            )))
        );
        let MenuRow::Item(no_icon) = &fly.children[1] else {
            panic!("second flyout child must be the icon-less handler");
        };
        // Empty icon path keeps the lucide fallback (`shell: None`).
        assert!(no_icon.shell.is_none());
        assert!(no_icon.glyph.is_none());
        let MenuRow::Item(picker) = &fly.children[2] else {
            panic!("third flyout child must be the picker");
        };
        assert!(matches!(picker.action, Some(MenuAction::ChooseApp(_))));
        assert_eq!(picker.glyph, Some('\u{E7AC}'));
    }

    #[test]
    fn pin_row_present_for_dirs_only() {
        let unpinned = build_item_menu(&dir_spec());
        let add = find_item(&unpinned, "Add to Quick Access");
        assert!(matches!(add.action, Some(MenuAction::Pin(_))));
        let mut pinned_spec = dir_spec();
        pinned_spec.pinned = true;
        let pinned = build_item_menu(&pinned_spec);
        let remove = find_item(&pinned, "Remove from Quick Access");
        assert!(matches!(remove.action, Some(MenuAction::Unpin(_))));
        assert!(
            pinned.iter().all(|row| match row {
                MenuRow::Item(item) => item.label.as_ref() != "Add to Quick Access",
                MenuRow::Separator => true,
            }),
            "pinned dirs offer Remove, not Add"
        );
        let file_rows = build_item_menu(&file_spec());
        assert!(
            file_rows.iter().all(|row| match row {
                MenuRow::Item(item) => {
                    item.label.as_ref() != "Add to Quick Access"
                        && item.label.as_ref() != "Remove from Quick Access"
                }
                MenuRow::Separator => true,
            }),
            "files must not offer Pin rows"
        );
        let mut volume_spec = dir_spec();
        volume_spec.is_volume = true;
        assert!(
            build_item_menu(&volume_spec).iter().all(|row| match row {
                MenuRow::Item(item) => {
                    item.label.as_ref() != "Add to Quick Access"
                        && item.label.as_ref() != "Remove from Quick Access"
                }
                MenuRow::Separator => true,
            }),
            "volumes must not offer Pin rows"
        );
    }

    #[test]
    fn split_location_keeps_dir_and_falls_back_on_root() {
        assert_eq!(
            split_location(Path::new(r"C:\Users\me\notes.txt")),
            r"C:\Users\me"
        );
        let root = Path::new(r"C:\");
        assert_eq!(split_location(root), root.to_string_lossy());
    }

    #[test]
    fn opens_with_falls_back_to_kind_when_shell_has_nothing() {
        assert_eq!(
            opens_with_for(Path::new(r"\\MTP\DEVICE\o1"), "Text Document"),
            "Text Document"
        );
        assert_eq!(
            opens_with_for(Path::new(r"C:\noext-file-xyz"), "File"),
            "File"
        );
    }

    #[test]
    fn copy_path_quoting_only_on_spaces() {
        assert_eq!(
            crate::fs_ops::quote_path_for_copy(Path::new(r"C:\a.txt")),
            r#""C:\a.txt""#
        );
        assert_eq!(
            crate::fs_ops::quote_path_for_copy(Path::new(r"C:\my docs\a.txt")),
            r#""C:\my docs\a.txt""#
        );
        let joined = crate::fs_ops::join_paths_for_copy(&[
            PathBuf::from(r"C:\a.txt"),
            PathBuf::from(r"C:\my docs\b.txt"),
        ]);
        assert_eq!(joined, "\"C:\\a.txt\"\n\"C:\\my docs\\b.txt\"");
    }

    #[test]
    fn portable_paths_offer_no_mutating_caps() {
        let caps = crate::path_caps::for_path(Path::new(r"\\MTP\DEVICE\o1"));
        assert!(!caps.rename && !caps.trash && !caps.reveal);
        let local = crate::path_caps::for_path(Path::new(r"C:\Users"));
        assert!(local.rename && local.trash && local.reveal);
    }

    #[test]
    fn props_fields_map_every_field_and_start_with_no_details() {
        let props = props_from_fields(PropsFields {
            name: "notes.txt".into(),
            kind: "Text Document".into(),
            size: "1.0 KB".into(),
            size_detail: "(1,024 bytes)".into(),
            size_on_disk: "4.0 KB".into(),
            size_on_disk_detail: "(4,096 bytes)".into(),
            contains: "".into(),
            modified: "Monday, December 1, 2025, 2:08:28 PM".into(),
            created: "Sunday, November 30, 2025, 9:00:00 AM".into(),
            accessed: "Monday, December 1, 2025, 2:08:28 PM".into(),
            path: r"C:\Users\me\notes.txt".into(),
            location: r"C:\Users\me".into(),
            opens_with: "Notepad".to_string(),
            readonly: Some(false),
            hidden: Some(false),
            attrs_note: false,
            attr_orig: Some(0),
        });
        assert_eq!(props.name, SharedString::from("notes.txt"));
        assert_eq!(props.kind, SharedString::from("Text Document"));
        assert_eq!(props.size, SharedString::from("1.0 KB"));
        assert_eq!(props.size_detail, SharedString::from("(1,024 bytes)"));
        assert_eq!(props.size_on_disk, SharedString::from("4.0 KB"));
        assert_eq!(
            props.size_on_disk_detail,
            SharedString::from("(4,096 bytes)")
        );
        assert!(props.contains.is_empty());
        assert_eq!(
            props.modified,
            SharedString::from("Monday, December 1, 2025, 2:08:28 PM")
        );
        assert_eq!(
            props.created,
            SharedString::from("Sunday, November 30, 2025, 9:00:00 AM")
        );
        assert_eq!(
            props.accessed,
            SharedString::from("Monday, December 1, 2025, 2:08:28 PM")
        );
        assert_eq!(props.path, SharedString::from(r"C:\Users\me\notes.txt"));
        assert_eq!(props.location, SharedString::from(r"C:\Users\me"));
        assert_eq!(props.opens_with, SharedString::from("Notepad"));
        assert_eq!(props.readonly, Some(false));
        assert_eq!(props.hidden, Some(false));
        assert!(!props.attrs_note);
        assert_eq!(props.attr_orig, Some(0));
        assert!(props.details.is_empty());
    }

    #[test]
    fn props_volume_branch_shows_free_of_total_and_em_dash_modified() {
        // A real Volume, with values derived exactly as the volume branch of
        // `show_properties` derives them.
        let volume = crate::volumes::Volume {
            name: "Windows-SSD (C:)".into(),
            path: PathBuf::from(r"C:\"),
            kind: crate::volumes::VolumeKind::Drive,
            free: 1024,
            total: 2048,
        };
        let props = props_from_fields(PropsFields {
            name: volume.name.clone().into(),
            kind: "Local Drive".into(),
            size: format!(
                "{} free of {}",
                crate::listing::format_size(volume.free),
                crate::listing::format_size(volume.total)
            )
            .into(),
            size_detail: "".into(),
            size_on_disk: "".into(),
            size_on_disk_detail: "".into(),
            contains: "".into(),
            modified: "—".into(),
            created: "—".into(),
            accessed: "—".into(),
            path: r"C:\".into(),
            location: r"C:\".into(),
            // Volumes carry no Opens-with target; the overlay hides the row.
            opens_with: String::new(),
            readonly: None,
            hidden: None,
            attrs_note: false,
            attr_orig: None,
        });
        assert_eq!(props.name, SharedString::from("Windows-SSD (C:)"));
        assert_eq!(props.size, SharedString::from("1.0 KB free of 2.0 KB"));
        assert_eq!(props.modified, SharedString::from("—"));
        assert!(props.opens_with.is_empty());
        assert!(props.contains.is_empty());
    }

    #[test]
    fn props_listing_branch_uses_em_dash_size_for_dirs() {
        // A real directory Entry, with size derived exactly as the listing
        // branch of `show_properties` derives it.
        let entry = Entry {
            path: PathBuf::from(r"C:\pics"),
            name: "pics".into(),
            kind: EntryKind::Directory,
            size: 4096,
            modified: None,
            hidden: false,
        };
        let size: String = if entry.is_directory() {
            "—".into()
        } else {
            crate::listing::format_size(entry.size)
        };
        let props = props_from_fields(PropsFields {
            name: entry.name.clone().into(),
            kind: crate::listing::kind_label(&entry).into(),
            size: size.into(),
            size_detail: "Calculating…".into(),
            size_on_disk: "Calculating…".into(),
            size_on_disk_detail: "".into(),
            contains: "Calculating…".into(),
            modified: crate::listing::format_full_datetime(entry.modified, chrono::Local::now())
                .into(),
            created: "—".into(),
            accessed: "—".into(),
            path: r"C:\pics".into(),
            location: r"C:\".into(),
            // Directories carry no Opens-with target; the overlay hides it.
            opens_with: String::new(),
            readonly: None,
            hidden: Some(false),
            attrs_note: true,
            attr_orig: Some(0),
        });
        assert_eq!(props.kind, SharedString::from("Folder"));
        assert_eq!(props.size, SharedString::from("—"));
        assert_eq!(props.modified, SharedString::from("—"));
        assert!(props.opens_with.is_empty());
        assert!(props.readonly.is_none());
        assert!(props.attrs_note);
    }

    #[test]
    fn props_fallback_branch_maps_kind_size_and_modified() {
        // Filesystem-fallback derivations: kind from the bare file name,
        // size from the byte length, mtime formatted, and em dashes when
        // there is no metadata at all.
        assert_eq!(
            crate::listing::kind_label_for_name("notes.txt"),
            "Text Document"
        );
        let props = props_from_fields(PropsFields {
            name: "notes.txt".into(),
            kind: crate::listing::kind_label_for_name("notes.txt").into(),
            size: crate::listing::format_size(2048).into(),
            size_detail: format_byte_detail(2048).into(),
            size_on_disk: "4.0 KB".into(),
            size_on_disk_detail: "(4,096 bytes)".into(),
            contains: "".into(),
            modified: crate::listing::format_full_datetime(None, chrono::Local::now()).into(),
            created: crate::listing::format_full_datetime(None, chrono::Local::now()).into(),
            accessed: crate::listing::format_full_datetime(None, chrono::Local::now()).into(),
            path: r"C:\Users\me\notes.txt".into(),
            location: r"C:\Users\me".into(),
            opens_with: "Text Document".to_string(),
            readonly: Some(false),
            hidden: Some(false),
            attrs_note: false,
            attr_orig: Some(0),
        });
        assert_eq!(props.kind, SharedString::from("Text Document"));
        assert_eq!(props.size, SharedString::from("2.0 KB"));
        assert_eq!(props.size_detail, SharedString::from("(2,048 bytes)"));
        assert_eq!(props.modified, SharedString::from("—"));
        let missing = props_from_fields(PropsFields {
            name: "gone".into(),
            kind: "—".into(),
            size: "—".into(),
            size_detail: "—".into(),
            size_on_disk: "—".into(),
            size_on_disk_detail: "".into(),
            contains: "".into(),
            modified: "—".into(),
            created: "—".into(),
            accessed: "—".into(),
            path: r"C:\gone".into(),
            location: r"C:\".into(),
            opens_with: "—".to_string(),
            readonly: None,
            hidden: None,
            attrs_note: false,
            attr_orig: None,
        });
        assert_eq!(missing.kind, SharedString::from("—"));
        assert_eq!(missing.size, SharedString::from("—"));
        assert_eq!(missing.modified, SharedString::from("—"));
        assert!(
            missing.size_on_disk_detail.is_empty(),
            "unknown on-disk size carries no detail"
        );
    }

    #[test]
    fn details_filter_drops_exactly_the_eight_first_class_labels() {
        let rows: Vec<(String, String)> = [
            ("Type", "Text Document"),
            ("Size", "1.0 KB"),
            ("Size on disk", "4.0 KB"),
            ("Created", "Monday, December 1, 2025, 2:08:28 PM"),
            ("Modified", "Monday, December 1, 2025, 2:08:28 PM"),
            ("Accessed", "Monday, December 1, 2025, 2:08:28 PM"),
            ("Attributes", "Archive"),
            ("Contains", "3 Files, 1 Folder"),
            ("Author", "me"),
            ("Title", "notes"),
            ("Dimensions", "800 x 600"),
            ("Length", "3:12"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let kept = filtered_details(rows);
        let labels: Vec<&str> = kept.iter().map(|(k, _)| k.as_ref()).collect();
        assert_eq!(labels, ["Author", "Title", "Dimensions", "Length"]);
    }

    #[test]
    fn opens_with_is_empty_for_dirs_and_volumes() {
        assert!(opens_with_display(Path::new(r"C:\pics"), "Folder", true).is_empty());
        // Files still resolve through the shell with a kind fallback.
        assert_eq!(
            opens_with_display(Path::new(r"\\MTP\DEVICE\o1"), "Text Document", false),
            "Text Document"
        );
    }

    #[test]
    fn attrs_dirty_is_true_only_when_bits_differ() {
        use crate::fs_ops::{ATTR_HIDDEN, ATTR_READONLY};
        // Untouched boxes, known or unknown opening bits: clean.
        assert!(!attrs_dirty(Some(0), Some(false), Some(false)));
        assert!(!attrs_dirty(Some(ATTR_READONLY), Some(true), Some(false)));
        assert!(!attrs_dirty(Some(ATTR_READONLY), None, None));
        assert!(!attrs_dirty(None, None, None));
        // Any flipped box: dirty.
        assert!(attrs_dirty(Some(0), Some(true), None));
        assert!(attrs_dirty(Some(ATTR_READONLY), Some(false), None));
        assert!(attrs_dirty(Some(0), None, Some(true)));
        assert!(attrs_dirty(Some(ATTR_HIDDEN), None, Some(false)));
        // Unknown opening bits with a concrete box: dirty.
        assert!(attrs_dirty(None, Some(true), None));
    }

    #[test]
    fn full_date_format_contains_the_year_once() {
        use chrono::TimeZone;
        let evening = chrono::Local
            .with_ymd_and_hms(2025, 12, 1, 14, 8, 28)
            .unwrap();
        let evening_sys: SystemTime = chrono::DateTime::<chrono::Utc>::from(evening).into();
        let text = crate::listing::format_full_datetime(Some(evening_sys), evening);
        assert_eq!(text.matches("2025").count(), 1);
        assert!(text.contains("December"));
        assert_eq!(
            crate::listing::format_full_datetime(None, chrono::Local::now()),
            "—"
        );
    }

    #[test]
    fn byte_detail_groups_thousands() {
        assert_eq!(format_byte_detail(580_833_358), "(580,833,358 bytes)");
        assert_eq!(format_byte_detail(2048), "(2,048 bytes)");
        assert_eq!(format_byte_detail(999), "(999 bytes)");
        assert_eq!(format_byte_detail(0), "(0 bytes)");
    }

    #[test]
    fn contains_counts_files_and_folders() {
        assert_eq!(format_contains(12, 3), "12 Files, 3 Folders");
        assert_eq!(format_contains(1, 1), "1 File, 1 Folder");
        assert_eq!(format_contains(0, 0), "0 Files, 0 Folders");
    }

    #[test]
    fn properties_and_delete_rows_are_glyph_only() {
        // No shell stock source on either builder; the MDL2 codepoint plus
        // the lucide fallback carry the row.
        for rows in [
            build_item_menu(&file_spec()),
            build_item_menu(&dir_spec()),
            build_empty_menu(
                ViewMode::List,
                SortKey::default(),
                true,
                PathBuf::from(r"C:\pics"),
            ),
        ] {
            let props = find_item(&rows, "Properties");
            assert!(
                props.shell.is_none(),
                "Properties must not use a stock icon"
            );
            assert_eq!(props.glyph, Some('\u{E946}'));
        }
        let file_rows = build_item_menu(&file_spec());
        let delete = find_item(&file_rows, "Delete");
        assert!(delete.shell.is_none(), "Delete must not use a stock icon");
        assert_eq!(delete.glyph, Some('\u{E74D}'));
        assert!(delete.danger);
        assert_eq!(delete.shortcut, Some(SharedString::from("Del")));
    }

    #[test]
    fn copy_row_renamed_with_shortcut_unchanged() {
        let rows = build_item_menu(&file_spec());
        let copy = find_item(&rows, "Copy as path");
        assert_eq!(copy.shortcut, Some(SharedString::from("Ctrl+Shift+C")));
        assert_eq!(copy.glyph, Some('\u{E8C8}'));
        assert!(
            rows.iter().all(|row| match row {
                MenuRow::Item(item) => item.label.as_ref() != "Copy path",
                MenuRow::Separator => true,
            }),
            "old Copy path label must be gone from the item menu"
        );
    }
    #[test]
    fn terminal_row_covers_files_at_their_parent() {
        // Files share the row; `fs_ops::open_terminal` falls back to the
        // parent dir for file paths, so the action keeps the file path.
        let file_rows = build_item_menu(&file_spec());
        let term = find_item(&file_rows, "Open in Terminal");
        assert!(
            matches!(term.action, Some(MenuAction::OpenInTerminal(_))),
            "files must offer Open in Terminal"
        );
        assert_eq!(term.glyph, Some('\u{E756}'));
        // Still a single-target row: multi-select hides it like before.
        let mut multi_spec = file_spec();
        multi_spec.multi = true;
        multi_spec.targets = vec![PathBuf::from(r"C:\a.txt"), PathBuf::from(r"C:\b.txt")];
        assert!(
            build_item_menu(&multi_spec).iter().all(|row| match row {
                MenuRow::Item(item) => item.label.as_ref() != "Open in Terminal",
                MenuRow::Separator => true,
            }),
            "multi-select must not offer Open in Terminal"
        );
        // Browse-only bin items keep no Terminal row, as before the files
        // expansion: every bin entry is a file, so without this gate the
        // expansion would add the row there.
        let mut bin_spec = file_spec();
        bin_spec.browse_only = true;
        assert!(
            build_item_menu(&bin_spec).iter().all(|row| match row {
                MenuRow::Item(item) => item.label.as_ref() != "Open in Terminal",
                MenuRow::Separator => true,
            }),
            "Recycle Bin items must not offer Open in Terminal"
        );
    }

    #[test]
    fn open_with_has_no_writable_gate_at_build() {
        // `open_menu` passes `Some` for every file now; the builder renders
        // the rows regardless of `writable`.
        let mut read_only = file_spec();
        read_only.writable = false;
        let rows = build_item_menu(&read_only);
        find_item(&rows, "Open with…");
        // `None` (not a file) still offers nothing Open-with.
        let dir_rows = build_item_menu(&dir_spec());
        assert!(
            dir_rows.iter().all(|row| match row {
                MenuRow::Item(item) => !item.label.as_ref().starts_with("Open with"),
                MenuRow::Separator => true,
            }),
            "directories must not offer Open-with"
        );
    }

    #[test]
    fn rename_event_action_commits_only_on_enter() {
        assert_eq!(
            rename_event_action(&InputEvent::PressEnter {
                secondary: false,
                shift: false
            }),
            Some(RenameEventAction::Commit)
        );
        assert_eq!(
            rename_event_action(&InputEvent::Blur),
            Some(RenameEventAction::Cancel)
        );
        assert_eq!(rename_event_action(&InputEvent::Change), None);
        assert_eq!(rename_event_action(&InputEvent::Focus), None);
    }

    #[test]
    fn rename_select_range_keeps_the_extension() {
        assert_eq!(rename_select_range("notes.txt"), 0..5);
        assert_eq!(rename_select_range("archive.tar.gz"), 0..11);
        assert_eq!(rename_select_range("Makefile"), 0.."Makefile".len());
        assert_eq!(rename_select_range(".gitignore"), 0..".gitignore".len());
        assert_eq!(rename_select_range(""), 0..0);
        // Unicode stem: the dot is ASCII, so the split stays a boundary.
        assert_eq!(rename_select_range("café.txt"), 0.."café".len());
    }

    #[test]
    fn size_on_disk_strings_pairs_value_and_detail() {
        let (value, detail) = size_on_disk_strings(Some(118_784));
        assert_eq!(
            value,
            SharedString::from(crate::listing::format_size(118_784))
        );
        assert_eq!(detail, SharedString::from("(118,784 bytes)"));
        let (missing, missing_detail) = size_on_disk_strings(None);
        assert_eq!(missing, SharedString::from("—"));
        assert!(missing_detail.is_empty());
    }

    fn sidebar_spec(kind: SidebarKind, pinned: bool) -> SidebarMenuSpec {
        let path = match kind {
            SidebarKind::Folder => PathBuf::from(r"C:\Users\me\pics"),
            SidebarKind::Volume => PathBuf::from(r"C:\"),
            SidebarKind::Device => PathBuf::from(r"\\MTP\deadbeef"),
        };
        SidebarMenuSpec { path, kind, pinned }
    }

    #[test]
    fn sidebar_folder_rows_pin_terminal_copy_reveal_delete() {
        let rows = build_sidebar_menu(&sidebar_spec(SidebarKind::Folder, false));
        assert_eq!(
            row_labels(&rows),
            [
                "Open",
                "<sep>",
                "Open in Terminal",
                "Add to Quick Access",
                "Copy as path",
                "Reveal in Explorer",
                "<sep>",
                "Properties",
                "Delete",
            ]
        );
        let pinned = build_sidebar_menu(&sidebar_spec(SidebarKind::Folder, true));
        let remove = find_item(&pinned, "Remove from Quick Access");
        assert!(matches!(remove.action, Some(MenuAction::Unpin(_))));
        assert!(
            pinned.iter().all(|row| match row {
                MenuRow::Item(item) => item.label.as_ref() != "Add to Quick Access",
                MenuRow::Separator => true,
            }),
            "pinned folders offer Remove, not Add"
        );
    }

    #[test]
    fn sidebar_volume_rows_drop_pin_and_delete() {
        let rows = build_sidebar_menu(&sidebar_spec(SidebarKind::Volume, false));
        assert_eq!(
            row_labels(&rows),
            [
                "Open",
                "<sep>",
                "Open in Terminal",
                "Copy path",
                "Reveal in Explorer",
                "<sep>",
                "Properties",
            ]
        );
    }

    #[test]
    fn sidebar_device_rows_keep_only_open_copy_properties() {
        let rows = build_sidebar_menu(&sidebar_spec(SidebarKind::Device, false));
        assert_eq!(
            row_labels(&rows),
            ["Open", "Copy path", "<sep>", "Properties",]
        );
    }

    #[test]
    fn sidebar_glyphs_mirror_the_item_menu() {
        let folder = build_sidebar_menu(&sidebar_spec(SidebarKind::Folder, false));
        let glyph = |rows: &Vec<MenuRow>, label| find_item(rows, label).glyph;
        assert_eq!(glyph(&folder, "Open"), Some('\u{E8E5}'));
        assert_eq!(glyph(&folder, "Open in Terminal"), Some('\u{E756}'));
        assert_eq!(glyph(&folder, "Copy as path"), Some('\u{E8C8}'));
        assert_eq!(glyph(&folder, "Reveal in Explorer"), Some('\u{E838}'));
        assert_eq!(glyph(&folder, "Properties"), Some('\u{E946}'));
        assert_eq!(glyph(&folder, "Delete"), Some('\u{E74D}'));
        let volume = build_sidebar_menu(&sidebar_spec(SidebarKind::Volume, false));
        assert_eq!(glyph(&volume, "Copy path"), Some('\u{E8C8}'));
    }

    #[test]
    fn sidebar_delete_is_a_single_danger_target() {
        let rows = build_sidebar_menu(&sidebar_spec(SidebarKind::Folder, false));
        let delete = find_item(&rows, "Delete");
        assert!(delete.danger);
        assert!(delete.shell.is_none(), "Delete stays glyph-only");
        assert_eq!(delete.shortcut, Some(SharedString::from("Del")));
        let MenuAction::Delete(targets) = delete.action.clone().unwrap() else {
            panic!("Delete must carry its target");
        };
        assert_eq!(targets, vec![PathBuf::from(r"C:\Users\me\pics")]);
    }

    #[test]
    fn sidebar_classify_splits_volumes_devices_folders_and_bin() {
        use crate::volumes::{Volume, VolumeKind};
        let volumes = vec![
            Volume {
                name: "Windows-SSD (C:)".into(),
                path: PathBuf::from(r"C:\"),
                kind: VolumeKind::Drive,
                free: 1,
                total: 2,
            },
            Volume {
                name: "Phone".into(),
                path: PathBuf::from(r"\\MTP\deadbeef"),
                kind: VolumeKind::Device,
                free: 0,
                total: 0,
            },
        ];
        let quick_access = vec![PathBuf::from(r"C:\Users\me\pics")];
        // Recycle Bin attaches no menu.
        assert!(
            classify_sidebar(
                Path::new(crate::recycle_bin::ROOT_STR),
                &volumes,
                &quick_access
            )
            .is_none()
        );
        let drive = classify_sidebar(Path::new(r"C:\"), &volumes, &quick_access).unwrap();
        assert_eq!(drive.kind, SidebarKind::Volume);
        assert!(!drive.pinned);
        let mtp = classify_sidebar(Path::new(r"\\MTP\deadbeef"), &volumes, &quick_access).unwrap();
        assert_eq!(mtp.kind, SidebarKind::Device);
        let folder =
            classify_sidebar(Path::new(r"C:\Users\me\pics"), &volumes, &quick_access).unwrap();
        assert_eq!(folder.kind, SidebarKind::Folder);
        assert!(folder.pinned);
        let sub =
            classify_sidebar(Path::new(r"C:\Users\me\other"), &volumes, &quick_access).unwrap();
        assert_eq!(sub.kind, SidebarKind::Folder);
        assert!(!sub.pinned);
    }
}
