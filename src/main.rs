mod chrome;
mod folder_table;
mod listing;
mod preview;
mod theme;
mod tree_pane;
mod watch;

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Styled, Task, Window, WindowOptions, actions, px,
};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::label::Label;
use gpui_component::list::ListItem;
use gpui_component::table::{TableEvent, TableState};
use gpui_component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui_component::{ActiveTheme, Icon, IconName, Root, Sizable, TitleBar, h_flex, v_flex};

use folder_table::FolderDelegate;
use listing::{Entry, EntryKind, Snapshot, list_dir, parent_in_workspace};
use preview::{Preview, build_preview};
use watch::FolderWatch;

actions!(
    ply,
    [
        Refresh,
        OpenFolder,
        ToggleHidden,
        GoToParent,
        CopyPath,
        Reveal,
        OpenSelection
    ]
);

pub(crate) enum LoadState<T> {
    Idle,
    Loading,
    Ready(T),
    Failed { message: SharedString },
}

pub(crate) struct Ply {
    pub(crate) workspace: PathBuf,
    pub(crate) current_folder: PathBuf,
    pub(crate) listing: LoadState<Snapshot>,
    last_listed_folder: Option<PathBuf>,
    last_fingerprint: Vec<listing::EntryFingerprint>,
    list_generation: u64,
    preview_generation: u64,
    list_task: Option<Task<()>>,
    preview_task: Option<Task<()>>,
    pub(crate) preview: Preview,
    pub(crate) selected: Option<Entry>,
    pub(crate) show_hidden: bool,
    pub(crate) banner: Option<SharedString>,
    pub(crate) table: Entity<TableState<FolderDelegate>>,
    pub(crate) tree: Entity<TreeState>,
    tree_roots: Vec<TreeItem>,
    pub(crate) filter: Entity<InputState>,
    watch: Option<FolderWatch>,
    focus: FocusHandle,
}

