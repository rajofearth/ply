use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, anchored, deferred, div, img, prelude::FluentBuilder, px,
};

use super::icon;
use crate::app::Ply;
use crate::app::menu::{MenuItem, MenuKind, MenuRow};
use crate::icons::Ico;
use crate::preview::Preview;

/// The layers that float above the panes: context menu, then Quick Look, then Properties.
pub fn render(ply: &Ply, cx: &mut Context<Ply>) -> Vec<AnyElement> {
    let mut layers = Vec::new();
    if let Some(menu) = context_menu(ply, cx) {
        layers.push(menu);
    }
    if let Some(ql) = quick_look(ply, cx) {
        layers.push(ql);
    }
    if let Some(dialog) = properties(ply, cx) {
        layers.push(dialog);
    }
    layers
}

fn context_menu(ply: &Ply, cx: &mut Context<Ply>) -> Option<AnyElement> {
    let menu = ply.menu.as_ref()?;
    let p = ply.palette();
    let has_toolbar = !menu.toolbar.is_empty();
    let flyout = menu.flyout.and_then(|i| match menu.rows.get(i) {
        Some(MenuRow::Item(item)) if !item.children.is_empty() => Some((i, item.children.clone())),
        _ => None,
    });

    let toolbar = menu.toolbar.clone();
    let rows = menu.rows.clone();
    let kind = menu.kind;

    let panel = div()
        .occlude()
        .min_w(px(260.))
        .max_w(px(320.))
        .py(px(4.))
        .bg(p.card)
        .border_1()
        .border_color(p.border)
        .shadow_lg()
        .when(has_toolbar, |el| {
            el.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .px(px(6.))
                    .py(px(4.))
                    .border_b_1()
                    .border_color(p.border)
                    .children(toolbar.into_iter().enumerate().map(|(i, btn)| {
                        let action = btn.action.clone();
                        let enabled = btn.enabled;
                        div()
                            .id(("tool", i))
                            .w(px(32.))
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_default()
                            .when(enabled, |el| el.hover(|s| s.bg(p.muted)))
                            .when(!enabled, |el| el.opacity(0.35))
                            .child(icon(
                                btn.icon,
                                px(16.),
                                if btn.danger {
                                    p.destructive
                                } else if enabled {
                                    p.foreground
                                } else {
                                    p.muted_foreground
                                },
                            ))
                            .when(enabled, |el| {
                                el.on_click(cx.listener(move |this, _, window, cx| {
                                    this.run(action.clone(), window, cx);
                                }))
                            })
                    })),
            )
        })
        .children(rows.into_iter().enumerate().map(|(i, row)| {
            match row {
                MenuRow::Separator => div()
                    .h(px(1.))
                    .my(px(4.))
                    .mx(px(8.))
                    .bg(p.border)
                    .into_any_element(),
                MenuRow::Item(item) => menu_row(ply, i, item, kind, cx),
            }
        }));

    let mut stack = vec![
        deferred(
            anchored()
                .position(menu.at)
                .snap_to_window_with_margin(px(8.))
                .child(panel),
        )
        .into_any_element(),
    ];

    if let Some((ix, children)) = flyout {
        let y_off = if has_toolbar { 42. } else { 4. } + ix as f32 * 28.;
        let at = gpui::Point::new(menu.at.x + px(252.), menu.at.y + px(y_off));
        stack.push(
            deferred(
                anchored()
                    .position(at)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .occlude()
                            .min_w(px(180.))
                            .py(px(4.))
                            .bg(p.card)
                            .border_1()
                            .border_color(p.border)
                            .shadow_lg()
                            .children(children.into_iter().enumerate().map(|(j, row)| match row {
                                MenuRow::Separator => {
                                    div().h(px(1.)).my(px(4.)).bg(p.border).into_any_element()
                                }
                                MenuRow::Item(item) => {
                                    menu_row(ply, 1000 + ix * 20 + j, item, kind, cx)
                                }
                            })),
                    ),
            )
            .into_any_element(),
        );
    }

    Some(
        div()
            .absolute()
            .inset_0()
            .children(stack)
            .into_any_element(),
    )
}

