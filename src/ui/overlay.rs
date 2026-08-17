use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, anchored, deferred, div, px,
};

use super::icon;
use crate::app::Ply;
use crate::icons::Ico;

/// The layers that float above the panes: context menu, then Properties.
pub fn render(ply: &Ply, cx: &mut Context<Ply>) -> Vec<AnyElement> {
    let mut layers = Vec::new();
    if let Some(menu) = context_menu(ply, cx) {
        layers.push(menu);
    }
    if let Some(dialog) = properties(ply, cx) {
        layers.push(dialog);
    }
    layers
}

fn context_menu(ply: &Ply, cx: &mut Context<Ply>) -> Option<AnyElement> {
    let menu = ply.menu.as_ref()?;
    let p = ply.palette();

    let items = menu.items.clone();
    Some(
        deferred(
            anchored().position(menu.at).snap_to_window_with_margin(px(8.)).child(
                div()
                    .occlude()
                    .min_w(px(180.))
                    .py(px(2.))
                    .bg(p.card)
                    .border_1()
                    .border_color(p.border)
                    .shadow_lg()
                    .children(items.into_iter().enumerate().map(|(i, item)| {
                        let action = item.action.clone();
                        div()
                            .id(("menu", i))
                            .px(px(12.))
                            .py(px(7.))
                            .text_size(px(12.5))
                            .cursor_default()
                            .text_color(if item.danger {
                                p.destructive
                            } else {
                                p.foreground
                            })
                            .hover(|s| s.bg(p.muted))
                            .child(item.label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run(action.clone(), window, cx);
                            }))
                    })),
            ),
        )
        .into_any_element(),
    )
}

fn properties(ply: &Ply, cx: &mut Context<Ply>) -> Option<AnyElement> {
    let props = ply.properties.as_ref()?;
    let p = ply.palette();

    let rows = [
        ("Type", props.kind.clone()),
        ("Size", props.size.clone()),
        ("Modified", props.modified.clone()),
        ("Location", props.location.clone()),
    ];

    Some(
        deferred(
            div()
                .id("scrim")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(p.overlay)
                .on_click(cx.listener(|this, _, _, cx| this.close_properties(cx)))
                .child(
                    div()
                        .occlude()
                        .w(px(320.))
                        .bg(p.card)
                        .border_1()
                        .border_color(p.border)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px(px(14.))
                                .py(px(10.))
                                .border_b_1()
                                .border_color(p.border)
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(p.muted_foreground)
                                        .child("PROPERTIES"),
                                )
                                .child(
                                    div()
                                        .id("close-props")
                                        .flex()
                                        .cursor_default()
                                        .child(icon(Ico::X, px(14.), p.muted_foreground))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_properties(cx)
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .p(px(14.))
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .mb(px(10.))
                                        .truncate()
                                        .child(props.name.clone()),
                                )
                                .children(rows.into_iter().map(|(label, value)| {
                                    div()
                                        .flex()
                                        .justify_between()
                                        .gap(px(12.))
                                        .py(px(6.))
                                        .border_b_1()
                                        .border_color(p.border)
                                        .text_size(px(12.))
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_color(p.muted_foreground)
                                                .child(label),
                                        )
                                        .child(div().truncate().text_right().child(value))
                                })),
                        ),
                ),
        )
        .into_any_element(),
    )
}
