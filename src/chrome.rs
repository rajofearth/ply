//! All Ply rendering: title bar, sidebar, Home, folder list/grid, status bar, Properties.

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, ClickEvent, Context, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px, relative,
};
use gpui_component::alert::Alert;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::menu::{ContextMenuExt, PopupMenu};
use gpui_component::scroll::ScrollableElement;
use gpui_component::spinner::Spinner;
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Selectable, Sizable, TitleBar, h_flex, v_flex,
};

use crate::listing::{Entry, EntryKind, entry_kind_label, format_mtime, format_size};
use crate::sidebar::{TreeRow, folder_label, path_id};
use crate::volumes::{Volume, VolumeKind};
use crate::{
    CopyPath, GoBack, GoForward, GoHome, LoadState, OpenFolder, OpenSelection, Ply, Refresh,
    Reveal, ShowProperties, ToggleHidden, ToggleTheme, ViewMode,
};

const SIDEBAR_WIDTH: f32 = 220.;
const TITLE_BAR_HEIGHT: f32 = 38.;
const STATUS_BAR_HEIGHT: f32 = 26.;
const ROW_HEIGHT: f32 = 29.;
const CELL_WIDTH: f32 = 96.;
const KIND_WIDTH: f32 = 130.;
const SIZE_WIDTH: f32 = 80.;
const MODIFIED_WIDTH: f32 = 110.;
const UI_TEXT: f32 = 12.5;

/// What a sidebar row opens when clicked.
#[derive(Clone)]
enum RowTarget {
    Home,
    /// A volume or picked folder: becomes the Workspace root.
    Root(PathBuf),
    /// A folder inside the Workspace (or under some volume).
    Folder(PathBuf),
}

impl Ply {
    pub(crate) fn render_chrome(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(self.title_bar(cx))
            .when_some(self.banner.clone(), |this, message| {
                this.child(
                    div()
                        .id("banner")
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_banner(cx)))
                        .child(Alert::warning("banner", message).banner()),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .items_start()
                    .child(self.sidebar(cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .bg(crate::theme::pane_bg(cx))
                            .child({
                                if self.at_home {
                                    self.home_view(cx).into_any_element()
                                } else {
                                    self.folder_view(cx).into_any_element()
                                }
                            })
                            .when(!self.at_home, |this| this.child(self.status_bar(cx))),
                    ),
            )
            .when_some(self.properties.clone(), |this, entry| {
                this.child(self.properties_modal(entry, cx))
            })
    }

    // -- Title bar ----------------------------------------------------------

    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = cx.theme().is_dark();
        TitleBar::new()
            .h(px(TITLE_BAR_HEIGHT))
            .child(
                h_flex()
                    .w_full()
                    .h_full()
                    .items_center()
                    .gap(px(4.))
                    .pr(px(6.))
                    .text_size(px(UI_TEXT))
                    .child(
                        div()
                            .mr(px(6.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Ply"),
                    )
                    .child(
                        Button::new("back")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ArrowLeft)
                            .disabled(!self.can_go_back())
                            .tooltip_with_action("Back", &GoBack, None)
                            .on_click(cx.listener(|this, _, _, cx| this.go_back(cx))),
                    )
                    .child(
                        Button::new("forward")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ArrowRight)
                            .disabled(!self.can_go_forward())
                            .tooltip_with_action("Forward", &GoForward, None)
                            .on_click(cx.listener(|this, _, _, cx| this.go_forward(cx))),
                    )
                    .child(
                        Button::new("home")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Building2)
                            .selected(self.at_home)
                            .tooltip_with_action("Home", &GoHome, None)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.go_home_ui(window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .child(self.breadcrumbs(cx)),
                    )
                    .child(
                        Button::new("refresh")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Redo)
                            .tooltip_with_action("Refresh", &Refresh, None)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .child(
                        Button::new("open-folder")
                            .ghost()
                            .xsmall()
                            .icon(IconName::FolderOpen)
                            .tooltip_with_action("Open folder", &OpenFolder, None)
                            .on_click(cx.listener(|this, _, _, cx| this.pick_workspace(cx))),
                    )
                    .child(
                        Button::new("theme")
                            .ghost()
                            .xsmall()
                            .icon(if dark { IconName::Sun } else { IconName::Moon })
                            .tooltip_with_action("Toggle theme", &ToggleTheme, None)
                            .on_click(cx.listener(|_, _, window, cx| {
                                crate::theme::toggle(window, cx);
                            })),
                    ),
            )
            .bg(cx.theme().title_bar)
            .border_color(cx.theme().title_bar_border)
    }

