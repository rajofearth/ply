use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Context, Pixels, Point, SharedString, Window, prelude::*};
use gpui_component::input::{InputEvent, InputState};

use crate::fs_ops;
use crate::icons::Ico;
use crate::listing::{Entry, Snapshot, SortKey, list_sorted};
use crate::volumes;

use super::{
    ConfirmAction, ConfirmDialog, LoadState, Menu, MenuAction, MenuItem, MenuRow, Ply, Properties,
    Rename, ToolBtn, ViewMode,
};

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
            let result = cx
                .background_spawn(async move { list_sorted(&folder, key) })
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
                            && current.same_contents(&snapshot)
                        {
                            return;
                        }
                        this.remember_names(&snapshot);
                        this.listing = LoadState::Ready(snapshot);
                        this.rebuild_visible();
                    }
                    Err(err) => {
                        this.listing = LoadState::Failed(err.to_string().into());
                        this.visible_indices.clear();
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
                            .into_iter()
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
                let _ = this.update(cx, |_this, cx| {
                    crate::thumbs::refresh_lnk(&lnks, cx)
                });
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

    /// Rebuild [`Ply::visible_indices`] from the Ready listing and filter.
    pub(super) fn rebuild_visible(&mut self) {
        self.visible_indices.clear();
        let LoadState::Ready(snapshot) = &self.listing else {
            return;
        };
        if self.filter_text.is_empty() {
            self.visible_indices.extend(0..snapshot.entries.len());
            return;
        }
        let needle = self.filter_text.to_lowercase();
        self.visible_indices.extend(
            snapshot
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.name.to_lowercase().contains(&needle))
                .map(|(i, _)| i),
        );
    }

    /// Entries in the current folder that survive the filter box.
    pub fn visible(&self) -> Vec<&Entry> {
        let LoadState::Ready(snapshot) = &self.listing else {
            return Vec::new();
        };
        self.visible_indices
            .iter()
            .filter_map(|&i| snapshot.entries.get(i))
            .collect()
    }

    /// Count of filtered entries without allocating the entry slice.
    pub fn visible_len(&self) -> usize {
        match &self.listing {
            LoadState::Ready(_) => self.visible_indices.len(),
            _ => 0,
        }
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
        let pinned = self.quick_access.contains(&path);
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

        let mut toolbar = Vec::new();
        if !is_volume && !browse_only {
            toolbar.push(btn(Ico::Scissors, MenuAction::Cut, false, false));
            toolbar.push(btn(Ico::Copy, MenuAction::Copy, false, false));
            if !multi && caps.rename {
                toolbar.push(btn(
                    Ico::Pencil,
                    MenuAction::Rename(path.clone()),
                    true,
                    false,
                ));
            }
            if caps.trash {
                toolbar.push(btn(
                    Ico::Trash,
                    MenuAction::Delete(targets.clone()),
                    true,
                    true,
                ));
            }
        }

        let mut rows = vec![row(
            "Open",
            Ico::ExternalLink,
            MenuAction::Open(path.clone()),
        )];
        if is_file && writable {
            rows.push(flyout(
                "Open with",
                Ico::ExternalLink,
                vec![
                    MenuItem::new(
                        "Choose another app…",
                        None,
                        Some(MenuAction::ChooseApp(path.clone())),
                    )
                    .into(),
                ],
            ));
        }
        if admin {
            rows.push(row(
                "Run as administrator",
                Ico::Shield,
                MenuAction::RunAsAdmin(path.clone()),
            ));
        }
        if !multi && is_dir && writable {
            rows.push(row(
                "Open in Terminal",
                Ico::Terminal,
                MenuAction::OpenInTerminal(path.clone()),
            ));
        }
        rows.push(MenuRow::Separator);
        if is_dir && !is_volume {
            rows.push(if pinned {
                row(
                    "Remove from Quick Access",
                    Ico::PinOff,
                    MenuAction::Unpin(path.clone()),
                )
            } else {
                row(
                    "Add to Quick Access",
                    Ico::Pin,
                    MenuAction::Pin(path.clone()),
                )
            });
            rows.push(MenuRow::Separator);
        }
        rows.push(row(
            "Copy path",
            Ico::Copy,
            MenuAction::CopyPath(path.clone()),
        ));
        if caps.reveal {
            rows.push(row(
                "Reveal in Explorer",
                Ico::Folder,
                MenuAction::Reveal(path.clone()),
            ));
        }
        rows.push(row(
            "Properties",
            Ico::Info,
            MenuAction::Properties(path.clone()),
        ));
        if !is_volume && !browse_only && caps.trash {
            let label = if targets.len() > 1 {
                format!("Delete {}", targets.len())
            } else {
                "Delete".into()
            };
            rows.push(MenuRow::Separator);
            rows.push(
                MenuItem::new(label, Some(Ico::Trash), Some(MenuAction::Delete(targets)))
                    .danger()
                    .into(),
            );
        }

        self.show_menu(at, toolbar, rows, cx);
    }

    pub fn open_empty_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(folder) = self.current_folder().map(Path::to_path_buf) else {
            return;
        };
        let writable = crate::path_caps::for_path(&folder).rename;
        let view = self.view;
        let sort = self.sort;

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
            ),
            MenuItem {
                children: vec![row("Folder", Ico::FolderPlus, MenuAction::NewFolder)],
                ..MenuItem::new("New", Some(Ico::FolderPlus), None).on(writable)
            }
            .into(),
            MenuRow::Separator,
            MenuItem::new("Paste", Some(Ico::ClipboardPaste), Some(MenuAction::Paste))
                .off()
                .into(),
            MenuRow::Separator,
        ];
        if writable {
            rows.push(row(
                "Open in Terminal",
                Ico::Terminal,
                MenuAction::OpenInTerminal(folder.clone()),
            ));
        }
        rows.push(row("Refresh", Ico::Refresh, MenuAction::Refresh));
        rows.push(row("Properties", Ico::Info, MenuAction::Properties(folder)));
        self.show_menu(at, Vec::new(), rows, cx);
    }

    fn show_menu(
        &mut self,
        at: Point<Pixels>,
        toolbar: Vec<ToolBtn>,
        rows: Vec<MenuRow>,
        cx: &mut Context<Self>,
    ) {
        self.menu = Some(Menu {
            at,
            toolbar,
            rows,
            flyout: None,
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
            MenuAction::RunAsAdmin(path) => {
                self.try_fs(fs_ops::run_as_admin(&path), "Could not elevate", cx)
            }
            MenuAction::OpenInTerminal(path) => {
                self.try_fs(fs_ops::open_terminal(&path), "Terminal failed", cx)
            }
            MenuAction::Pin(path) => self.pin(path, cx),
            MenuAction::Unpin(path) => self.unpin(&path, cx),
            MenuAction::CopyPath(path) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                    path.to_string_lossy().into_owned(),
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

        if let Some(volume) = self.volumes.iter().find(|v| v.path == path) {
            let kind = match volume.kind {
                volumes::VolumeKind::Drive => "Local Drive",
                volumes::VolumeKind::Device => "Removable Device",
                volumes::VolumeKind::Network => "Network Drive",
            };
            self.set_props(
                volume.name.clone(),
                kind,
                format!(
                    "{} free of {}",
                    crate::listing::format_size(volume.free),
                    crate::listing::format_size(volume.total)
                ),
                "—",
                path_display,
                cx,
            );
            return;
        }

        if let Some(entry) = self.listing_entry(path) {
            let size = if entry.is_directory() {
                "—".into()
            } else {
                crate::listing::format_size(entry.size)
            };
            self.set_props(
                entry.name.clone(),
                crate::listing::kind_label(entry),
                size,
                crate::listing::format_mtime(entry.modified, chrono::Local::now()),
                path_display,
                cx,
            );
            return;
        }

        let meta = if crate::path_caps::is_portable(path) {
            None
        } else {
            std::fs::metadata(path).ok()
        };
        let now = chrono::Local::now();
        let name = self.display_name(path);
        let (kind, size, modified): (String, String, String) = match &meta {
            Some(m) if m.is_dir() => (
                "Folder".into(),
                "—".into(),
                crate::listing::format_mtime(m.modified().ok(), now),
            ),
            Some(m) => {
                let file_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| name.to_string());
                (
                    crate::listing::kind_label_for_name(&file_name).into(),
                    crate::listing::format_size(m.len()),
                    crate::listing::format_mtime(m.modified().ok(), now),
                )
            }
            None => ("—".into(), "—".into(), "—".into()),
        };
        self.set_props(name, kind, size, modified, path_display, cx);
    }

    fn set_props(
        &mut self,
        name: impl Into<SharedString>,
        kind: impl Into<SharedString>,
        size: impl Into<SharedString>,
        modified: impl Into<SharedString>,
        path: SharedString,
        cx: &mut Context<Self>,
    ) {
        self.properties = Some(Properties {
            name: name.into(),
            kind: kind.into(),
            size: size.into(),
            modified: modified.into(),
            path,
        });
        cx.notify();
    }

    pub fn close_properties(&mut self, cx: &mut Context<Self>) {
        if self.properties.take().is_some() {
            cx.notify();
        }
    }
}

fn btn(icon: Ico, action: MenuAction, enabled: bool, danger: bool) -> ToolBtn {
    ToolBtn {
        icon,
        action,
        enabled,
        danger,
    }
}

fn row(label: impl Into<SharedString>, icon: Ico, action: MenuAction) -> MenuRow {
    MenuItem::new(label, Some(icon), Some(action)).into()
}

fn flyout(label: impl Into<SharedString>, icon: Ico, children: Vec<MenuRow>) -> MenuRow {
    MenuItem {
        children,
        ..MenuItem::new(label, Some(icon), None)
    }
    .into()
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
