use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px,
};

use super::icon;
use crate::app::Ply;
use crate::icons::Ico;

/// Tab strip under the title bar. Always visible so a single tab can still spawn another.
pub fn render(ply: &Ply, cx: &mut Context<Ply>) -> impl IntoElement {
    let p = ply.palette();
    let active = ply.active_index();

    div()
        .flex()
        .items_center()
        .h(px(28.))
        .px(px(8.))
        .gap(px(4.))
        .flex_none()
        .border_b_1()
        .border_color(p.border)
        .children(ply.tabs().iter().enumerate().map(|(ix, tab)| {
            let id = tab.id;
            let title = ply.tab_title(tab);
            let on = ix == active;
            div()
                .id(("tab", id))
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(22.))
                .px(px(8.))
                .max_w(px(180.))
                .cursor_default()
                .when(on, |el| {
                    el.bg(p.select_strong)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.foreground)
                })
                .when(!on, |el| {
                    el.text_color(p.muted_foreground).hover(|s| s.bg(p.muted))
                })
                .child(div().truncate().text_size(px(12.)).child(title))
                .child(
                    div()
                        .id(("tab-x", id))
                        .flex()
                        .child(icon(Ico::X, px(11.), p.muted_foreground))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_tab(id, window, cx);
                        })),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.activate_tab(ix, cx)))
        }))
        .child(
            div()
                .id("new-tab")
                .flex()
                .items_center()
                .justify_center()
                .w(px(22.))
                .h(px(22.))
                .cursor_default()
                .hover(|s| s.bg(p.muted))
                .child(icon(Ico::Plus, px(12.), p.muted_foreground))
                .on_click(cx.listener(|this, _, window, cx| this.shortcut_new_tab(window, cx))),
        )
}
