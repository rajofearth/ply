use gpui::{
    Context, Focusable, InteractiveElement, IntoElement, Keystroke, ParentElement, Styled, Window,
    div, prelude::FluentBuilder, px,
};
use gpui_component::breadcrumb::{Breadcrumb, BreadcrumbItem};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::kbd::Kbd;
use gpui_component::label::Label;
use gpui_component::resizable::{h_resizable, resizable_panel};
use gpui_component::separator::Separator;
use gpui_component::spinner::Spinner;
use gpui_component::status_bar::StatusBar;
use gpui_component::switch::Switch;
use gpui_component::table::DataTable;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, TitleBar, h_flex, v_flex};

use crate::listing::parent_in_workspace;
use crate::preview::preview_el;
use crate::theme;
use crate::{GoToParent, LoadState, OpenFolder, Ply, Refresh};

impl Ply {
    pub(crate) fn typing_in_filter(&self, window: &Window, cx: &gpui::App) -> bool {
        self.filter.focus_handle(cx).is_focused(window)
    }

    pub(crate) fn render_chrome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                TitleBar::new()
                    .bg(cx.theme().title_bar)
                    .border_color(cx.theme().title_bar_border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::FolderOpen).small())
                            .child("Ply"),
                    ),
            )
            .when_some(self.banner.clone(), |this, msg| {
                this.child(gpui_component::alert::Alert::warning("banner", msg).banner())
            })
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().flex_1().min_w(px(120.)).child(self.breadcrumbs(cx)))
                    .child(div().w(px(240.)).child(self.filter_box())),
            )
            .child(
                h_resizable("explorer-split")
                    .child(
                        resizable_panel()
                            .size(px(240.))
                            .size_range(px(160.)..px(420.))
                            .child(
                                v_flex()
                                    .size_full()
                                    .bg(theme::sidebar_bg(cx))
                                    .child(self.tree_header(cx))
                                    .child(
                                        div()
                                            .flex_1()
                                            .key_context("PlyList")
                                            .child(self.render_tree(cx)),
                                    ),
                            ),
                    )
                    .child(
                        resizable_panel().child(
                            v_flex()
                                .size_full()
                                .bg(theme::pane_bg(cx))
                                .key_context("PlyList")
                                .child(self.table_el()),
                        ),
                    )
                    .child(
                        resizable_panel()
                            .size(px(320.))
                            .size_range(px(200.)..px(640.))
                            .child(self.preview_wrap(cx)),
                    ),
            )
            .flex_1()
            .child(self.action_row(cx))
            .child(self.status_el(cx))
    }

    fn tree_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_1p5()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Icon::new(IconName::Folder)
                    .small()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Folders"),
            )
            .when(matches!(self.listing, LoadState::Loading), |this| {
                this.child(Spinner::new().small().color(cx.theme().muted_foreground))
            })
    }

    fn breadcrumbs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let mut items = Vec::new();

        let at_workspace = self.current_folder == self.workspace;
        let workspace = self.workspace.clone();
        let home = view.clone();
        items.push(
            BreadcrumbItem::new("Workspace")
                .disabled(at_workspace)
                .on_click(move |_, _, cx| {
                    home.update(cx, |this, cx| {
                        this.set_current_folder(workspace.clone(), cx);
                    });
                }),
        );

        if let Ok(rel) = self.current_folder.strip_prefix(&self.workspace) {
            let mut acc = self.workspace.clone();
            let parts: Vec<_> = rel.components().collect();
            let last = parts.len().saturating_sub(1);
            for (i, part) in parts.into_iter().enumerate() {
                acc.push(part.as_os_str());
                let label = part.as_os_str().to_string_lossy().into_owned();
                let path = acc.clone();
                let jump = view.clone();
                items.push(
                    BreadcrumbItem::new(label)
                        .disabled(i == last)
                        .on_click(move |_, _, cx| {
                            jump.update(cx, |this, cx| {
                                this.set_current_folder(path.clone(), cx);
                            });
                        }),
                );
            }
        }

        Breadcrumb::new().children(items)
    }

    fn filter_box(&self) -> impl IntoElement {
        Input::new(&self.filter)
            .small()
            .prefix(Icon::new(IconName::Search).small())
    }

    fn table_el(&self) -> impl IntoElement {
        div()
            .flex_1()
            .child(DataTable::new(&self.table).stripe(true))
    }

    fn preview_wrap(&self, cx: &gpui::App) -> impl IntoElement {
        div()
            .size_full()
            .bg(theme::pane_bg(cx))
            .child(preview_el(&self.preview, cx))
    }

    fn action_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_1()
            .items_center()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("open-folder")
                    .ghost()
                    .icon(IconName::FolderOpen)
                    .tooltip_with_action("Open Workspace", &OpenFolder, None)
                    .label("Open")
                    .on_click(cx.listener(|this, _, _, cx| this.pick_workspace(cx))),
            )
            .child(
                Button::new("refresh")
                    .ghost()
                    .icon(IconName::Redo)
                    .tooltip_with_action("Refresh listing", &Refresh, None)
                    .label("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| this.reload_listing(cx))),
            )
            .child(
                Button::new("up")
                    .ghost()
                    .icon(IconName::ChevronUp)
                    .tooltip_with_action("Go to parent", &GoToParent, None)
                    .label("Up")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(parent) =
                            parent_in_workspace(&this.current_folder, &this.workspace)
                        {
                            this.set_current_folder(parent, cx);
                        }
                    })),
            )
            .child(Separator::vertical())
            .child({
                let view = cx.entity();
                Switch::new("hidden")
                    .checked(self.show_hidden)
                    .on_click(move |checked, _, cx| {
                        view.update(cx, |this, cx| {
                            this.show_hidden = *checked;
                            this.reload_listing(cx);
                        });
                    })
            })
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Icon::new(if self.show_hidden {
                            IconName::Eye
                        } else {
                            IconName::EyeOff
                        })
                        .small()
                        .text_color(cx.theme().muted_foreground),
                    )
                    .child(div().text_sm().child("Hidden")),
            )
    }

    fn status_el(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let delegate = self.table.read(cx).delegate();
        let visible = delegate.visible_len();
        let total = delegate.total_len();
        let selected = self
            .selected
            .as_ref()
            .map(|e| e.name.as_str())
            .unwrap_or("nothing selected");
        let load = match &self.listing {
            LoadState::Loading => "loading",
            LoadState::Failed { .. } => "failed",
            LoadState::Ready(_) => "ready",
            LoadState::Idle => "idle",
        };
        let mut bar = StatusBar::new()
            .left(format!("{visible} of {total}"))
            .left(Separator::vertical())
            .left(Label::new(selected))
            .right(load);
        if let Ok(stroke) = Keystroke::parse("alt-up") {
            bar = bar.right(Kbd::new(stroke));
        }
        bar
    }
}
