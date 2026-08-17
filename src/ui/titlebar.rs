use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, WindowControlArea, div, prelude::FluentBuilder, px,
};

use super::icon;
use crate::app::Ply;
use crate::icons::Ico;

/// Title bar: identity, history, breadcrumbs, theme, window controls.
///
/// The bar registers itself as the window's drag area and the three buttons on
/// the right as the caption controls, so Windows performs the move, snap,
/// maximize/restore and close itself. Every clickable child calls `occlude`:
/// GPUI's hit test walks topmost-first and stops at the first occluding hitbox,
/// which keeps the drag area out of the result so a press on a control is a
/// click rather than the start of a window drag.
pub fn render(ply: &Ply, cx: &mut Context<Ply>) -> impl IntoElement {
    let p = ply.palette();
    let is_home = ply.is_home();
    let crumbs = ply.crumbs();
    let last = crumbs.len().saturating_sub(1);

    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(38.))
        .px(px(12.))
        .flex_none()
        .border_b_1()
        .border_color(p.border)
        .window_control_area(WindowControlArea::Drag)
        .child(
            div()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.muted_foreground)
                .mr(px(4.))
                .child("Ply"),
        )
        .child(
            nav_button("back", Ico::ArrowLeft, ply.can_go_back(), p, cx).on_click(cx.listener(
                |this, _, window, cx| {
                    this.go_back(window, cx);
                },
            )),
        )
        .child(
            nav_button("forward", Ico::ArrowRight, ply.can_go_forward(), p, cx).on_click(
                cx.listener(|this, _, window, cx| {
                    this.go_forward(window, cx);
                }),
            ),
        )
        .child(
            div()
                .id("home")
                .flex()
                .occlude()
                .cursor_default()
                .child(icon(
                    Ico::Home,
                    px(14.),
                    if is_home {
                        p.foreground
                    } else {
                        p.muted_foreground
                    },
                ))
                .on_click(cx.listener(|this, _, window, cx| this.go_home(window, cx))),
        )
        .child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .gap(px(6.))
                .ml(px(6.))
                .min_w_0()
                .overflow_hidden()
                .text_size(px(12.5))
                .whitespace_nowrap()
                .text_color(p.muted_foreground)
                .when(is_home, |el| {
                    el.child(div().text_color(p.foreground).child("Home"))
                })
                .children(crumbs.into_iter().enumerate().map(|(i, (name, path))| {
                    let current = i == last;
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .flex_none()
                        .when(i > 0, |el| {
                            el.child(icon(Ico::ChevronRight, px(12.), p.muted_foreground))
                        })
                        .child(
                            div()
                                .id(("crumb", i))
                                .occlude()
                                .cursor_default()
                                .when(current, |el| {
                                    el.text_color(p.foreground)
                                        .font_weight(FontWeight::MEDIUM)
                                })
                                .hover(|s| s.text_color(p.foreground))
                                .child(name)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_folder(path.clone(), window, cx)
                                })),
                        )
                })),
        )
        .child(
            div()
                .id("theme")
                .flex()
                .occlude()
                .mr(px(6.))
                .cursor_default()
                .child(icon(
                    match ply.mode {
                        crate::theme::Mode::Dark => Ico::Moon,
                        crate::theme::Mode::Light => Ico::Sun,
                    },
                    px(13.),
                    p.muted_foreground,
                ))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_mode(cx))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .text_color(p.muted_foreground)
                .child(caption_button(
                    "minimize",
                    Ico::Minus,
                    px(13.),
                    WindowControlArea::Min,
                    p.foreground,
                    p,
                ))
                .child(caption_button(
                    "maximize",
                    Ico::Square,
                    px(11.),
                    WindowControlArea::Max,
                    p.foreground,
                    p,
                ))
                .child(caption_button(
                    "close",
                    Ico::X,
                    px(14.),
                    WindowControlArea::Close,
                    p.destructive,
                    p,
                )),
        )
}

/// Minimize / maximize / close. These carry no click handler: the area is
/// handed to the window manager, which runs the real action on mouse up and so
/// toggles maximize and restore correctly.
fn caption_button(
    id: &'static str,
    ico: Ico,
    size: gpui::Pixels,
    area: WindowControlArea,
    hover: gpui::Hsla,
    p: crate::theme::Palette,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .occlude()
        .cursor_default()
        .window_control_area(area)
        .hover(|s| s.text_color(hover))
        .child(icon(ico, size, p.muted_foreground))
}

/// Back / forward. Disabled ends dim out rather than disappearing.
fn nav_button(
    id: &'static str,
    ico: Ico,
    enabled: bool,
    p: crate::theme::Palette,
    _cx: &mut Context<Ply>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .occlude()
        .cursor_default()
        .when(!enabled, |el| el.opacity(0.4))
        .child(icon(
            ico,
            px(15.),
            if enabled {
                p.foreground
            } else {
                p.muted_foreground
            },
        ))
}
