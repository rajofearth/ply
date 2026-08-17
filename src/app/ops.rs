use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Context, Pixels, Point, SharedString, Window, prelude::*};
use gpui_component::input::{InputEvent, InputState};

use crate::fs_ops;
use crate::listing::{Entry, Snapshot, list_dir};
use crate::volumes;

use super::{LoadState, Menu, MenuAction, MenuItem, Properties, Ply, Rename};

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

    /// Notice drives appearing and disappearing. Windows delivers this as
    /// `WM_DEVICECHANGE`, which GPUI does not surface, so poll instead.
    /// Lettered discover runs only when `GetLogicalDrives` changes; MTP refreshes
    /// on a slower cadence so an expensive WPD scan never rides every tick.
    pub(super) fn start_volume_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut last_mask = volumes::logical_drives_mask();
            let mut ticks_since_mtp: u32 = 0;
            const MTP_EVERY_TICKS: u32 = 7; // ~10.5s at 1.5s/tick
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;

                let mask = volumes::logical_drives_mask();
                if mask != last_mask {
                    // Sequential: an unreachable network share can stall lettered
                    // discovery; awaiting keeps lettered polls from stacking.
                    let lettered =
                        cx.background_spawn(async { volumes::discover_lettered() }).await;
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
            let (lo, hi) = if anchor <= ix { (anchor, ix) } else { (ix, anchor) };
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

    // ---- menu, properties, file operations --------------------------------

    pub fn open_menu(&mut self, at: Point<Pixels>, path: PathBuf, cx: &mut Context<Self>) {
        if !self.is_selected(&path) {
            self.replace_selection(vec![path.clone()]);
        }
        let pinned = self.quick_access.contains(&path);
        let is_volume = self.volumes.iter().any(|v| v.path == path);
        let caps = crate::path_caps::for_path(&path);
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
        if !is_volume && caps.rename {
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
        if caps.reveal {
            items.push(MenuItem {
                label: "Reveal in Explorer".into(),
                action: MenuAction::Reveal(path.clone()),
                danger: false,
            });
        }
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
        if !is_volume && caps.trash {
            let label = if targets.len() > 1 {
                format!("Delete {}", targets.len()).into()
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

    pub fn delete(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let count = paths.len();
        match fs_ops::delete_to_trash(&paths) {
            Ok(()) => {
                self.clear_selection_paths();
                self.anchor = None;
                let note = if count == 1 {
                    "Moved to the Recycle Bin.".to_string()
                } else {
                    format!("Moved {count} to the Recycle Bin.")
                };
                self.note(note, cx);
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
        let path_display: SharedString = path.to_string_lossy().into_owned().into();

        if let Some(volume) = self.volumes.iter().find(|v| v.path == path) {
            let kind = match volume.kind {
                volumes::VolumeKind::Drive => "Local Drive",
                volumes::VolumeKind::Device => "Removable Device",
                volumes::VolumeKind::Network => "Network Drive",
            };
            self.properties = Some(Properties {
                name: volume.name.clone().into(),
                kind: kind.into(),
                size: format!(
                    "{} free of {}",
                    crate::listing::format_size(volume.free),
                    crate::listing::format_size(volume.total)
                )
                .into(),
                modified: "—".into(),
                path: path_display,
            });
            cx.notify();
            return;
        }

        if let LoadState::Ready(snapshot) = &self.listing
            && let Some(entry) = snapshot.entries.iter().find(|e| e.path == path)
        {
            let size = if entry.is_directory() {
                "—".into()
            } else {
                crate::listing::format_size(entry.size).into()
            };
            self.properties = Some(Properties {
                name: entry.name.clone().into(),
                kind: crate::listing::kind_label(entry).into(),
                size,
                modified: crate::listing::format_mtime(entry.modified, chrono::Local::now()).into(),
                path: path_display,
            });
            cx.notify();
            return;
        }

        // No listing row: metadata when the OS can answer; portable paths
        // without a listing stay honest dashes rather than a stub Entry.
        let meta = if crate::path_caps::is_portable(path) {
            None
        } else {
            std::fs::metadata(path).ok()
        };

        let now = chrono::Local::now();
        let name = self.display_name(path);
        let (kind, size, modified) = match &meta {
            Some(m) if m.is_dir() => (
                SharedString::from("Folder"),
                SharedString::from("—"),
                SharedString::from(crate::listing::format_mtime(m.modified().ok(), now)),
            ),
            Some(m) => {
                let file_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| name.to_string());
                (
                    SharedString::from(crate::listing::kind_label_for_name(&file_name)),
                    SharedString::from(crate::listing::format_size(m.len())),
                    SharedString::from(crate::listing::format_mtime(m.modified().ok(), now)),
                )
            }
            None => (
                SharedString::from("—"),
                SharedString::from("—"),
                SharedString::from("—"),
            ),
        };

        self.properties = Some(Properties {
            name,
            kind,
            size,
            modified,
            path: path_display,
        });
        cx.notify();
    }

    pub fn close_properties(&mut self, cx: &mut Context<Self>) {
        if self.properties.take().is_some() {
            cx.notify();
        }
    }
}
