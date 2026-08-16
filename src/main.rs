mod chrome;
mod listing;
mod sidebar;
mod theme;
mod volumes;
mod watch;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// #region agent log
/// Append one NDJSON debug line to `/tmp/ply-debug.log` (hypothesis-driven nav debugging).
pub(crate) fn agent_debug_log(hypothesis_id: &str, location: &str, message: &str, data: &str) {
    use std::io::Write;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/ply-debug.log")
    {
        let _ = writeln!(
            f,
            r#"{{"hypothesisId":"{hypothesis_id}","location":"{location}","message":"{message}","data":{data},"timestamp":{ts}}}"#
        );
    }
}
// #endregion

use gpui::{
    AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement, Modifiers,
    ParentElement, Render, SharedString, Styled, Task, Window, WindowOptions, actions, px,
};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::{ActiveTheme, Root, TitleBar, v_flex};

use listing::{Entry, EntryKind, Snapshot, list_dir, parent_in_workspace, sort_snapshot};
use sidebar::SidebarTree;
use volumes::{Volume, default_quick_access, discover_volumes};
use watch::FolderWatch;

actions!(
    ply,
    [
        Refresh,
        OpenFolder,
        ToggleHidden,
        GoHome,
        GoBack,
        GoForward,
        CopyPath,
        Reveal,
        OpenSelection,
        ShowProperties,
        CloseProperties,
        ToggleTheme,
        ToggleViewList,
        ToggleViewGrid,
    ]
);

/// How the Current Folder listing is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewMode {
    List,
    Grid,
}

pub(crate) enum LoadState<T> {
    Idle,
    Loading,
    Ready(T),
    Failed { message: SharedString },
}

pub(crate) struct Ply {
    /// Home shows drives and quick access instead of a Current Folder listing.
    pub(crate) at_home: bool,
    /// Root the current browse session is confined to (a volume or a picked folder).
    pub(crate) workspace: PathBuf,
    pub(crate) current_folder: PathBuf,
    history_back: Vec<PathBuf>,
    history_forward: Vec<PathBuf>,
    pub(crate) view_mode: ViewMode,
    pub(crate) volumes: Vec<Volume>,
    pub(crate) quick_access: Vec<PathBuf>,
    pub(crate) listing: LoadState<Snapshot>,
    pub(crate) selected: Vec<PathBuf>,
    anchor_ix: Option<usize>,
    pub(crate) filter: Entity<InputState>,
    pub(crate) filter_text: String,
    pub(crate) properties: Option<Entry>,
    pub(crate) tree: SidebarTree,
    pub(crate) show_hidden: bool,
    pub(crate) banner: Option<SharedString>,
    pub(crate) sort_key: &'static str,
    pub(crate) sort_ascending: bool,
    last_listed_folder: Option<PathBuf>,
    last_fingerprint: Vec<listing::EntryFingerprint>,
    list_generation: u64,
    list_task: Option<Task<()>>,
    watch: Option<FolderWatch>,
    pub(crate) focus: FocusHandle,
}

