use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div, prelude::FluentBuilder, px,
};
use gpui_component::Sizable;
use gpui_component::input::Input;

use super::icon;
use crate::app::{Ply, ViewMode};
use crate::icons::Ico;

/// Only shown inside a folder: counts, the filter, and the view toggle.
pub fn render(ply: &Ply, cx: &mut Context<Ply>) -> impl IntoElement {
    let p = ply.palette();
    let shown = ply.visible().len();
    let selected = ply.tab().selection.len();

    let left = match &ply.status {
        Some(message) => message.to_string(),
        None if selected > 0 => format!("{shown} items · {selected} selected"),
        None => format!("{shown} items"),
    };

    div()
        .flex()
        .items_center()
        .justify_between()
        .h(px(26.))
        .px(px(12.))
        .flex_none()
        .border_t_1()
        .border_color(p.border)
        .text_size(px(11.))
        .text_color(p.muted_foreground)
        .child(div().truncate().child(left))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .flex_none()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .w(px(170.))
                        .px(px(8.))
                        .py(px(3.))
                        .border_1()
                        .border_color(p.border)
                        .child(icon(Ico::Search, px(12.), p.muted_foreground))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(ply.filter()).xsmall().appearance(false)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .border_1()
                        .border_color(p.border)
                        .child(toggle(ply, ViewMode::List, Ico::List, cx))
                        .child(toggle(ply, ViewMode::Grid, Ico::LayoutGrid, cx))
                        .child(toggle(ply, ViewMode::Column, Ico::Columns, cx)),
                ),
        )
}

fn toggle(ply: &Ply, view: ViewMode, ico: Ico, cx: &mut Context<Ply>) -> impl IntoElement {
    let p = ply.palette();
    let on = ply.view() == view;
    div()
        .id(match view {
            ViewMode::List => "view-list",
            ViewMode::Grid => "view-grid",
            ViewMode::Column => "view-column",
        })
        .flex()
        .px(px(6.))
        .py(px(3.))
        .cursor_default()
        .when(on, |el| el.bg(p.accent))
        .when(!on, |el| el.hover(|s| s.bg(p.muted)))
        .child(icon(
            ico,
            px(12.),
            if on { p.foreground } else { p.muted_foreground },
        ))
        .on_click(cx.listener(move |this, _, _, cx| this.set_view(view, cx)))
}
