//! The window: title bar, sidebar, centre pane, and the layers above them.

mod browser;
mod home;
mod overlay;
mod sidebar;
mod status;
mod titlebar;

use std::sync::Arc;

use gpui::{
    Hsla, InteractiveElement, IntoElement, ObjectFit, ParentElement, Pixels, Render, RenderImage,
    StatefulInteractiveElement, Styled, StyledImage, Svg, Window, actions, div, img,
    prelude::FluentBuilder, px, svg,
};

use crate::app::{Location, Ply, ViewMode, dismiss_topmost};
use crate::icons::Ico;
use crate::theme;

actions!(
    ply,
    [
        ToggleTheme,
        GoBack,
        GoForward,
        GoUp,
        GoHome,
        Dismiss,
        Activate,
        BeginRename,
        DeleteSelection,
        SelectUp,
        SelectDown,
        SelectLeft,
        SelectRight,
        ExtendUp,
        ExtendDown,
        ExtendLeft,
        ExtendRight,
        Refresh,
        FocusFilter,
        CopySelectedPath,
    ]
);

/// A collision-resistant element id for a path, so rows keep their identity
/// across re-renders without allocating a string per frame.
pub fn stable_id(path: &std::path::Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

/// A lucide glyph. GPUI masks the SVG, so `color` fills the strokes.
pub fn icon(ico: Ico, size: Pixels, color: Hsla) -> Svg {
    svg()
        .path(ico.path())
        .size(size)
        .flex_none()
        .text_color(color)
}

/// A cached raster (thumbnail, shell icon) as a fixed-size image element.
pub(crate) fn thumb_img(thumb: &Arc<RenderImage>, size: f32) -> impl IntoElement {
    img(thumb.clone())
        .size(px(size))
        .rounded(px(2.))
        .object_fit(ObjectFit::Cover)
}

/// An invisible, fixed-size slot that reserves an icon's box while its real
/// shell raster resolves. Nothing is painted; the slot only keeps row/cell
/// geometry stable so the icons swap in without layout jumps.
pub(crate) fn icon_slot(size: f32) -> impl IntoElement {
    div().flex_none().size(px(size))
}

/// The small uppercase headings above each sidebar/home section.
///
/// The web build letter-spaces these; GPUI has no letter-spacing, so the
/// uppercasing and size carry the distinction on their own.
pub fn section_label(text: &'static str, color: Hsla) -> impl IntoElement {
    div()
        .px(px(12.))
        .pb(px(4.))
        .text_size(px(10.))
        .text_color(color)
        .child(text.to_uppercase())
}

/// Estimate the number of grid columns from the window width.
const CELL_W: f32 = 96.;
const CELL_GAP: f32 = 4.;
const SIDEBAR_W: f32 = 220.;
fn grid_cols(window: &Window) -> usize {
    let avail = f32::from(window.viewport_size().width) - SIDEBAR_W;
    ((avail / (CELL_W + CELL_GAP)).floor() as usize).max(1)
}

impl Render for Ply {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let editing = self.rename.is_some();
        self.sync_filter_placeholder(window, cx);

        div()
            .key_context("Ply")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .relative()
            .font(theme::ui_font())
            .text_size(px(13.))
            .bg(p.background)
            .text_color(p.foreground)
            .border_1()
            .border_color(p.border)
            .on_action(cx.listener(|this, _: &ToggleTheme, window, cx| {
                // Mirrors the web build's guard: a bare letter must not fire
                // while a text field has focus.
                if this.typing(window, cx) {
                    return;
                }
                this.toggle_mode(cx);
            }))
            .on_action(cx.listener(|this, _: &GoBack, window, cx| this.go_back(window, cx)))
            .on_action(cx.listener(|this, _: &GoForward, window, cx| this.go_forward(window, cx)))
            .on_action(cx.listener(|this, _: &GoUp, window, cx| this.go_up(window, cx)))
            .on_action(cx.listener(|this, _: &GoHome, window, cx| this.go_home(window, cx)))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| dismiss_topmost(this, cx)))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.reload(cx)))
            .on_action(cx.listener(|this, _: &Activate, window, cx| {
                if !this.typing(window, cx) {
                    this.activate_selection(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &BeginRename, window, cx| {
                if let Some(path) = this.selection.last().cloned() {
                    this.begin_rename(path, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteSelection, window, cx| {
                if !this.typing(window, cx) {
                    this.delete_selection(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SelectUp, window, cx| {
                if !this.typing(window, cx) {
                    if this.view == ViewMode::Grid {
                        let cols = grid_cols(window);
                        this.move_grid_selection(cols, 0, -1, false, cx);
                    } else {
                        this.move_selection(-1, false, cx);
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &SelectDown, window, cx| {
                if !this.typing(window, cx) {
                    if this.view == ViewMode::Grid {
                        let cols = grid_cols(window);
                        this.move_grid_selection(cols, 0, 1, false, cx);
                    } else {
                        this.move_selection(1, false, cx);
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, window, cx| {
                if !this.typing(window, cx) && this.view == ViewMode::Grid {
                    let cols = grid_cols(window);
                    this.move_grid_selection(cols, -1, 0, false, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SelectRight, window, cx| {
                if !this.typing(window, cx) && this.view == ViewMode::Grid {
                    let cols = grid_cols(window);
                    this.move_grid_selection(cols, 1, 0, false, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ExtendUp, window, cx| {
                if !this.typing(window, cx) {
                    if this.view == ViewMode::Grid {
                        let cols = grid_cols(window);
                        this.move_grid_selection(cols, 0, -1, true, cx);
                    } else {
                        this.move_selection(-1, true, cx);
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &ExtendDown, window, cx| {
                if !this.typing(window, cx) {
                    if this.view == ViewMode::Grid {
                        let cols = grid_cols(window);
                        this.move_grid_selection(cols, 0, 1, true, cx);
                    } else {
                        this.move_selection(1, true, cx);
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &ExtendLeft, window, cx| {
                if !this.typing(window, cx) && this.view == ViewMode::Grid {
                    let cols = grid_cols(window);
                    this.move_grid_selection(cols, -1, 0, true, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ExtendRight, window, cx| {
                if !this.typing(window, cx) && this.view == ViewMode::Grid {
                    let cols = grid_cols(window);
                    this.move_grid_selection(cols, 1, 0, true, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                if !this.is_home() {
                    this.filter.update(cx, |input, cx| input.focus(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &CopySelectedPath, window, cx| {
                if this.typing(window, cx) {
                    return;
                }
                if let Some(path) = this.selection.last().cloned() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                        path.to_string_lossy().into_owned(),
                    ));
                    this.note("Path copied.", cx);
                }
            }))
            .child(titlebar::render(self, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar::render(self, cx))
                    .child(
                        div()
                            .id("centre")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .when(!editing, |el| {
                                el.on_click(cx.listener(|this, _, _, cx| this.clear_selection(cx)))
                            })
                            .map(|el| match self.location {
                                Location::Home => el.child(home::render(self, cx)),
                                Location::Folder(_) => el
                                    .child(browser::render(self, cx))
                                    .child(status::render(self, cx)),
                            }),
                    ),
            )
            .children(overlay::render(self, cx))
    }
}