impl Ply {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.filter_text = this.filter.read(cx).value().to_string();
                this.selected.clear();
                this.anchor_ix = None;
                cx.notify();
            }
        })
        .detach();

        let focus = cx.focus_handle();
        focus.focus(window, cx);

        let mut ply = Self {
            at_home: true,
            workspace: home.clone(),
            current_folder: home,
            history_back: Vec::new(),
            history_forward: Vec::new(),
            view_mode: ViewMode::List,
            volumes: Vec::new(),
            quick_access: Vec::new(),
            listing: LoadState::Idle,
            selected: Vec::new(),
            anchor_ix: None,
            filter,
            filter_text: String::new(),
            properties: None,
            tree: SidebarTree::default(),
            show_hidden: false,
            banner: None,
            sort_key: "name",
            sort_ascending: true,
            last_listed_folder: None,
            last_fingerprint: Vec::new(),
            list_generation: 0,
            list_task: None,
            watch: None,
            focus,
        };
        ply.refresh_volumes(cx);
        ply.start_watch_poll(cx);
        ply
    }

    /// Re-read mounts and quick access folders off the UI thread (Home is live).
    pub(crate) fn refresh_volumes(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let discovered = cx
                .background_spawn(async { (discover_volumes(), default_quick_access()) })
                .await;
            this.update(cx, |this, cx| {
                let (volumes, quick_access) = discovered;
                this.volumes = volumes;
                this.quick_access = quick_access;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn start_watch_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                this.update(cx, |this, cx| {
                    if this
                        .watch
                        .as_ref()
                        .is_some_and(|w| w.take_change_debounced(Duration::from_millis(75)))
                    {
                        this.reload_listing(cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn arm_watch(&mut self, cx: &mut Context<Self>) {
        match FolderWatch::current_folder(self.current_folder.clone()) {
            Ok(w) => self.watch = Some(w),
            Err(err) => {
                self.watch = None;
                self.banner = Some(format!("Watch unavailable: {err}").into());
                cx.notify();
            }
        }
    }

    // -- Navigation ---------------------------------------------------------

    /// Leave browsing and show drives + quick access again.
    pub(crate) fn go_home(&mut self, cx: &mut Context<Self>) {
        // #region agent log
        agent_debug_log(
            "A",
            "main.rs:go_home",
            "go_home called",
            &format!(
                r#"{{"at_home_before":{},"workspace":"{}","current_folder":"{}"}}"#,
                self.at_home,
                self.workspace.display(),
                self.current_folder.display()
            ),
        );
        // #endregion
        self.at_home = true;
        self.watch = None;
        self.list_task = None;
        self.listing = LoadState::Idle;
        self.last_listed_folder = None;
        self.last_fingerprint.clear();
        self.history_back.clear();
        self.history_forward.clear();
        self.selected.clear();
        self.anchor_ix = None;
        self.properties = None;
        self.banner = None;
        self.refresh_volumes(cx);
        cx.notify();
    }

    /// The volume (or picked folder) that should bound browsing for `path`.
    fn workspace_for(&self, path: &Path) -> PathBuf {
        self.volumes
            .iter()
            .map(|volume| volume.path.as_path())
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.as_os_str().len())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    }

    /// Open `path` as a fresh browse session rooted at `root`.
    pub(crate) fn enter_root(&mut self, root: PathBuf, path: PathBuf, cx: &mut Context<Self>) {
        // #region agent log
        let path_is_dir = path.is_dir();
        agent_debug_log(
            "A",
            "main.rs:enter_root",
            "enter_root entry",
            &format!(
                r#"{{"at_home_before":{},"root":"{}","path":"{}","path_is_dir":{},"workspace_before":"{}","current_before":"{}"}}"#,
                self.at_home,
                root.display(),
                path.display(),
                path_is_dir,
                self.workspace.display(),
                self.current_folder.display()
            ),
        );
        // #endregion
        self.at_home = false;
        self.workspace = root;
        self.current_folder = path;
        self.history_back.clear();
        self.history_forward.clear();
        self.selected.clear();
        self.anchor_ix = None;
        self.properties = None;
        self.banner = None;
        self.reveal_in_sidebar(cx);
        self.reload_listing(cx);
        self.arm_watch(cx);
        // #region agent log
        agent_debug_log(
            "C",
            "main.rs:enter_root",
            "enter_root after state update",
            &format!(
                r#"{{"at_home":{},"workspace":"{}","current_folder":"{}"}}"#,
                self.at_home,
                self.workspace.display(),
                self.current_folder.display()
            ),
        );
        // #endregion
    }

    /// Set the Current Folder, entering a new Workspace when `path` is outside this one.
    pub(crate) fn navigate_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // #region agent log
        agent_debug_log(
            "A",
            "main.rs:navigate_to",
            "navigate_to entry",
            &format!(
                r#"{{"path":"{}","path_is_dir":{},"at_home":{},"workspace":"{}","current_folder":"{}"}}"#,
                path.display(),
                path.is_dir(),
                self.at_home,
                self.workspace.display(),
                self.current_folder.display()
            ),
        );
        // #endregion
        if !path.is_dir() {
            // #region agent log
            agent_debug_log(
                "A",
                "main.rs:navigate_to",
                "early return: not a dir",
                &format!(r#"{{"path":"{}"}}"#, path.display()),
            );
            // #endregion
            self.banner = Some(format!("Not a folder: {}", path.display()).into());
            cx.notify();
            return;
        }
        if !self.at_home && path.starts_with(&self.workspace) {
            if path == self.current_folder {
                // #region agent log
                agent_debug_log(
                    "A",
                    "main.rs:navigate_to",
                    "early return: same folder",
                    &format!(r#"{{"path":"{}"}}"#, path.display()),
                );
                // #endregion
                return;
            }
            // #region agent log
            agent_debug_log(
                "A",
                "main.rs:navigate_to",
                "branch: in-workspace navigate",
                &format!(r#"{{"path":"{}"}}"#, path.display()),
            );
            // #endregion
            self.history_back.push(self.current_folder.clone());
            self.history_forward.clear();
            self.current_folder = path;
            self.selected.clear();
            self.anchor_ix = None;
            self.properties = None;
            self.reveal_in_sidebar(cx);
            self.reload_listing(cx);
            self.arm_watch(cx);
            return;
        }
        let root = self.workspace_for(&path);
        // #region agent log
        agent_debug_log(
            "A",
            "main.rs:navigate_to",
            "branch: enter_root via workspace_for",
            &format!(
                r#"{{"path":"{}","root":"{}","at_home":{}}}"#,
                path.display(),
                root.display(),
                self.at_home
            ),
        );
        // #endregion
        self.enter_root(root, path, cx);
    }

    /// Back through history, then up to the parent, then out to Home.
    pub(crate) fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.at_home {
            return;
        }
        if let Some(previous) = self.history_back.pop() {
            self.history_forward.push(self.current_folder.clone());
            self.current_folder = previous;
            self.selected.clear();
            self.anchor_ix = None;
            self.properties = None;
            self.reveal_in_sidebar(cx);
            self.reload_listing(cx);
            self.arm_watch(cx);
            return;
        }
        match parent_in_workspace(&self.current_folder, &self.workspace) {
            Some(parent) => {
                self.current_folder = parent;
                self.selected.clear();
                self.anchor_ix = None;
                self.properties = None;
                self.reveal_in_sidebar(cx);
                self.reload_listing(cx);
                self.arm_watch(cx);
            }
            None => self.go_home(cx),
        }
    }

    pub(crate) fn go_forward(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.history_forward.pop() else {
            return;
        };
        self.history_back.push(self.current_folder.clone());
        self.current_folder = next;
        self.selected.clear();
        self.anchor_ix = None;
        self.properties = None;
        self.reveal_in_sidebar(cx);
        self.reload_listing(cx);
        self.arm_watch(cx);
    }

    pub(crate) fn can_go_back(&self) -> bool {
        !self.at_home
    }

    pub(crate) fn can_go_forward(&self) -> bool {
        !self.history_forward.is_empty()
    }

    // -- Sidebar tree -------------------------------------------------------

    fn reveal_in_sidebar(&mut self, cx: &mut Context<Self>) {
        let root = self.workspace.clone();
        let current = self.current_folder.clone();
        self.tree.reveal(&root, &current);
        self.load_pending_sidebar_children(cx);
    }

    pub(crate) fn toggle_sidebar_row(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.tree.toggle(&path);
        self.load_pending_sidebar_children(cx);
        cx.notify();
    }

    pub(crate) fn expand_sidebar_row(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.tree.set_expanded(&path, true);
        self.load_pending_sidebar_children(cx);
    }

    /// List children for every open row that has none yet.
    fn load_pending_sidebar_children(&mut self, cx: &mut Context<Self>) {
        for path in self.tree.expanded_paths() {
            if self.tree.needs_children(&path) {
                // Claim the row so a second pass does not spawn a duplicate list.
                self.tree.set_children(&path, Vec::new());
                self.spawn_sidebar_children(path, cx);
            }
        }
    }

    fn spawn_sidebar_children(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let show_hidden = self.show_hidden;
        let folder = path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { sidebar::list_child_folders(&folder, show_hidden) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(children) => this.tree.set_children(&path, children),
                    Err(err) => {
                        this.tree.set_expanded(&path, false);
                        this.banner = Some(format!("Could not list folder: {err}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // -- Listing ------------------------------------------------------------

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.tree.forget_children();
        self.load_pending_sidebar_children(cx);
        if self.at_home {
            self.refresh_volumes(cx);
        } else {
            self.reload_listing(cx);
        }
    }

    pub(crate) fn reload_listing(&mut self, cx: &mut Context<Self>) {
        // #region agent log
        agent_debug_log(
            "C",
            "main.rs:reload_listing",
            "reload_listing start",
            &format!(
                r#"{{"at_home":{},"current_folder":"{}","list_generation_next":{},"runId":"post-fix"}}"#,
                self.at_home,
                self.current_folder.display(),
                self.list_generation + 1
            ),
        );
        // #endregion
        self.list_generation += 1;
        let generation = self.list_generation;
        // Keep prior Ready entries visible while refreshing the *same* folder so a
        // no-op watch reload does not flash Loading / require a notify when unchanged.
        let keep_showing = matches!(self.listing, LoadState::Ready(_))
            && self.last_listed_folder.as_ref() == Some(&self.current_folder);
        if !keep_showing {
            self.listing = LoadState::Loading;
            cx.notify();
        }
        let folder = self.current_folder.clone();
        let show_hidden = self.show_hidden;
        self.list_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { list_dir(&folder, show_hidden) })
                .await;
            this.update(cx, |this, cx| {
                if this.list_generation != generation {
                    // #region agent log
                    agent_debug_log(
                        "C",
                        "main.rs:reload_listing",
                        "stale generation discarded",
                        &format!(
                            r#"{{"generation":{},"current_generation":{},"at_home":{}}}"#,
                            generation, this.list_generation, this.at_home
                        ),
                    );
                    // #endregion
                    return;
                }
                match result {
                    Ok(snapshot) => {
                        let listed = this.current_folder.clone();
                        let unchanged = this.last_listed_folder.as_ref() == Some(&listed)
                            && snapshot.fingerprint == this.last_fingerprint;
                        this.last_listed_folder = Some(listed);
                        this.last_fingerprint = snapshot.fingerprint.clone();
                        let entry_count = snapshot.entries.len();
                        this.listing = LoadState::Ready(this.sorted(snapshot));
                        if !unchanged {
                            this.banner = None;
                        }
                        // Drop notify noise from our own readdir so the watch poll
                        // cannot schedule another reload from Access/atime events.
                        if let Some(watch) = this.watch.as_ref() {
                            watch.acknowledge();
                        }
                        // #region agent log
                        agent_debug_log(
                            "C",
                            "main.rs:reload_listing",
                            "listing ready",
                            &format!(
                                r#"{{"at_home":{},"current_folder":"{}","entry_count":{},"unchanged":{},"runId":"post-fix"}}"#,
                                this.at_home,
                                this.current_folder.display(),
                                entry_count,
                                unchanged
                            ),
                        );
                        // #endregion
                        // ADR 0002: skip notify when Snapshot fingerprint is unchanged.
                        if !unchanged {
                            cx.notify();
                        }
                    }
                    Err(err) => {
                        let message: SharedString = err.to_string().into();
                        // #region agent log
                        agent_debug_log(
                            "A",
                            "main.rs:reload_listing",
                            "listing failed",
                            &format!(
                                r#"{{"at_home":{},"error":"{}"}}"#,
                                this.at_home,
                                message.replace('"', "'")
                            ),
                        );
                        // #endregion
                        this.banner = Some(format!("Could not list folder: {message}").into());
                        this.listing = LoadState::Failed { message };
                        if let Some(watch) = this.watch.as_ref() {
                            watch.acknowledge();
                        }
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
    }

    /// Sort by the active column, keeping folders above files like Explorer does.
    fn sorted(&self, snapshot: Snapshot) -> Snapshot {
        let sorted = sort_snapshot(snapshot, self.sort_key, self.sort_ascending);
        let (mut entries, files): (Vec<Entry>, Vec<Entry>) = sorted
            .entries
            .into_iter()
            .partition(|entry| entry.is_directory());
        entries.extend(files);
        let fingerprint = entries.iter().map(Entry::fingerprint).collect();
        Snapshot {
            entries,
            fingerprint,
        }
    }

    /// Sort by `key`, flipping direction when it is already the sort column.
    pub(crate) fn sort_by(&mut self, key: &'static str, cx: &mut Context<Self>) {
        if self.sort_key == key {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort_key = key;
            self.sort_ascending = true;
        }
        if let LoadState::Ready(snapshot) = std::mem::replace(&mut self.listing, LoadState::Loading)
        {
            self.listing = LoadState::Ready(self.sorted(snapshot));
        }
        cx.notify();
    }

    pub(crate) fn toggle_hidden(&mut self, cx: &mut Context<Self>) {
        self.show_hidden = !self.show_hidden;
        self.tree.forget_children();
        self.load_pending_sidebar_children(cx);
        if !self.at_home {
            self.reload_listing(cx);
        }
        cx.notify();
    }

    pub(crate) fn set_view_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        self.view_mode = mode;
        cx.notify();
    }

    /// Entries of the Current Folder after the filter, in sort order.
    pub(crate) fn visible_entries(&self) -> Vec<&Entry> {
        let LoadState::Ready(snapshot) = &self.listing else {
            return Vec::new();
        };
        let needle = self.filter_text.trim().to_lowercase();
        snapshot
            .entries
            .iter()
            .filter(|entry| needle.is_empty() || entry.name.to_lowercase().contains(&needle))
            .collect()
    }

    pub(crate) fn total_entries(&self) -> usize {
        match &self.listing {
            LoadState::Ready(snapshot) => snapshot.entries.len(),
            _ => 0,
        }
    }

    // -- Selection ----------------------------------------------------------

    pub(crate) fn is_selected(&self, path: &Path) -> bool {
        self.selected.iter().any(|p| p == path)
    }

    /// Click selection: plain replaces, Ctrl/Cmd toggles, Shift extends from the anchor.
    pub(crate) fn select_at(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self
            .visible_entries()
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        let Some(path) = paths.get(ix).cloned() else {
            return;
        };
        if modifiers.shift {
            let anchor = self.anchor_ix.unwrap_or(ix);
            let (lo, hi) = if anchor <= ix {
                (anchor, ix)
            } else {
                (ix, anchor)
            };
            self.selected = paths[lo..=hi.min(paths.len() - 1)].to_vec();
        } else if modifiers.control || modifiers.platform {
            if let Some(pos) = self.selected.iter().position(|p| *p == path) {
                self.selected.remove(pos);
            } else {
                self.selected.push(path);
            }
            self.anchor_ix = Some(ix);
        } else {
            self.selected = vec![path];
            self.anchor_ix = Some(ix);
        }
        cx.notify();
    }

    /// Entries behind `selected`, in listing order.
    pub(crate) fn selected_entries(&self) -> Vec<Entry> {
        let LoadState::Ready(snapshot) = &self.listing else {
            return Vec::new();
        };
        snapshot
            .entries
            .iter()
            .filter(|entry| self.is_selected(&entry.path))
            .cloned()
            .collect()
    }

    fn primary_selection(&self) -> Option<Entry> {
        let last = self.selected.last()?;
        self.selected_entries()
            .into_iter()
            .find(|entry| &entry.path == last)
            .or_else(|| self.selected_entries().into_iter().next())
    }

    // -- Entry commands -----------------------------------------------------

    pub(crate) fn open_entry(&mut self, entry: Entry, cx: &mut Context<Self>) {
        match entry.kind {
            EntryKind::Directory => self.navigate_to(entry.path, cx),
            EntryKind::Symlink { .. } if entry.path.is_dir() => self.navigate_to(entry.path, cx),
            EntryKind::File | EntryKind::Symlink { .. } => {
                if let Err(err) = open::that(&entry.path) {
                    self.banner = Some(format!("Open failed: {err}").into());
                    cx.notify();
                }
            }
        }
    }

    pub(crate) fn open_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.primary_selection() {
            self.open_entry(entry, cx);
        }
    }

    pub(crate) fn reveal_selection(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.primary_selection() else {
            return;
        };
        #[cfg(windows)]
        let result = std::process::Command::new("explorer")
            .arg(format!("/select,{}", entry.path.display()))
            .spawn()
            .map(|_| ());
        #[cfg(not(windows))]
        let result = open::that(entry.path.parent().unwrap_or(&entry.path));
        match result {
            Ok(()) => self.banner = Some("Revealed in the system file manager.".into()),
            Err(err) => self.banner = Some(format!("Reveal failed: {err}").into()),
        }
        cx.notify();
    }

    pub(crate) fn copy_path(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected_entries();
        if selected.is_empty() {
            return;
        }
        let joined = selected
            .iter()
            .map(|entry| entry.path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(joined));
        self.banner = Some(
            if selected.len() == 1 {
                "Path copied.".to_string()
            } else {
                format!("{} paths copied.", selected.len())
            }
            .into(),
        );
        cx.notify();
    }

    pub(crate) fn show_properties(&mut self, entry: Option<Entry>, cx: &mut Context<Self>) {
        self.properties = entry.or_else(|| self.primary_selection());
        cx.notify();
    }

    pub(crate) fn close_properties(&mut self, cx: &mut Context<Self>) {
        self.properties = None;
        cx.notify();
    }

    pub(crate) fn dismiss_banner(&mut self, cx: &mut Context<Self>) {
        self.banner = None;
        cx.notify();
    }

    /// Pick any folder and browse it as its own Workspace root.
    pub(crate) fn pick_workspace(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_spawn(async { rfd::FileDialog::new().pick_folder() })
                .await;
            this.update(cx, |this, cx| {
                if let Some(path) = picked {
                    this.enter_root(path.clone(), path, cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Render for Ply {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .relative()
            .key_context("Ply")
            .track_focus(&self.focus)
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.refresh(cx)))
            .on_action(cx.listener(|this, _: &OpenFolder, _, cx| this.pick_workspace(cx)))
            .on_action(cx.listener(|this, _: &ToggleHidden, _, cx| this.toggle_hidden(cx)))
            .on_action(cx.listener(|this, _: &GoHome, _, cx| this.go_home(cx)))
            .on_action(cx.listener(|this, _: &GoBack, _, cx| this.go_back(cx)))
            .on_action(cx.listener(|this, _: &GoForward, _, cx| this.go_forward(cx)))
            .on_action(cx.listener(|this, _: &CopyPath, _, cx| this.copy_path(cx)))
            .on_action(cx.listener(|this, _: &Reveal, _, cx| this.reveal_selection(cx)))
            .on_action(cx.listener(|this, _: &OpenSelection, _, cx| this.open_selection(cx)))
            .on_action(
                cx.listener(|this, _: &ShowProperties, _, cx| this.show_properties(None, cx)),
            )
            .on_action(cx.listener(|this, _: &CloseProperties, _, cx| this.close_properties(cx)))
            .on_action(cx.listener(|_, _: &ToggleTheme, window, cx| theme::toggle(window, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleViewList, _, cx| {
                    this.set_view_mode(ViewMode::List, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &ToggleViewGrid, _, cx| {
                    this.set_view_mode(ViewMode::Grid, cx)
                }),
            )
            .child(self.render_chrome(window, cx))
    }
}

fn main() {
    let app = gpui_platform::application().with_assets(gpui_component_assets::Assets);
    app.run(move |cx| {
        gpui_component::init(cx);
        theme::apply(cx);

        cx.bind_keys([
            gpui::KeyBinding::new("f5", Refresh, Some("Ply")),
            gpui::KeyBinding::new("ctrl-o", OpenFolder, Some("Ply")),
            gpui::KeyBinding::new("ctrl-h", ToggleHidden, Some("Ply && !PlyFilter")),
            gpui::KeyBinding::new("d", ToggleTheme, Some("Ply && !PlyFilter")),
            gpui::KeyBinding::new("alt-left", GoBack, Some("Ply")),
            gpui::KeyBinding::new("alt-up", GoBack, Some("Ply")),
            gpui::KeyBinding::new("backspace", GoBack, Some("Ply && !PlyFilter")),
            gpui::KeyBinding::new("alt-right", GoForward, Some("Ply")),
            gpui::KeyBinding::new("alt-home", GoHome, Some("Ply")),
            gpui::KeyBinding::new("ctrl-c", CopyPath, Some("Ply && !PlyFilter")),
            gpui::KeyBinding::new("enter", OpenSelection, Some("Ply && !PlyFilter")),
            gpui::KeyBinding::new("alt-enter", ShowProperties, Some("Ply && !PlyFilter")),
            gpui::KeyBinding::new("escape", CloseProperties, Some("Ply && !PlyFilter")),
            gpui::KeyBinding::new("ctrl-1", ToggleViewList, Some("Ply")),
            gpui::KeyBinding::new("ctrl-2", ToggleViewGrid, Some("Ply")),
        ]);

        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds {
                        origin: gpui::point(px(80.), px(80.)),
                        size: gpui::size(px(1280.), px(800.)),
                    })),
                    app_id: Some("app.ply.explorer".into()),
                    window_decorations: Some(gpui::WindowDecorations::Client),
                    ..TitleBar::window_options()
                },
                |window, cx| {
                    let view = cx.new(|cx| Ply::new(window, cx));
                    cx.new(|cx| Root::new(view, window, cx).bg(cx.theme().background))
                },
            )
            .expect("window");
        })
        .detach();
    });
}
