use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, anchored, deferred, div, prelude::FluentBuilder, px,
};

use super::icon;
use crate::app::{MenuItem, MenuRow, Ply};
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
    if let Some(dialog) = confirm_dialog(ply, cx) {
        layers.push(dialog);
    }
    layers
}

fn context_menu(ply: &Ply, cx: &mut Context<Ply>) -> Option<AnyElement> {
    let menu = ply.menu.as_ref()?;
    let p = ply.palette();
    let has_toolbar = !menu.toolbar.is_empty();
    let flyout = menu.flyout.and_then(|i| match menu.rows.get(i) {
        Some(MenuRow::Item(item)) if !item.children.is_empty() => Some(i),
        _ => None,
    });

    let panel = chrome(p)
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
                    .children(menu.toolbar.iter().enumerate().map(|(i, btn)| {
                        let action = btn.action.clone();
                        let enabled = btn.enabled;
                        let color = if btn.danger {
                            p.destructive
                        } else if enabled {
                            p.foreground
                        } else {
                            p.muted_foreground
                        };
                        div()
                            .id(("tool", i))
                            .w(px(32.))
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_default()
                            .when(enabled, |el| el.hover(|s| s.bg(p.muted)))
                            .child(icon(btn.icon, px(16.), color))
                            .when(enabled, |el| {
                                el.on_click(cx.listener(move |this, _, window, cx| {
                                    this.run(action.clone(), window, cx);
                                }))
                            })
                    })),
            )
        })
        .children(paint_rows(ply, &menu.rows, 0, cx));

    let mut stack = vec![
        deferred(
            anchored()
                .position(menu.at)
                .snap_to_window_with_margin(px(8.))
                .child(panel),
        )
        .into_any_element(),
    ];

    if let Some(ix) = flyout {
        let y_off = if has_toolbar { 42. } else { 4. } + ix as f32 * 28.;
        let at = gpui::Point::new(menu.at.x + px(172.), menu.at.y + px(y_off));
        let kids = match &menu.rows[ix] {
            MenuRow::Item(item) => item.children.as_slice(),
            MenuRow::Separator => &[],
        };
        stack.push(
            deferred(
                anchored()
                    .position(at)
                    .snap_to_window_with_margin(px(8.))
                    .child(chrome(p).children(paint_rows(ply, kids, 1000 + ix * 20, cx))),
            )
            .into_any_element(),
        );
    }

    Some(
        div()
            .id("menu-dismiss")
            .absolute()
            .inset_0()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close_menu(cx)),
            )
            .children(stack)
            .into_any_element(),
    )
}

fn chrome(p: crate::theme::Palette) -> gpui::Div {
    div()
        .occlude()
        .flex_none()
        .py(px(4.))
        .bg(p.card)
        .border_1()
        .border_color(p.border)
        .shadow_lg()
}

fn paint_rows(ply: &Ply, rows: &[MenuRow], base: usize, cx: &mut Context<Ply>) -> Vec<AnyElement> {
    let hair = ply.palette().border;
    rows.iter()
        .enumerate()
        .map(|(i, row)| match row {
            MenuRow::Separator => div()
                .h(px(1.))
                .my(px(4.))
                .mx(px(8.))
                .bg(hair)
                .into_any_element(),
            MenuRow::Item(item) => menu_row(ply, base + i, item, cx),
        })
        .collect()
}

fn menu_row(ply: &Ply, i: usize, item: &MenuItem, cx: &mut Context<Ply>) -> AnyElement {
    let p = ply.palette();
    let enabled = item.enabled;
    let has_kids = !item.children.is_empty();
    let action = item.action.clone();
    let danger = item.danger;
    let strong = item.strong;
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
        .when(strong, |el| el.font_weight(FontWeight::MEDIUM))
        .when(enabled, |el| el.hover(|s| s.bg(p.muted)))
        .map(|el| {
            if let Some(ico) = item.icon {
                el.child(icon(ico, px(14.), color))
            } else if strong {
                el.child(icon(Ico::Check, px(14.), color))
            } else {
                el.child(div().w(px(14.)))
            }
        })
        .child(div().child(item.label.clone()))
        .when(has_kids, |el| {
            el.child(icon(Ico::ChevronRight, px(12.), p.muted_foreground))
        })
        .when(strong && item.icon.is_some() && !has_kids, |el| {
            el.child(icon(Ico::Check, px(14.), color))
        })
        .when(enabled && has_kids, |el| {
            el.on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.set_flyout(Some(i), cx);
            }))
        })
        .when(enabled && !has_kids && action.is_some(), |el| {
            let action = action.clone().unwrap();
            el.on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.run(action.clone(), window, cx);
            }))
        })
        .into_any_element()
}

fn properties(ply: &Ply, cx: &mut Context<Ply>) -> Option<AnyElement> {
    let props = ply.properties.as_ref()?;
    let p = ply.palette();

    let rows = [
        ("Type", props.kind.clone()),
        ("Size", props.size.clone()),
        ("Modified", props.modified.clone()),
        ("Path", props.path.clone()),
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
                                .children(
                                    rows.into_iter()
                                        .map(|(label, value)| field(p, label.into(), value)),
                                )
                                .children(
                                    props.details.iter().map(|(label, value)| {
                                        field(p, label.clone(), value.clone())
                                    }),
                                ),
                        ),
                ),
        )
        .into_any_element(),
    )
}

fn field(p: crate::theme::Palette, label: gpui::SharedString, value: gpui::SharedString) -> gpui::Div {
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
}

/// A modal confirm step. Dismisses on Cancel, clicking the scrim, or Esc; the
/// confirming action runs only on the explicit Confirm button.
fn confirm_dialog(ply: &Ply, cx: &mut Context<Ply>) -> Option<AnyElement> {
    let dialog = ply.confirm.as_ref()?;
    let p = ply.palette();
    let confirm_color = if dialog.danger {
        p.destructive
    } else {
        p.foreground
    };
    let confirm_text = dialog.confirm_text.clone();
    let cancel_text = gpui::SharedString::from("Cancel");

    Some(
        deferred(
            div()
                .id("confirm-scrim")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(p.overlay)
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.cancel_confirm(cx)),
                )
                .child(
                    div()
                        .occlude()
                        .w(px(360.))
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
                                        .child(dialog.title.to_uppercase()),
                                ),
                        )
                        .child(
                            div()
                                .p(px(14.))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(p.foreground)
                                        .child(dialog.message.clone()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap(px(8.))
                                        .mt(px(16.))
                                        .child(confirm_button(
                                            p,
                                            cancel_text,
                                            p.foreground,
                                            cx.listener(|this, _, _, cx| this.cancel_confirm(cx)),
                                        ))
                                        .child(confirm_button(
                                            p,
                                            confirm_text,
                                            confirm_color,
                                            cx.listener(|this, _, _, cx| this.run_confirm(cx)),
                                        )),
                                ),
                        ),
                ),
        )
        .into_any_element(),
    )
}

/// A simple labelled action button (Ply hand-rolls controls rather than pulling
/// in a component lib), carrying a full-width click highlight.
fn confirm_button(
    p: crate::theme::Palette,
    label: gpui::SharedString,
    text: gpui::Hsla,
    click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(label.to_string())
        .flex()
        .items_center()
        .px(px(14.))
        .py(px(6.))
        .text_size(px(12.))
        .text_color(text)
        .cursor_default()
        .hover(|s| s.bg(p.muted))
        .child(label)
        .on_click(click)
}