impl Ply {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspace = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let current_folder = workspace.clone();
        let table = cx.new(|cx| TableState::new(FolderDelegate::new(), window, cx));
        let tree_roots = vec![tree_pane::workspace_item(&workspace)];
        let tree = cx.new(|cx| TreeState::new(cx).items(tree_roots.clone()));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter this folder…"));
        cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                let q = this.filter.read(cx).value().to_string();
                this.table.update(cx, |table, cx| {
                    table.delegate_mut().set_filter(q);
                    cx.notify();
                });
            }
        })
        .detach();
        cx.subscribe(&table, |this, table, event: &TableEvent, cx| {
            if let TableEvent::SelectRow(ix) | TableEvent::DoubleClickedRow(ix) = event {
                let selected = table.read(cx).delegate().entry(*ix).cloned();
                if let Some(entry) = selected {
                    this.select_entry(entry, cx);
                }
                if matches!(event, TableEvent::DoubleClickedRow(_)) {
                    this.open_selected(cx);
                }
            }
        })
        .detach();
        cx.subscribe(&tree, |this, _, event: &TreeEvent, cx| {
            if let TreeEvent::Expanded(id) = event {
                if let Some(path) = tree_pane::path_from_tree_id(id.as_ref()) {
                    this.ensure_tree_children(path, cx);
                }
            }
        })
        .detach();

        let mut ply = Self {
            workspace: workspace.clone(),
            current_folder,
            listing: LoadState::Idle,
            last_listed_folder: None,
            last_fingerprint: Vec::new(),
            list_generation: 0,
            preview_generation: 0,
            list_task: None,
            preview_task: None,
            preview: Preview::None,
            selected: None,
            show_hidden: false,
            banner: None,
            table,
            tree,
            tree_roots,
            filter,
            watch: None,
            focus: cx.focus_handle(),
        };
        ply.ensure_tree_children(workspace, cx);
        ply.reload_listing(cx);
        ply.arm_watch(cx);
        ply.start_watch_poll(cx);
        ply
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

    fn set_workspace(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.workspace = path.clone();
        self.current_folder = path.clone();
        self.selected = None;
        self.preview = Preview::None;
        self.tree_roots = vec![tree_pane::workspace_item(&path)];
        let roots = self.tree_roots.clone();
        self.tree.update(cx, |tree, cx| {
            tree.set_items(roots, cx);
        });
        self.ensure_tree_children(path, cx);
        self.reload_listing(cx);
        self.arm_watch(cx);
    }

    pub(crate) fn set_current_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !path.starts_with(&self.workspace) {
            self.banner = Some("That folder is outside the Workspace.".into());
            cx.notify();
            return;
        }
        if path == self.current_folder {
            return;
        }
        self.current_folder = path;
        self.selected = None;
        self.preview = Preview::None;
        self.reload_listing(cx);
        self.arm_watch(cx);
    }

    fn ensure_tree_children(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !path.starts_with(&self.workspace) {
            return;
        }
        let id = tree_pane::path_id(&path);
        if !tree_pane::item_needs_load(&self.tree_roots, &id) {
            return;
        }
        let show_hidden = self.show_hidden;
        let folder = path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { tree_pane::list_folder_children(&folder, show_hidden) })
                .await;
            this.update(cx, |this, cx| {
                let id = tree_pane::path_id(&path);
                if !tree_pane::item_needs_load(&this.tree_roots, &id) {
                    return;
                }
                match result {
                    Ok(snapshot) => {
                        let children = tree_pane::items_from_dir_snapshot(&snapshot);
                        if tree_pane::set_children(&mut this.tree_roots, &id, children) {
                            let roots = this.tree_roots.clone();
                            this.tree.update(cx, |tree, cx| {
                                tree.set_items(roots, cx);
                            });
                        }
                    }
                    Err(err) => {
                        this.banner = Some(format!("Could not list folder: {err}").into());
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn reload_listing(&mut self, cx: &mut Context<Self>) {
        self.list_generation += 1;
        let generation = self.list_generation;
        self.listing = LoadState::Loading;
        self.table.update(cx, |table, cx| {
            table.delegate_mut().set_loading(true);
            cx.notify();
        });
        let folder = self.current_folder.clone();
        let show_hidden = self.show_hidden;
        self.list_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { list_dir(&folder, show_hidden) })
                .await;
            this.update(cx, |this, cx| {
                if this.list_generation != generation {
                    return;
                }
                match result {
                    Ok(snapshot) => {
                        let listed = this.current_folder.clone();
                        if this.last_listed_folder.as_ref() == Some(&listed)
                            && snapshot.fingerprint == this.last_fingerprint
                        {
                            this.listing = LoadState::Ready(snapshot);
                            this.table.update(cx, |table, cx| {
                                table.delegate_mut().set_loading(false);
                                cx.notify();
                            });
                            return;
                        }
                        this.last_listed_folder = Some(listed);
                        this.last_fingerprint = snapshot.fingerprint.clone();
                        this.listing = LoadState::Ready(snapshot.clone());
                        this.table.update(cx, |table, cx| {
                            table.delegate_mut().set_snapshot(snapshot);
                            cx.notify();
                        });
                        this.banner = None;
                        cx.notify();
                    }
                    Err(err) => {
                        let message: SharedString = err.to_string().into();
                        this.banner = Some(format!("Could not list folder: {message}").into());
                        this.listing = LoadState::Failed { message };
                        this.table.update(cx, |table, cx| {
                            table.delegate_mut().set_loading(false);
                            cx.notify();
                        });
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
    }

    fn select_entry(&mut self, entry: Entry, cx: &mut Context<Self>) {
        self.selected = Some(entry.clone());
        self.load_preview(entry, cx);
    }

    fn load_preview(&mut self, entry: Entry, cx: &mut Context<Self>) {
        self.preview_generation += 1;
        let generation = self.preview_generation;
        self.preview = Preview::Loading;
        cx.notify();
        self.preview_task = Some(cx.spawn(async move |this, cx| {
            let preview = cx
                .background_spawn(async move { build_preview(entry) })
                .await;
            this.update(cx, |this, cx| {
                if this.preview_generation != generation {
                    return;
                }
                this.preview = preview;
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.selected.clone() else {
            return;
        };
        match entry.kind {
            EntryKind::Directory => self.set_current_folder(entry.path, cx),
            EntryKind::File | EntryKind::Symlink { .. } => {
                if let Err(err) = open::that(&entry.path) {
                    self.banner = Some(format!("Open failed: {err}").into());
                    cx.notify();
                }
            }
        }
    }

    fn reveal_selected(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.selected.clone() else {
            return;
        };
        #[cfg(windows)]
        let result = std::process::Command::new("explorer")
            .arg(format!("/select,{}", entry.path.display()))
            .spawn()
            .map(|_| ());
        #[cfg(not(windows))]
        let result = open::that(&entry.path);
        if let Err(err) = result {
            self.banner = Some(format!("Reveal failed: {err}").into());
            cx.notify();
        }
    }

    fn copy_path(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = &self.selected else {
            return;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            entry.path.to_string_lossy().into_owned(),
        ));
        self.banner = Some("Path copied.".into());
        cx.notify();
    }

    pub(crate) fn pick_workspace(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_spawn(async { rfd::FileDialog::new().pick_folder() })
                .await;
            this.update(cx, |this, cx| {
                if let Some(path) = picked {
                    this.set_workspace(path, cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Render for Ply {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.reload_listing(cx)))
            .on_action(cx.listener(|this, _: &OpenFolder, _, cx| this.pick_workspace(cx)))
            .on_action(cx.listener(|this, _: &ToggleHidden, _, cx| {
                this.show_hidden = !this.show_hidden;
                this.reload_listing(cx);
            }))
            .on_action(cx.listener(|this, _: &GoToParent, window, cx| {
                if this.typing_in_filter(window, cx) {
                    return;
                }
                if let Some(parent) = parent_in_workspace(&this.current_folder, &this.workspace) {
                    this.set_current_folder(parent, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CopyPath, window, cx| {
                if this.typing_in_filter(window, cx) {
                    return;
                }
                this.copy_path(cx);
            }))
            .on_action(cx.listener(|this, _: &Reveal, _, cx| this.reveal_selected(cx)))
            .on_action(cx.listener(|this, _: &OpenSelection, window, cx| {
                if this.typing_in_filter(window, cx) {
                    return;
                }
                this.open_selected(cx);
            }))
            .child(self.render_chrome(cx))
    }
}

impl Ply {
    pub(crate) fn render_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let current_id = tree_pane::path_id(&self.current_folder);
        tree(&self.tree, move |ix, entry, _selected, _window, cx| {
            let id = entry.item().id.clone();
            let label = entry.item().label.clone();
            let is_current = id.as_ref() == current_id;
            let expanded = entry.is_expanded();
            let chevron = if expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };
            let folder = if expanded {
                IconName::FolderOpen
            } else {
                IconName::Folder
            };
            ListItem::new(ix)
                .selected(is_current)
                .pl(px(8.) + px(16.) * entry.depth() as f32)
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Icon::new(chevron)
                                .small()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            Icon::new(folder).small().text_color(if is_current {
                                cx.theme().primary
                            } else {
                                cx.theme().muted_foreground
                            }),
                        )
                        .child(Label::new(label)),
                )
                .on_click({
                    let view = view.clone();
                    move |_, _, cx| {
                        view.update(cx, |this, cx| {
                            let Some(path) = tree_pane::path_from_tree_id(id.as_ref()) else {
                                return;
                            };
                            this.set_current_folder(path.clone(), cx);
                            this.ensure_tree_children(path, cx);
                        });
                    }
                })
        })
    }
}

fn main() {
    let app = gpui_platform::application().with_assets(gpui_component_assets::Assets);
    app.run(move |cx| {
        gpui_component::init(cx);
        theme::apply(cx);

        cx.bind_keys([
            gpui::KeyBinding::new("f5", Refresh, None),
            gpui::KeyBinding::new("ctrl-o", OpenFolder, None),
            gpui::KeyBinding::new("ctrl-h", ToggleHidden, None),
            gpui::KeyBinding::new("alt-up", GoToParent, None),
            gpui::KeyBinding::new("backspace", GoToParent, Some("PlyList")),
            gpui::KeyBinding::new("ctrl-c", CopyPath, Some("PlyList")),
            gpui::KeyBinding::new("enter", OpenSelection, Some("PlyList")),
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