    fn breadcrumbs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut crumbs: Vec<(SharedString, Option<PathBuf>)> = Vec::new();
        if self.at_home {
            crumbs.push(("Home".into(), None));
        } else {
            crumbs.push((SharedString::from("Home"), Some(PathBuf::new())));
            crumbs.push((self.workspace_label().into(), Some(self.workspace.clone())));
            if let Ok(rel) = self.current_folder.strip_prefix(&self.workspace) {
                let mut acc = self.workspace.clone();
                for part in rel.components() {
                    acc.push(part.as_os_str());
                    crumbs.push((
                        part.as_os_str().to_string_lossy().into_owned().into(),
                        Some(acc.clone()),
                    ));
                }
            }
        }

        let last = crumbs.len().saturating_sub(1);
        let mut row = h_flex().items_center().gap(px(2.)).flex_none();
        for (ix, (label, target)) in crumbs.into_iter().enumerate() {
            if ix > 0 {
                row = row.child(
                    Icon::new(IconName::ChevronRight)
                        .size(px(11.))
                        .text_color(cx.theme().muted_foreground),
                );
            }
            let is_last = ix == last;
            let mut crumb = div()
                .id(("crumb", ix))
                .px(px(5.))
                .py(px(2.))
                .truncate()
                .text_color(if is_last {
                    cx.theme().foreground
                } else {
                    cx.theme().muted_foreground
                })
                .child(label);
            if let Some(path) = target.filter(|_| !is_last) {
                let hover_bg = cx.theme().accent;
                crumb = crumb
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover_bg))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.focus.focus(window, cx);
                        if path.as_os_str().is_empty() {
                            this.go_home_ui(window, cx);
                        } else {
                            this.navigate_to(path.clone(), cx);
                        }
                    }));
            }
            row = row.child(crumb);
        }
        row
    }

    /// Display name for the Workspace root: the volume name when it is one.
    fn workspace_label(&self) -> String {
        self.volumes
            .iter()
            .find(|volume| volume.path == self.workspace)
            .map(|volume| volume.name.clone())
            .unwrap_or_else(|| folder_label(&self.workspace))
    }

    // -- Sidebar ------------------------------------------------------------

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let drives: Vec<&Volume> = self
            .volumes
            .iter()
            .filter(|volume| volume.kind == VolumeKind::Drive)
            .collect();
        let devices: Vec<&Volume> = self
            .volumes
            .iter()
            .filter(|volume| volume.kind != VolumeKind::Drive)
            .collect();

        let mut rows: Vec<AnyElement> = Vec::new();

        rows.push(section_label("Home", cx).into_any_element());
        rows.push(
            self.sidebar_row(
                "sidebar-home",
                "Home".into(),
                IconName::Building2,
                0,
                false,
                false,
                self.at_home,
                RowTarget::Home,
                cx,
            )
            .into_any_element(),
        );
        for path in &self.quick_access {
            rows.extend(self.sidebar_folder_rows(path.clone(), 1, cx));
        }

        rows.push(section_label("This PC", cx).into_any_element());
        for volume in &drives {
            rows.extend(self.sidebar_volume_rows(volume, cx));
        }

        if !devices.is_empty() {
            rows.push(section_label("Devices & network", cx).into_any_element());
            for volume in &devices {
                rows.extend(self.sidebar_volume_rows(volume, cx));
            }
        }

        v_flex()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .bg(crate::theme::sidebar_bg(cx))
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .py(px(4.))
                    .child(v_flex().w_full().children(rows))
                    .overflow_y_scrollbar(),
            )
    }

    /// A volume row plus its expanded folder descendants.
    fn sidebar_volume_rows(&self, volume: &Volume, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let icon = match volume.kind {
            VolumeKind::Network => IconName::Network,
            _ => IconName::HardDrive,
        };
        let expanded = self.tree.is_expanded(&volume.path);
        let mut rows = vec![
            self.sidebar_row(
                SharedString::from(format!("vol-{}", volume.id)),
                volume.name.clone(),
                icon,
                0,
                true,
                expanded,
                !self.at_home && self.current_folder == volume.path,
                RowTarget::Root(volume.path.clone()),
                cx,
            )
            .into_any_element(),
        ];
        for TreeRow { path, depth } in self.tree.rows_under(&volume.path, 1) {
            rows.push(self.sidebar_folder_row(path, depth, cx).into_any_element());
        }
        rows
    }

    /// A folder row plus its expanded descendants (used for quick access pins).
    fn sidebar_folder_rows(
        &self,
        path: PathBuf,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = vec![
            self.sidebar_folder_row(path.clone(), depth, cx)
                .into_any_element(),
        ];
        for row in self.tree.rows_under(&path, depth + 1) {
            rows.push(
                self.sidebar_folder_row(row.path, row.depth, cx)
                    .into_any_element(),
            );
        }
        rows
    }

    fn sidebar_folder_row(
        &self,
        path: PathBuf,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let expanded = self.tree.is_expanded(&path);
        let active = !self.at_home && self.current_folder == path;
        self.sidebar_row(
            SharedString::from(format!("dir-{}", path_id(&path))),
            folder_label(&path),
            if expanded {
                IconName::FolderOpen
            } else {
                IconName::Folder
            },
            depth,
            true,
            expanded,
            active,
            RowTarget::Folder(path),
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn sidebar_row(
        &self,
        key: impl Into<SharedString>,
        label: String,
        icon: IconName,
        depth: usize,
        expandable: bool,
        expanded: bool,
        active: bool,
        target: RowTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key: SharedString = key.into();
        let active_bg = crate::theme::select_strong(cx);
        let hover_bg = if active {
            active_bg
        } else {
            cx.theme().sidebar_accent
        };
        let muted = cx.theme().muted_foreground;
        let chevron_path = match &target {
            RowTarget::Root(path) | RowTarget::Folder(path) => Some(path.clone()),
            RowTarget::Home => None,
        };

        h_flex()
            .w_full()
            .py(px(5.))
            .pr(px(10.))
            .pl(px(10.) + px(16.) * depth as f32)
            .gap(px(4.))
            .items_center()
            .text_size(px(UI_TEXT))
            .when(active, |this| this.bg(active_bg))
            .hover(move |style| style.bg(hover_bg))
            .child(
                div()
                    .id(SharedString::from(format!("chevron-{key}")))
                    .size(px(14.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(expandable, |this| {
                        this.cursor_pointer()
                            .child(
                                Icon::new(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(11.))
                                .text_color(muted),
                            )
                            .when_some(chevron_path, |this, path| {
                                this.on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_sidebar_row(path.clone(), cx);
                                }))
                            })
                    }),
            )
            .child(
                h_flex()
                    .id(key)
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(6.))
                    .items_center()
                    .cursor_pointer()
                    .child(Icon::new(icon).size(px(14.)).text_color(muted))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(label))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.focus.focus(window, cx);
                        match target.clone() {
                            RowTarget::Home => this.go_home_ui(window, cx),
                            RowTarget::Root(path) => {
                                this.expand_sidebar_row(path.clone(), cx);
                                this.enter_root(path.clone(), path, cx);
                            }
                            RowTarget::Folder(path) => {
                                this.expand_sidebar_row(path.clone(), cx);
                                this.navigate_to(path, cx);
                            }
                        }
                    })),
            )
    }

    // -- Home ---------------------------------------------------------------

    fn home_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut sections = v_flex().w_full().p(px(16.)).gap(px(18.));

        let drives: Vec<&Volume> = self
            .volumes
            .iter()
            .filter(|v| v.kind == VolumeKind::Drive)
            .collect();
        let devices: Vec<&Volume> = self
            .volumes
            .iter()
            .filter(|v| matches!(v.kind, VolumeKind::Device | VolumeKind::Network))
            .collect();

        let mut drive_cards: Vec<AnyElement> = Vec::new();
        for volume in &drives {
            drive_cards.push(self.drive_card(volume, cx).into_any_element());
        }
        sections = sections.child(
            v_flex()
                .w_full()
                .gap(px(10.))
                .child(section_label("Drives", cx))
                .child(if drive_cards.is_empty() {
                    h_flex()
                        .gap(px(6.))
                        .items_center()
                        .text_color(cx.theme().muted_foreground)
                        .child(Spinner::new().small())
                        .child("Looking for drives")
                        .into_any_element()
                } else {
                    h_flex()
                        .w_full()
                        .flex_wrap()
                        .gap(px(10.))
                        .children(drive_cards)
                        .into_any_element()
                }),
        );

        sections = sections.child(
            v_flex()
                .w_full()
                .gap(px(10.))
                .child(section_label("Devices & network", cx))
                .child(if devices.is_empty() {
                    div()
                        .text_size(px(UI_TEXT))
                        .text_color(cx.theme().muted_foreground)
                        .child("No devices or network drives connected.")
                        .into_any_element()
                } else {
                    let mut device_cards: Vec<AnyElement> = Vec::new();
                    for volume in &devices {
                        device_cards.push(self.drive_card(volume, cx).into_any_element());
                    }
                    h_flex()
                        .w_full()
                        .flex_wrap()
                        .gap(px(10.))
                        .children(device_cards)
                        .into_any_element()
                }),
        );

        if !self.quick_access.is_empty() {
            let mut pins: Vec<AnyElement> = Vec::new();
            for path in &self.quick_access {
                pins.push(self.quick_access_card(path, cx).into_any_element());
            }
            sections = sections.child(
                v_flex()
                    .w_full()
                    .gap(px(10.))
                    .child(section_label("Quick access", cx))
                    .child(h_flex().w_full().flex_wrap().gap(px(10.)).children(pins)),
            );
        }

        div()
            .id("home-scroll")
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .child(sections)
            .overflow_y_scrollbar()
    }

    fn drive_card(&self, volume: &Volume, cx: &mut Context<Self>) -> impl IntoElement {
        let pct = volume.pct_used();
        let icon = match volume.kind {
            VolumeKind::Network => IconName::Network,
            _ => IconName::HardDrive,
        };
        let hover_bg = cx.theme().accent;
        let path = volume.path.clone();

        v_flex()
            .id(SharedString::from(format!("card-{}", volume.id)))
            .w(px(236.))
            .p(px(14.))
            .gap(px(10.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(crate::theme::card_bg(cx))
            .cursor_pointer()
            .hover(move |style| style.bg(hover_bg))
            .child(
                h_flex()
                    .gap(px(8.))
                    .items_center()
                    .child(
                        Icon::new(icon)
                            .size(px(16.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(UI_TEXT))
                            .child(volume.name.clone()),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(4.))
                    .bg(crate::theme::chart_bar_track(cx))
                    .child(
                        div()
                            .h_full()
                            .w(relative(pct as f32 / 100.))
                            .bg(crate::theme::chart_bar(cx)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child(volume.free_label())
                    .child(volume.total_label()),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.focus.focus(window, cx);
                this.expand_sidebar_row(path.clone(), cx);
                this.enter_root(path.clone(), path.clone(), cx);
            }))
    }

    fn quick_access_card(&self, path: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let hover_bg = cx.theme().accent;
        let target = path.to_path_buf();
        h_flex()
            .id(SharedString::from(format!("pin-{}", path_id(path))))
            .w(px(160.))
            .p(px(10.))
            .gap(px(8.))
            .items_center()
            .border_1()
            .border_color(cx.theme().border)
            .bg(crate::theme::card_bg(cx))
            .cursor_pointer()
            .hover(move |style| style.bg(hover_bg))
            .child(
                Icon::new(IconName::Folder)
                    .size(px(14.))
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(UI_TEXT))
                    .child(folder_label(path)),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.focus.focus(window, cx);
                this.navigate_to(target.clone(), cx);
            }))
    }

    // -- Folder view --------------------------------------------------------

    fn folder_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = self.visible_entries();
        let body = match &self.listing {
            LoadState::Loading if entries.is_empty() => center_message(
                h_flex()
                    .gap(px(8.))
                    .items_center()
                    .child(Spinner::new().small())
                    .child("Listing folder")
                    .into_any_element(),
                cx,
            )
            .into_any_element(),
            LoadState::Failed { message } => center_message(
                div()
                    .text_color(cx.theme().danger)
                    .child(message.clone())
                    .into_any_element(),
                cx,
            )
            .into_any_element(),
            _ if entries.is_empty() => center_message(
                v_flex()
                    .items_center()
                    .gap(px(8.))
                    .child(Icon::new(IconName::Inbox).size(px(24.)))
                    .child(if self.filter_text.trim().is_empty() {
                        "This folder is empty"
                    } else {
                        "No entries match the filter"
                    })
                    .into_any_element(),
                cx,
            )
            .into_any_element(),
            _ => match self.view_mode {
                ViewMode::List => self.list_body(&entries, cx).into_any_element(),
                ViewMode::Grid => self.grid_body(&entries, cx).into_any_element(),
            },
        };

        v_flex()
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .when(self.view_mode == ViewMode::List, |this| {
                this.child(self.list_header(cx))
            })
            .child(body)
    }

    fn list_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(px(26.))
            .flex_none()
            .items_center()
            .px(px(10.))
            .gap(px(8.))
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().table_head)
            .text_size(px(11.5))
            .text_color(cx.theme().table_head_foreground)
            .child(self.column_header("Name", "name", None, cx))
            .child(self.column_header("Kind", "kind", Some(KIND_WIDTH), cx))
            .child(self.column_header("Size", "size", Some(SIZE_WIDTH), cx))
            .child(self.column_header("Modified", "modified", Some(MODIFIED_WIDTH), cx))
    }

    fn column_header(
        &self,
        label: &'static str,
        key: &'static str,
        width: Option<f32>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sorted = self.sort_key == key;
        let ascending = self.sort_ascending;
        h_flex()
            .id(SharedString::from(format!("column-{key}")))
            .gap(px(4.))
            .items_center()
            .cursor_pointer()
            .when_some(width, |this, width| this.w(px(width)).flex_none())
            .when(width.is_none(), |this| this.flex_1().min_w(px(0.)))
            .when(key == "size", |this| this.justify_end())
            .child(label)
            .when(sorted, |this| {
                this.child(
                    Icon::new(if ascending {
                        IconName::SortAscending
                    } else {
                        IconName::SortDescending
                    })
                    .size(px(11.)),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.focus.focus(window, cx);
                this.sort_by(key, cx);
            }))
    }

    fn list_body(&self, entries: &[&Entry], cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<AnyElement> = entries
            .iter()
            .enumerate()
            .map(|(ix, entry)| self.list_row(ix, entry, cx).into_any_element())
            .collect();

        div()
            .id("list-scroll")
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .child(v_flex().w_full().children(rows))
            .overflow_y_scrollbar()
    }

    fn list_row(&self, ix: usize, entry: &Entry, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.is_selected(&entry.path);
        let muted = cx.theme().muted_foreground;
        let hover_bg = if selected {
            crate::theme::select_strong(cx)
        } else {
            cx.theme().table_hover
        };
        let size = if entry.is_directory() {
            "—".to_string()
        } else {
            format_size(entry.size)
        };

        h_flex()
            .id(("row", ix))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex_none()
            .items_center()
            .px(px(10.))
            .gap(px(8.))
            .text_size(px(UI_TEXT))
            .cursor_pointer()
            .when(selected, |this| this.bg(crate::theme::select_strong(cx)))
            .hover(move |style| style.bg(hover_bg))
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(8.))
                    .items_center()
                    .child(Icon::new(entry_icon(entry)).size(px(14.)).text_color(muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .child(entry.name.clone()),
                    ),
            )
            .child(
                div()
                    .w(px(KIND_WIDTH))
                    .flex_none()
                    .truncate()
                    .text_color(muted)
                    .child(entry_kind_label(entry)),
            )
            .child(
                div()
                    .w(px(SIZE_WIDTH))
                    .flex_none()
                    .text_right()
                    .text_color(muted)
                    .child(size),
            )
            .child(
                div()
                    .w(px(MODIFIED_WIDTH))
                    .flex_none()
                    .truncate()
                    .text_color(muted)
                    .child(format_mtime(entry.modified)),
            )
            .on_click(self.row_click(ix, cx))
            .on_mouse_down(MouseButton::Right, self.row_right_click(ix, cx))
            .context_menu(entry_context_menu(self.focus.clone()))
    }

    fn grid_body(&self, entries: &[&Entry], cx: &mut Context<Self>) -> impl IntoElement {
        let cells: Vec<AnyElement> = entries
            .iter()
            .enumerate()
            .map(|(ix, entry)| self.grid_cell(ix, entry, cx).into_any_element())
            .collect();

        div()
            .id("grid-scroll")
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .items_start()
                    .p(px(10.))
                    .gap(px(6.))
                    .children(cells),
            )
            .overflow_y_scrollbar()
    }

    fn grid_cell(&self, ix: usize, entry: &Entry, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.is_selected(&entry.path);
        let hover_bg = if selected {
            crate::theme::select_strong(cx)
        } else {
            cx.theme().table_hover
        };

        v_flex()
            .id(("cell", ix))
            .w(px(CELL_WIDTH))
            .h(px(86.))
            .flex_none()
            .p(px(6.))
            .gap(px(6.))
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(selected, |this| this.bg(crate::theme::select_strong(cx)))
            .hover(move |style| style.bg(hover_bg))
            .child(
                Icon::new(entry_icon(entry))
                    .size(px(28.))
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .w_full()
                    .text_size(px(11.))
                    .text_center()
                    .truncate()
                    .child(entry.name.clone()),
            )
            .on_click(self.row_click(ix, cx))
            .on_mouse_down(MouseButton::Right, self.row_right_click(ix, cx))
            .context_menu(entry_context_menu(self.focus.clone()))
    }

    /// Single click selects (Ctrl/Shift aware), double click opens.
    fn row_click(
        &self,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static {
        cx.listener(move |this, event: &ClickEvent, window, cx| {
            this.focus.focus(window, cx);
            if event.click_count() >= 2 {
                if let Some(entry) = this.visible_entries().get(ix).map(|e| (*e).clone()) {
                    this.open_entry(entry, cx);
                }
            } else {
                this.select_at(ix, event.modifiers(), cx);
            }
        })
    }

    /// Right click selects the row it landed on unless it is already selected.
    fn row_right_click(
        &self,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut gpui::App) + 'static {
        cx.listener(move |this, _, window, cx| {
            this.focus.focus(window, cx);
            let path = this
                .visible_entries()
                .get(ix)
                .map(|entry| entry.path.clone());
            if let Some(path) = path
                && !this.is_selected(&path)
            {
                this.select_at(ix, Default::default(), cx);
            }
        })
    }

    // -- Status bar ---------------------------------------------------------

    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.visible_entries().len();
        let total = self.total_entries();
        let selected = self.selected.len();
        let counts = if visible == total {
            format!("{visible} items · {selected} selected")
        } else {
            format!("{visible} of {total} items · {selected} selected")
        };

        h_flex()
            .w_full()
            .h(px(STATUS_BAR_HEIGHT))
            .flex_none()
            .items_center()
            .px(px(10.))
            .gap(px(8.))
            .border_t_1()
            .border_color(cx.theme().status_bar_border)
            .bg(cx.theme().status_bar)
            .text_size(px(11.5))
            .child(div().text_color(cx.theme().muted_foreground).child(counts))
            .child(div().flex_1())
            .child(
                div().key_context("PlyFilter").w(px(190.)).child(
                    Input::new(&self.filter)
                        .xsmall()
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).size(px(11.))),
                ),
            )
            .child(
                Button::new("hidden")
                    .ghost()
                    .xsmall()
                    .icon(if self.show_hidden {
                        IconName::Eye
                    } else {
                        IconName::EyeOff
                    })
                    .selected(self.show_hidden)
                    .tooltip_with_action("Show hidden entries", &ToggleHidden, None)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_hidden(cx))),
            )
            .child(
                Button::new("view-list")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Menu)
                    .selected(self.view_mode == ViewMode::List)
                    .tooltip("List view")
                    .on_click(cx.listener(|this, _, _, cx| this.set_view_mode(ViewMode::List, cx))),
            )
            .child(
                Button::new("view-grid")
                    .ghost()
                    .xsmall()
                    .icon(IconName::LayoutDashboard)
                    .selected(self.view_mode == ViewMode::Grid)
                    .tooltip("Grid view")
                    .on_click(cx.listener(|this, _, _, cx| this.set_view_mode(ViewMode::Grid, cx))),
            )
    }

    // -- Properties ---------------------------------------------------------

    fn properties_modal(&self, entry: Entry, cx: &mut Context<Self>) -> impl IntoElement {
        let size = if entry.is_directory() {
            "—".to_string()
        } else {
            format_size(entry.size)
        };
        let link_target = match &entry.kind {
            EntryKind::Symlink { target } => Some(target.display().to_string()),
            _ => None,
        };

        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("properties-backdrop")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .bg(cx.theme().overlay)
                    .on_click(cx.listener(|this, _, _, cx| this.close_properties(cx))),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        v_flex()
                            .id("properties-card")
                            .occlude()
                            .w(px(440.))
                            .p(px(16.))
                            .gap(px(12.))
                            .bg(crate::theme::card_bg(cx))
                            .border_1()
                            .border_color(cx.theme().border)
                            .text_size(px(UI_TEXT))
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap(px(8.))
                                    .items_center()
                                    .child(
                                        Icon::new(entry_icon(&entry))
                                            .size(px(16.))
                                            .text_color(cx.theme().muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .truncate()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(entry.name.clone()),
                                    )
                                    .child(
                                        Button::new("properties-close")
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::Close)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.close_properties(cx)
                                            })),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .w_full()
                                    .gap(px(6.))
                                    .child(property_row("Kind", entry_kind_label(&entry), cx))
                                    .child(property_row("Size", size, cx))
                                    .child(property_row(
                                        "Modified",
                                        format_mtime(entry.modified),
                                        cx,
                                    ))
                                    .child(property_row(
                                        "Hidden",
                                        if entry.hidden { "Yes" } else { "No" }.to_string(),
                                        cx,
                                    ))
                                    .child(property_row(
                                        "Path",
                                        entry.path.display().to_string(),
                                        cx,
                                    ))
                                    .when_some(link_target, |this, target| {
                                        this.child(property_row("Link target", target, cx))
                                    }),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .justify_end()
                                    .gap(px(6.))
                                    .child(
                                        Button::new("properties-copy")
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::Copy)
                                            .label("Copy path")
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.copy_path(cx)),
                                            ),
                                    )
                                    .child(
                                        Button::new("properties-done")
                                            .outline()
                                            .xsmall()
                                            .label("Close")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.close_properties(cx)
                                            })),
                                    ),
                            ),
                    ),
            )
    }
}

