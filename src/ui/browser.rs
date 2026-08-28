use std::ops::Range;
use std::path::PathBuf;

use gpui::AppContext;
use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Stateful, StatefulInteractiveElement, Styled, div,
    prelude::FluentBuilder, px, uniform_list,
};
use gpui_component::Sizable;
use gpui_component::input::Input;

use super::icon;
use super::sidebar::{DragLabel, PinDrag};
use crate::app::{LoadState, Ply, ViewMode};
use crate::icons::Ico;
use crate::listing::{
    Entry, SortKey, entry_icon, format_mtime, format_size, kind_label, truncate_middle,
};
use crate::theme::Palette;
use crate::thumbs;
use chrono::{DateTime, Local};

pub fn render(ply: &Ply, cx: &mut Context<Ply>) -> impl IntoElement {
    let list_view = ply.view == ViewMode::List;
    let count = ply.visible_len();

    // The list view is virtualized: `uniform_list` scrolls itself and only asks
    // for the rows it is about to paint, so it must not sit inside another
    // scroll area. Grid stays a plain wrapped flow for now.
    let body: AnyElement = match empty_message(ply, count) {
        Some(text) => scroll_area(list_view).child(text).into_any_element(),
        None if list_view => uniform_list(
            "entries",
            count,
            cx.processor(|this, range: Range<usize>, _window, cx| {
                let now = Local::now();
                let entries = this.visible();
                range
                    .filter_map(|ix| entries.get(ix).map(|e| list_row(this, e, ix, now, cx)))
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .w_full()
        .min_h_0()
        .into_any_element(),
        None => scroll_area(list_view)
            .child(
                div().flex().flex_wrap().gap(px(4.)).children(
                    ply.visible()
                        .iter()
                        .enumerate()
                        .map(|(ix, e)| grid_cell(ply, e, ix, cx)),
                ),
            )
            .into_any_element(),
    };

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.open_empty_menu(ev.position, cx);
            }),
        )
        .when(list_view, |el| el.child(list_headers(ply)))
        .child(body)
}

/// Fixed column widths shared by the header and every list row so cells line up.
const KIND_COL: f32 = 130.;
const SIZE_COL: f32 = 80.;
const MODIFIED_COL: f32 = 130.;

fn list_headers(ply: &Ply) -> impl IntoElement {
    let p = ply.palette();
    let key = ply.sort;
    let col = |label, w, active, right| {
        div()
            .w(px(w))
            .flex_none()
            .child(header_label(label, active, false, right, p))
    };
    div()
        .w_full()
        .flex()
        .items_center()
        .h(px(28.))
        .px(px(12.))
        .flex_none()
        .border_b_1()
        .border_color(p.border)
        .gap(px(16.))
        .text_size(px(11.))
        .text_color(p.muted_foreground)
        .child(header_label("Name", key == SortKey::Name, true, false, p))
        .child(col("Kind", KIND_COL, key == SortKey::Kind, false))
        .child(col("Size", SIZE_COL, key == SortKey::Size, true))
        .child(col(
            "Modified",
            MODIFIED_COL,
            key == SortKey::Modified,
            false,
        ))
}

fn header_label(
    text: &'static str,
    active: bool,
    grow: bool,
    right: bool,
    p: Palette,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .when(grow, |el| el.flex_1().min_w_0())
        .when(!grow, |el| el.w_full())
        .when(right, |el| el.justify_end())
        .when(active, |el| {
            el.font_weight(FontWeight::MEDIUM).text_color(p.foreground)
        })
        .child(text)
        .when(active, |el| {
            el.child(icon(Ico::ChevronDown, px(10.), p.muted_foreground))
        })
}

/// Scrolling container for everything that is not the virtualized list.
fn scroll_area(list_view: bool) -> Stateful<Div> {
    div()
        .id("entries")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .when(!list_view, |el| el.p(px(14.)))
}

/// The placeholder shown instead of rows, if there are no rows to show.
fn empty_message(ply: &Ply, count: usize) -> Option<AnyElement> {
    match &ply.listing {
        LoadState::Loading if count == 0 => Some(message("Loading…", ply)),
        LoadState::Failed(err) => Some(message(err.clone(), ply)),
        _ if count > 0 => None,
        _ if !ply.filter_text.is_empty() => Some(message("Nothing matches that filter.", ply)),
        _ => Some(message("This folder is empty.", ply)),
    }
}

fn message(text: impl Into<gpui::SharedString>, ply: &Ply) -> AnyElement {
    div()
        .p(px(24.))
        .text_size(px(12.5))
        .text_color(ply.palette().muted_foreground)
        .child(text.into())
        .into_any_element()
}

/// Shows a decoded media thumbnail when one is cached, a per-extension class
/// icon for plain files, otherwise the generic SVG icon. Triggers extraction
/// on demand for image/video/doc/audio entries.
fn icon_or_thumb(
    ply: &Ply,
    entry: &Entry,
    box_px: f32,
    icon_px: f32,
    cx: &mut Context<Ply>,
) -> AnyElement {
    let p = ply.palette();
    if entry.is_directory()
        && let Some(thumb) = thumbs::folder_icon(ply, entry, cx)
    {
        return super::thumb_img(&thumb, box_px).into_any_element();
    }
    if thumbs::wants_shell_icon(entry) {
        let key = thumbs::probe_key(ply, entry, cx);
        let cached = ply.thumb_cache().read(cx).get(&key);
        if cached.is_none() {
            thumbs::request_thumbnail(ply, entry, cx);
        }
        return match cached {
            Some(thumb) => super::thumb_img(&thumb, box_px).into_any_element(),
            None => icon(entry_icon(entry), px(icon_px), p.muted_foreground).into_any_element(),
        };
    }
    if thumbs::wants_class_icon(entry)
        && let Some(thumb) = thumbs::class_icon(ply, entry, cx)
    {
        return super::thumb_img(&thumb, box_px).into_any_element();
    }
    icon(entry_icon(entry), px(icon_px), p.muted_foreground).into_any_element()
}

