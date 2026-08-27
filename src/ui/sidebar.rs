use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AppContext, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, div,
    prelude::FluentBuilder, px,
};

use super::{icon, section_label};
use crate::app::Ply;
use crate::icons::Ico;
use crate::volumes;

/// A folder being dragged onto Home to pin it.
#[derive(Clone)]
pub struct PinDrag(pub PathBuf);

pub fn render(ply: &Ply, cx: &mut Context<Ply>) -> impl IntoElement {
    let p = ply.palette();
    let (drives, devices) = volumes::partition_drives_devices(&ply.volumes);

    let mut pinned = Vec::new();
    for path in ply.quick_access.clone() {
        let label = ply.display_name(&path);
        push_branch(ply, &mut pinned, path, label, Ico::Folder, 0, cx);
    }

    let mut drive_rows = Vec::new();
    for v in drives {
        push_branch(
            ply,
            &mut drive_rows,
            v.path.clone(),
            v.name.clone().into(),
            v.ico(),
            0,
            cx,
        );
    }

    let mut device_rows = Vec::new();
    for v in devices {
        push_branch(
            ply,
            &mut device_rows,
            v.path.clone(),
            v.name.clone().into(),
            v.ico(),
            0,
            cx,
        );
    }

    div()
        .id("sidebar")
        .w(px(220.))
        .flex_none()
        .py(px(10.))
        .bg(p.sidebar)
        .border_r_1()
        .border_color(p.sidebar_border)
        .overflow_y_scroll()
        .child(section_label("Home", p.muted_foreground))
        .child(
            div()
                .id("home-row")
                .flex()
                .items_center()
                .gap(px(6.))
                .py(px(5.))
                .pl(px(28.))
                .pr(px(10.))
                .text_size(px(12.5))
                .cursor_default()
                .when(ply.is_home(), |el| {
                    el.bg(p.accent)
                        .text_color(p.foreground)
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!ply.is_home(), |el| {
                    el.text_color(p.muted_foreground).hover(|s| s.bg(p.muted))
                })
                .child(icon(Ico::Home, px(14.), p.muted_foreground))
                .child("Home")
                .on_click(cx.listener(|this, _, window, cx| this.go_home(window, cx))),
        )
        .child(
            div()
                .id("pinned")
                .min_h(px(6.))
                .drag_over::<PinDrag>(move |s, _, _, _| s.bg(p.muted))
                .on_drop(cx.listener(|this, drag: &PinDrag, _, cx| {
                    this.pin(drag.0.clone(), cx);
                }))
                .when(pinned.is_empty(), |el| {
                    el.child(
                        div()
                            .px(px(12.))
                            .py(px(8.))
                            .text_size(px(11.))
                            .text_color(p.muted_foreground)
                            .child("Drag a folder here to pin it"),
                    )
                })
                .children(pinned),
        )
        .child(div().h(px(12.)))
        .child(section_label("This PC", p.muted_foreground))
        .when(cfg!(windows), |el| el.child(recycle_bin_row(ply, cx)))
        .children(drive_rows)
        .child(div().h(px(12.)))
        .child(section_label("Devices & network", p.muted_foreground))
        .when(device_rows.is_empty(), |el| {
            el.child(
                div()
                    .px(px(12.))
                    .py(px(4.))
                    .text_size(px(11.))
                    .text_color(p.muted_foreground)
                    .child("Nothing connected"),
            )
        })
        .children(device_rows)
}

/// Emit a row, then its children if the user opened it.
///
/// Navigation deliberately never expands a branch: the tree only opens when the
/// chevron is clicked, so browsing in the centre pane leaves it undisturbed.
fn push_branch(
    ply: &Ply,
    out: &mut Vec<AnyElement>,
    path: PathBuf,
    label: SharedString,
    ico: Ico,
    depth: usize,
    cx: &mut Context<Ply>,
) {
    out.push(row(ply, &path, label, ico, depth, cx));
    if !ply.is_expanded(&path) {
        return;
    }
    for child in ply.child_folders(&path).to_vec() {
        let label = ply.display_name(&child);
        push_branch(ply, out, child, label, Ico::Folder, depth + 1, cx);
    }
}

fn row(
    ply: &Ply,
    path: &Path,
    label: SharedString,
    ico: Ico,
    depth: usize,
    cx: &mut Context<Ply>,
) -> AnyElement {
    let p = ply.palette();
    // Quiet highlight only; the tree is never scrolled or opened to reveal this.
    let active = ply.current_folder() == Some(path);
    let expanded = ply.is_expanded(path);
    let id = super::stable_id(path);

    div()
        .id(("side", id))
        .flex()
        .items_center()
        .gap(px(6.))
        .py(px(5.))
        .pr(px(10.))
        .pl(px(10. + depth as f32 * 16.))
        .text_size(px(12.5))
        .cursor_default()
        .when(active, |el| {
            el.bg(p.accent)
                .text_color(p.foreground)
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |el| {
            el.text_color(p.muted_foreground).hover(|s| s.bg(p.muted))
        })
        .child(
            div()
                .id(("chev", id))
                .w(px(12.))
                .flex()
                .flex_none()
                .justify_center()
                .child(icon(
                    if expanded {
                        Ico::ChevronDown
                    } else {
                        Ico::ChevronRight
                    },
                    px(11.),
                    p.muted_foreground,
                ))
                .on_click({
                    let path = path.to_path_buf();
                    cx.listener(move |this, _, _, cx| {
                        this.toggle_expanded(path.clone(), cx);
                    })
                }),
        )
        .child(icon(ico, px(14.), p.muted_foreground))
        .child(div().truncate().child(label))
        .on_click({
            let path = path.to_path_buf();
            cx.listener(move |this, _, window, cx| {
                this.open_folder(path.clone(), window, cx);
            })
        })
        .on_mouse_down(MouseButton::Right, {
            let path = path.to_path_buf();
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                this.open_menu(ev.position, path.clone(), cx);
            })
        })
        .on_drag(PinDrag(path.to_path_buf()), |drag, _, _, cx| {
            let name = drag
                .0
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            cx.new(|_| DragLabel(name.into()))
        })
        .into_any_element()
}

/// The static Recycle Bin row: a leaf (no chevron) that navigates to the
/// synthetic recycle-bin location on click. No right-click menu, since the
/// shell items there carry no filesystem capabilities.
fn recycle_bin_row(ply: &Ply, cx: &mut Context<Ply>) -> AnyElement {
    let p = ply.palette();
    let active = ply
        .current_folder()
        .is_some_and(crate::recycle_bin::is_recycle_bin);
    div()
        .id("recycle-bin-row")
        .flex()
        .items_center()
        .gap(px(6.))
        .py(px(5.))
        .pl(px(28.))
        .pr(px(10.))
        .text_size(px(12.5))
        .cursor_default()
        .when(active, |el| {
            el.bg(p.accent)
                .text_color(p.foreground)
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |el| {
            el.text_color(p.muted_foreground).hover(|s| s.bg(p.muted))
        })
        .child(icon(Ico::Trash, px(14.), p.muted_foreground))
        .child("Recycle Bin")
        .on_click(cx.listener(|this, _, window, cx| {
            this.open_folder(crate::recycle_bin::root(), window, cx);
        }))
        .into_any_element()
}

/// The chip that follows the cursor while dragging a folder.
pub struct DragLabel(pub SharedString);
impl gpui::Render for DragLabel {
    fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .text_size(px(12.))
            .bg(gpui::black().opacity(0.75))
            .text_color(gpui::white())
            .child(self.0.clone())
    }
}
