//! The window: title bar, sidebar, centre pane, and the layers above them.

mod browser;
mod home;
mod overlay;
mod sidebar;
mod status;
mod titlebar;

pub(crate) use status::filter_placeholder;

use std::sync::Arc;

use gpui::{
    Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Hsla, InteractiveElement,
    IntoElement, ObjectFit, ParentElement, Pixels, Render, RenderImage, StatefulInteractiveElement,
    Styled, StyledImage, Svg, Window, actions, div, img, prelude::FluentBuilder, px, svg,
};

use crate::app::{Location, MenuRow, Ply, ViewMode, dismiss_topmost};
use crate::icons::Ico;
use crate::theme;
use crate::{MenuActivate, MenuDown, MenuLeft, MenuRight, MenuUp};

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

/// A Segoe Fluent Icons codepoint at the given px size, tinted like lucide.
/// Font chain is Fluent -> MDL2 (Win10) -> UI Symbol; off Windows the chain
/// simply misses and the caller keeps its lucide fallback, so this never
/// replaces the lucide path, only supplements it where `MenuItem.glyph` lands.
pub(crate) fn glyph_icon(codepoint: char, size: f32, color: Hsla) -> impl IntoElement {
    div()
        .flex_none()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .font(Font {
            family: "Segoe Fluent Icons".into(),
            features: FontFeatures::default(),
            fallbacks: Some(FontFallbacks::from_fonts(vec![
                "Segoe MDL2 Assets".to_string(),
                "Segoe UI Symbol".to_string(),
            ])),
            weight: FontWeight::default(),
            style: FontStyle::default(),
        })
        .text_size(px(size))
        .text_color(color)
        .child(codepoint.to_string())
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

/// Estimate the number of grid columns from the window width. The centre pane
/// spans the viewport minus the fixed sidebar, so row width is `viewport -
/// SIDEBAR_W`; `grid_cols_from_width` turns that into columns. Shared with the
/// virtualized grid layout in `browser.rs` so navigation and rendering agree.
fn grid_cols(window: &Window) -> usize {
    browser::grid_cols_from_width(f32::from(window.viewport_size().width) - browser::SIDEBAR_W)
}

impl Render for Ply {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let editing = self.rename.is_some();
        self.sync_filter_placeholder(window, cx);

        // Paint-storm detector (see `Ply::thumb_storm`): fast viewport
        // travel means a scroll fling is in flight.
        self.note_paint();

        // Storm-settle repaint: a fling's last frames may all be slots, and
        // with nothing left pending no further render would ever repaint the
        // settled viewport — the screen would freeze on placeholders. One
        // debounced timer per storm guarantees the follow-up paint that
        // shows the thumbs. It re-arms while still flinging.
        if self.thumb_storm && !self.storm_settle_pending {
            self.storm_settle_pending = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(300))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.storm_settle_pending = false;
                    cx.notify();
                });
            })
            .detach();
        }

        // Free GPU textures for thumbnails that have left the bounded cache.
        // GPUI's own window atlas never evicts, so without this every image
        // ever painted keeps a tile in GPU memory forever.
        let dropped = self
            .thumb_cache()
            .update(cx, |cache, _| cache.drain_drops());
        for image in dropped {
            window.drop_image(image).ok();
        }

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
            .on_action(cx.listener(|this, _: &MenuUp, window, cx| {
                if this.menu.is_none() || this.typing(window, cx) {
                    return;
                }
                if let Some(menu) = this.menu.as_mut() {
                    menu.move_selection(-1);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &MenuDown, window, cx| {
                if this.menu.is_none() || this.typing(window, cx) {
                    return;
                }
                if let Some(menu) = this.menu.as_mut() {
                    menu.move_selection(1);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &MenuLeft, window, cx| {
                if this.menu.is_none() || this.typing(window, cx) {
                    return;
                }
                let flying = this.menu.as_ref().is_some_and(|m| m.flyout.is_some());
                if flying {
                    this.set_flyout(None, cx);
                } else {
                    dismiss_topmost(this, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &MenuRight, window, cx| {
                if this.menu.is_none() || this.typing(window, cx) {
                    return;
                }
                let open = this
                    .menu
                    .as_ref()
                    .and_then(|m| m.selected)
                    .and_then(|i| this.menu.as_ref().and_then(|m| m.rows.get(i)))
                    .is_some_and(
                        |row| matches!(row, MenuRow::Item(item) if !item.children.is_empty()),
                    );
                if open {
                    let ix = this.menu.as_ref().and_then(|m| m.selected);
                    this.set_flyout(ix, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &MenuActivate, window, cx| {
                if this.menu.is_none() || this.typing(window, cx) {
                    return;
                }
                let pick = this
                    .menu
                    .as_ref()
                    .and_then(|m| m.selected)
                    .and_then(|i| this.menu.as_ref().and_then(|m| m.rows.get(i)))
                    .and_then(|row| match row {
                        MenuRow::Item(item) => {
                            Some((!item.children.is_empty(), item.action.clone()))
                        }
                        MenuRow::Separator => None,
                    });
                match pick {
                    Some((true, _)) => {
                        let ix = this.menu.as_ref().and_then(|m| m.selected);
                        this.set_flyout(ix, cx);
                    }
                    Some((false, Some(action))) => this.run(action, window, cx),
                    _ => {}
                }
            }))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.reload(cx)))
            .on_action(cx.listener(|this, _: &Activate, window, cx| {
                if this.menu.is_some() {
                    return;
                }
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
                if this.menu.is_some() {
                    return;
                }
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
                if this.menu.is_some() {
                    return;
                }
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
                if this.menu.is_some() {
                    return;
                }
                if !this.typing(window, cx) && this.view == ViewMode::Grid {
                    let cols = grid_cols(window);
                    this.move_grid_selection(cols, -1, 0, false, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SelectRight, window, cx| {
                if this.menu.is_some() {
                    return;
                }
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
                                    .child(browser::render(self, window, cx))
                                    .child(status::render(self, cx)),
                            }),
                    ),
            )
            .children(overlay::render(self, cx))
    }
}