fn menu_row(
    ply: &Ply,
    i: usize,
    item: MenuItem,
    _kind: MenuKind,
    cx: &mut Context<Ply>,
) -> AnyElement {
    let p = ply.palette();
    let enabled = item.enabled;
    let has_kids = !item.children.is_empty();
    let action = item.action.clone();
    let danger = item.danger;
    let color = if !enabled {
        p.muted_foreground
    } else if danger {
        p.destructive
    } else {
        p.foreground
    };

    div()
        .id(("menu", i))
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(12.))
        .py(px(6.))
        .h(px(28.))
        .text_size(px(12.5))
        .cursor_default()
        .text_color(color)
        .when(enabled, |el| el.hover(|s| s.bg(p.muted)))
        .when(!enabled, |el| el.opacity(0.55))
        .map(|el| {
            if let Some(ico) = item.icon {
                el.child(icon(ico, px(14.), color))
            } else {
                el
            }
        })
        .child(div().flex_1().child(item.label.clone()))
        .when(has_kids, |el| {
            el.child(icon(Ico::ChevronRight, px(12.), p.muted_foreground))
        })
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if has_kids && *hovered {
                this.set_flyout(Some(i), cx);
            }
        }))
        .when(enabled && action.is_some(), |el| {
            let action = action.clone().unwrap();
            el.on_click(cx.listener(move |this, _, window, cx| {
                this.run(action.clone(), window, cx);
            }))
        })
        .when(enabled && has_kids, |el| {
            el.on_click(cx.listener(move |this, _, _, cx| {
                this.set_flyout(Some(i), cx);
            }))
        })
        .into_any_element()
}

fn quick_look(ply: &Ply, cx: &mut Context<Ply>) -> Option<AnyElement> {
    let ql = ply.quick_look.as_ref()?;
    let p = ply.palette();
    let name: SharedString = ql
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .into();

    let body = match &ql.preview {
        Preview::Image(path) => img(path.clone())
            .max_w(px(900.))
            .max_h(px(620.))
            .into_any_element(),
        Preview::Thumbnail { image, caption } => div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .child(img(image.clone()).max_w(px(900.)).max_h(px(580.)))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(p.muted_foreground)
                    .child(caption.clone()),
            )
            .into_any_element(),
        Preview::Text { content, truncated } => div()
            .id("ql-text")
            .max_w(px(720.))
            .max_h(px(560.))
            .overflow_y_scroll()
            .p(px(16.))
            .text_size(px(12.5))
            .child(content.clone())
            .when(*truncated, |el| {
                el.child(
                    div()
                        .mt(px(8.))
                        .text_color(p.muted_foreground)
                        .child("Preview truncated."),
                )
            })
            .into_any_element(),
        Preview::Card { kind, size, hint } => div()
            .w(px(360.))
            .p(px(16.))
            .child(div().text_size(px(13.)).mb(px(8.)).child(kind.clone()))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(p.muted_foreground)
                    .mb(px(12.))
                    .child(size.clone()),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(p.muted_foreground)
                    .child(hint.clone()),
            )
            .into_any_element(),
    };

    Some(
        deferred(
            div()
                .id("ql-scrim")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(p.overlay)
                .on_click(cx.listener(|this, _, _, cx| this.close_quick_look(cx)))
                .child(
                    div()
                        .occlude()
                        .bg(p.card)
                        .border_1()
                        .border_color(p.border)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px(px(14.))
                                .py(px(8.))
                                .border_b_1()
                                .border_color(p.border)
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(p.muted_foreground)
                                        .child("QUICK LOOK"),
                                )
                                .child(
                                    div()
                                        .id("close-ql")
                                        .flex()
                                        .cursor_default()
                                        .child(icon(Ico::X, px(14.), p.muted_foreground))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.close_quick_look(cx)),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .px(px(14.))
                                .py(px(8.))
                                .text_size(px(14.))
                                .truncate()
                                .child(name),
                        )
                        .child(div().px(px(14.)).pb(px(14.)).child(body)),
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
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.close_properties(cx)),
                                        ),
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