fn list_row(
    ply: &Ply,
    entry: &Entry,
    ix: usize,
    now: DateTime<Local>,
    cx: &mut Context<Ply>,
) -> AnyElement {
    let p = ply.palette();
    let selected = ply.is_selected(&entry.path);
    let renaming = ply.rename.as_ref().is_some_and(|r| r.path == entry.path);
    let path = entry.path.clone();
    let is_dir = entry.is_directory();

    div()
        .id(("row", super::stable_id(&entry.path)))
        .w_full()
        .flex()
        .items_center()
        .h(px(29.))
        .px(px(12.))
        .gap(px(16.))
        .border_b_1()
        .border_color(p.border)
        .cursor_default()
        .when(selected, |el| {
            el.bg(p.select_strong).font_weight(FontWeight::MEDIUM)
        })
        .when(!selected, |el| el.hover(|s| s.bg(p.muted)))
        .child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .gap(px(8.))
                .min_w_0()
                .child(icon_or_thumb(ply, entry, 16., 14., cx))
                .map(|el| {
                    if renaming {
                        el.child(rename_field(ply, cx))
                    } else {
                        el.child(
                            div()
                                .truncate()
                                .text_size(px(12.5))
                                .child(entry.name.clone()),
                        )
                    }
                }),
        )
        .child(
            div()
                .w(px(KIND_COL))
                .flex_none()
                .truncate()
                .text_size(px(12.))
                .text_color(p.muted_foreground)
                .child(kind_label(entry)),
        )
        .child(
            div()
                .w(px(SIZE_COL))
                .flex_none()
                .text_right()
                .text_size(px(12.))
                .text_color(p.muted_foreground)
                .child(if is_dir {
                    "—".to_string()
                } else {
                    format_size(entry.size)
                }),
        )
        .child(
            div()
                .w(px(MODIFIED_COL))
                .flex_none()
                .truncate()
                .text_size(px(12.))
                .text_color(p.muted_foreground)
                .child(format_mtime(entry.modified, now)),
        )
        .when(!renaming, |el| {
            el.on_click(activate(ix, path.clone(), cx))
                .on_mouse_down(MouseButton::Right, context_menu(path.clone(), cx))
                .when(is_dir, |el| el.on_drag(PinDrag(path.clone()), drag_chip))
        })
        .into_any_element()
}

fn grid_cell(ply: &Ply, entry: &Entry, ix: usize, cx: &mut Context<Ply>) -> AnyElement {
    let p = ply.palette();
    let selected = ply.is_selected(&entry.path);
    let renaming = ply.rename.as_ref().is_some_and(|r| r.path == entry.path);
    let path = entry.path.clone();
    let is_dir = entry.is_directory();

    div()
        .id(("cell", super::stable_id(&entry.path)))
        .w(px(96.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(6.))
        .p(px(10.))
        .text_center()
        .cursor_default()
        .when(selected, |el| el.bg(p.select_strong))
        .when(!selected, |el| el.hover(|s| s.bg(p.muted)))
        .child(icon_or_thumb(ply, entry, 56., 56., cx))
        .map(|el| {
            if renaming {
                el.child(rename_field(ply, cx))
            } else {
                el.child(
                    div()
                        .w_full()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(11.))
                        .line_height(px(14.))
                        .child(truncate_middle(&entry.name, 12)),
                )
            }
        })
        .when(!renaming, |el| {
            el.on_click(activate(ix, path.clone(), cx))
                .on_mouse_down(MouseButton::Right, context_menu(path.clone(), cx))
                .when(is_dir, |el| el.on_drag(PinDrag(path.clone()), drag_chip))
        })
        .into_any_element()
}

/// Single click selects (honouring ctrl/shift); double click opens.
fn activate(
    ix: usize,
    path: PathBuf,
    cx: &mut Context<Ply>,
) -> impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static {
    cx.listener(move |this, ev: &ClickEvent, window, cx| {
        cx.stop_propagation();
        if ev.click_count() >= 2 {
            this.activate(&path, window, cx);
            return;
        }
        let m = ev.modifiers();
        this.click_row(ix, m.shift, m.secondary(), cx);
    })
}

fn context_menu(
    path: PathBuf,
    cx: &mut Context<Ply>,
) -> impl Fn(&MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static {
    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
        cx.stop_propagation();
        this.open_menu(ev.position, path.clone(), cx);
    })
}

fn drag_chip(
    drag: &PinDrag,
    _: gpui::Point<gpui::Pixels>,
    _: &mut gpui::Window,
    cx: &mut gpui::App,
) -> gpui::Entity<DragLabel> {
    let name = drag
        .0
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    cx.new(|_| DragLabel(name.into()))
}

/// Escape is handled here rather than as a global action: the focused input
/// claims the key first, so the cancel has to sit on the way out.
fn rename_field(ply: &Ply, cx: &mut Context<Ply>) -> AnyElement {
    let rename = ply.rename.as_ref().expect("called only while renaming");
    div()
        .w_full()
        .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _, cx| {
            if ev.keystroke.key == "escape" {
                cx.stop_propagation();
                this.cancel_rename(cx);
            }
        }))
        .child(Input::new(&rename.input).xsmall())
        .into_any_element()
}