/// Read-only entry menu. Deferred menu elements dispatch outside the row's
/// element tree, so the menu is pointed back at Ply's focus handle.
fn entry_context_menu(
    focus: FocusHandle,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, _, _| {
        menu.action_context(focus.clone())
            .menu("Open", Box::new(OpenSelection))
            .menu("Copy path", Box::new(CopyPath))
            .menu("Reveal", Box::new(Reveal))
            .separator()
            .menu("Properties", Box::new(ShowProperties))
    }
}

fn entry_icon(entry: &Entry) -> IconName {
    match entry.kind {
        EntryKind::Directory => IconName::Folder,
        EntryKind::Symlink { .. } => IconName::ExternalLink,
        EntryKind::File => IconName::File,
    }
}

fn section_label(label: &'static str, cx: &mut Context<Ply>) -> impl IntoElement {
    div()
        .px(px(10.))
        .py(px(6.))
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(label.to_uppercase())
}

fn property_row(label: &'static str, value: String, cx: &mut Context<Ply>) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap(px(10.))
        .items_start()
        .child(
            div()
                .w(px(96.))
                .flex_none()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(div().flex_1().min_w(px(0.)).child(value))
}

fn center_message(body: AnyElement, cx: &mut Context<Ply>) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_h(px(0.))
        .w_full()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .text_color(cx.theme().muted_foreground)
        .child(body)
}
