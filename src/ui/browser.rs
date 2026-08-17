use std::path::PathBuf;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, StatefulInteractiveElement, Styled, div,
    prelude::FluentBuilder, px,
};
use gpui::AppContext;
use gpui_component::Sizable;
use gpui_component::input::Input;

use super::icon;
use super::sidebar::{DragLabel, PinDrag};
use crate::app::{LoadState, Ply, ViewMode};
use crate::icons::Ico;
use crate::listing::{Entry, format_mtime, format_size, kind_label};

/// The icon for an entry, chosen from its kind the way the design does.
pub fn entry_icon(entry: &Entry) -> Ico {
    if entry.is_directory() {
        return Ico::Folder;
    }
    match kind_label(entry) {
        k if k.contains("Image") || k == "Icon" => Ico::Image,
        k if k.contains("Video") => Ico::Video,
        k if k.contains("Audio") || k == "Playlist" => Ico::Music,
        k if k.contains("Document") || k == "Source File" || k.contains("Workbook") => Ico::FileText,
        _ => Ico::File,
    }
}

pub fn render(ply: &Ply, cx: &mut Context<Ply>) -> impl IntoElement {
    let p = ply.palette();
    let list_view = ply.view == ViewMode::List;
    let entries = ply.visible();

    let body: Vec<AnyElement> = match &ply.listing {
        LoadState::Loading if entries.is_empty() => vec![message("Loading…", ply)],
        LoadState::Failed(err) => vec![message(err.clone(), ply)],
        _ if entries.is_empty() && !ply.filter_text.is_empty() => {
            vec![message("Nothing matches that filter.", ply)]
        }
        _ if entries.is_empty() => vec![message("This folder is empty.", ply)],
        _ if list_view => entries
            .iter()
            .enumerate()
            .map(|(ix, e)| list_row(ply, e, ix, cx))
            .collect(),
        _ => vec![
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .children(
                    entries
                        .iter()
                        .enumerate()
                        .map(|(ix, e)| grid_cell(ply, e, ix, cx)),
                )
                .into_any_element(),
        ],
    };

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .when(list_view, |el| {
            el.child(
                div()
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
                    .child(div().flex_1().min_w_0().child("Name"))
                    .child(div().w(px(130.)).flex_none().child("Kind"))
                    .child(div().w(px(80.)).flex_none().text_right().child("Size"))
                    .child(div().w(px(130.)).flex_none().child("Modified")),
            )
        })
        .child(
            div()
                .id("entries")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .when(!list_view, |el| el.p(px(14.)))
                .children(body),
        )
}

fn message(text: impl Into<gpui::SharedString>, ply: &Ply) -> AnyElement {
    div()
        .p(px(24.))
        .text_size(px(12.5))
        .text_color(ply.palette().muted_foreground)
        .child(text.into())
        .into_any_element()
}

fn list_row(ply: &Ply, entry: &Entry, ix: usize, cx: &mut Context<Ply>) -> AnyElement {
    let p = ply.palette();
    let selected = ply.is_selected(&entry.path);
    let renaming = ply.rename.as_ref().is_some_and(|r| r.path == entry.path);
    let path = entry.path.clone();
    let is_dir = entry.is_directory();

    div()
        .id(("row", super::stable_id(&entry.path)))
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
                .child(icon(entry_icon(entry), px(14.), p.muted_foreground))
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
                .w(px(130.))
                .flex_none()
                .truncate()
                .text_size(px(12.))
                .text_color(p.muted_foreground)
                .child(kind_label(entry)),
        )
        .child(
            div()
                .w(px(80.))
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
                .w(px(130.))
                .flex_none()
                .truncate()
                .text_size(px(12.))
                .text_color(p.muted_foreground)
                .child(format_mtime(entry.modified)),
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
        .child(icon(entry_icon(entry), px(28.), p.muted_foreground))
        .map(|el| {
            if renaming {
                el.child(rename_field(ply, cx))
            } else {
                el.child(
                    div()
                        .w_full()
                        .text_size(px(11.))
                        .line_height(px(14.))
                        .child(entry.name.clone()),
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
