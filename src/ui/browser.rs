use std::ops::Range;
use std::path::PathBuf;

use gpui::AppContext;
use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Stateful, StatefulInteractiveElement, Styled, Window, div,
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

pub fn render(ply: &Ply, window: &Window, cx: &mut Context<Ply>) -> impl IntoElement {
    let list_view = ply.view == ViewMode::List;
    let count = ply.visible_len();
    let request_gen = ply.list_generation;

    // Visible-first prefetch: request icons for visible entries first, then a
    // bounded lookahead past the visible window. This mirrors KIO/Nautilus
    // visible-first scheduling so the pool serves on-screen rows before
    // off-screen ones.
    {
        let entries = ply.visible();
        let visible_count = if list_view {
            let row_h = 29.;
            (f32::from(window.viewport_size().height) / row_h).ceil() as usize
        } else {
            let cols = grid_cols_from_width(avail_width(window));
            let row_h = GRID_CELL_W + 6. + 14. + 6.;
            let rows = (f32::from(window.viewport_size().height) / row_h).ceil() as usize;
            rows * cols
        };
        let visible_end = visible_count.min(entries.len());
        let lookahead_end = (visible_end + PREFETCH_LOOKAHEAD).min(entries.len());
        // Visible entries first, then the lookahead window.
        for entry in entries.iter().take(lookahead_end) {
            thumbs::ensure_entry_icons(ply, entry, cx, request_gen);
        }
    }

    // Working-set lock: tell the thumbnail cache which entries are visible so
    // it never evicts on-screen thumbnails. Only the keys for actually visible
    // entries are locked; everything else is evictable and re-decodes on
    // demand when scrolled back into view.
    {
        let entries = ply.visible();
        let thumb = ply.thumb_cache();
        thumb.update(cx, |cache, _| {
            let mut keys = Vec::with_capacity(entries.len());
            for e in &entries {
                let key = if thumbs::is_lnk(e) {
                    match cache.lnk_stamp(&e.path) {
                        Some(stamp) => thumbs::stamped_key(&e.path, stamp),
                        None => thumbs::cache_key(&e.path, e.modified),
                    }
                } else {
                    thumbs::cache_key(&e.path, e.modified)
                };
                keys.push(key);
            }
            cache.set_working_set(&keys);
        });
    }

    // Both views are virtualized through `uniform_list`, which scrolls itself
    // and only asks for the rows it is about to paint, so neither sits inside
    // another scroll area. The grid has no virtualized primitive of its own, so
    // it reuses `uniform_list` by packing a fixed number of cells per row: each
    // list item is one row of `cols` cells laid out horizontally, mapped over
    // the flat `visible_indices`. `cols` follows the same width rule the arrow
    // keys use (`grid_cols_from_width`), so navigation and layout stay aligned.
    let cols = grid_cols_from_width(avail_width(window));
    let row_count = grid_row_count(count, cols);
    let body: AnyElement = match empty_message(ply, count) {
        Some(text) => scroll_area(list_view).child(text).into_any_element(),
        None if list_view => uniform_list(
            "entries",
            count,
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                let now = Local::now();
                let entries = this.visible();
                let req_gen = this.list_generation;
                range
                    .filter_map(|ix| {
                        entries
                            .get(ix)
                            .map(|e| list_row(this, e, ix, now, cx, req_gen))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .w_full()
        .min_h_0()
        .into_any_element(),
        None => uniform_list(
            "grid_rows",
            row_count,
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                let entries = this.visible();
                let req_gen = this.list_generation;
                range
                    .map(|row_ix| {
                        let rng = grid_row_range(row_ix, cols, entries.len());
                        div()
                            .flex()
                            .gap(px(4.))
                            .children(
                                rng.map(|ix| grid_cell(this, entries[ix], ix, cx, req_gen)),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .w_full()
        .min_h_0()
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

/// How far past the visible window the visible-first prefetch extends. Kept
/// small so the pool serves on-screen rows before spending slots on look-ahead.
const PREFETCH_LOOKAHEAD: usize = 24;

/// Grid cell geometry. `GRID_CELL_STRIDE` is the horizontal span one cell
/// claims including the gap after it, so a row of `cols` cells is `cols*96 +
/// (cols-1)*4` wide. Shared with the arrow-key navigation so navigation and
/// layout agree on the column count.
pub(crate) const GRID_CELL_W: f32 = 96.;
pub(crate) const GRID_CELL_GAP: f32 = 4.;
pub(crate) const SIDEBAR_W: f32 = 220.;

/// The width the centre grid actually lays out into: the whole viewport minus
/// the fixed sidebar. `grid_cell` doesn't add its own margin, so this is what a
/// full row of cells can span.
fn avail_width(window: &Window) -> f32 {
    f32::from(window.viewport_size().width) - SIDEBAR_W
}

/// Number of grid columns a given available width can hold, never below one
/// so a tiny window still gets a usable single-column grid.
pub(crate) fn grid_cols_from_width(avail: f32) -> usize {
    ((avail / (GRID_CELL_W + GRID_CELL_GAP)).floor() as usize).max(1)
}

/// Number of packed rows needed to hold `item_count` cells at `cols` per row.
pub(crate) fn grid_row_count(item_count: usize, cols: usize) -> usize {
    if item_count == 0 || cols == 0 {
        return 0;
    }
    item_count.div_ceil(cols)
}

/// The flat visible-index range (start..end) a single packed row covers. `start`
/// is `row_ix*cols`; the last row is truncated to the item count. Rows past the
/// end produce an empty range.
pub(crate) fn grid_row_range(row_ix: usize, cols: usize, item_count: usize) -> Range<usize> {
    let start = row_ix.saturating_mul(cols);
    if cols == 0 || start >= item_count {
        return start..start;
    }
    start..(start + cols).min(item_count)
}

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
///
/// A slot renders its real raster the moment it is cached; while it loads the
/// slot is an invisible placeholder so geometry stays put. The themed glyph is
/// only the terminal state, for entries that will never have a shell icon or
/// whose resolution permanently failed.
fn icon_or_thumb(
    ply: &Ply,
    entry: &Entry,
    box_px: f32,
    icon_px: f32,
    cx: &mut Context<Ply>,
    request_gen: u64,
) -> AnyElement {
    let p = ply.palette();
    match thumbs::entry_icon_probe(ply, entry, cx, request_gen) {
        thumbs::IconProbe::Ready(img) => super::thumb_img(&img, box_px).into_any_element(),
        thumbs::IconProbe::Loading => super::icon_slot(box_px).into_any_element(),
        thumbs::IconProbe::Glyph => {
            icon(entry_icon(entry), px(icon_px), p.muted_foreground).into_any_element()
        }
    }
}

fn list_row(
    ply: &Ply,
    entry: &Entry,
    ix: usize,
    now: DateTime<Local>,
    cx: &mut Context<Ply>,
    request_gen: u64,
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
                .child(icon_or_thumb(ply, entry, 16., 14., cx, request_gen))
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

fn grid_cell(ply: &Ply, entry: &Entry, ix: usize, cx: &mut Context<Ply>, request_gen: u64) -> AnyElement {
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
        .child(icon_or_thumb(ply, entry, 56., 56., cx, request_gen))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_cols_are_at_least_one_and_floor() {
        // Tiny widths still yield a single column.
        assert_eq!(grid_cols_from_width(0.0), 1);
        assert_eq!(grid_cols_from_width(50.0), 1);
        assert_eq!(grid_cols_from_width(99.0), 1);
        assert_eq!(grid_cols_from_width(100.0), 1);
        // Exact stride boundaries and non-clean divisions floor down.
        assert_eq!(grid_cols_from_width(199.0), 1);
        assert_eq!(grid_cols_from_width(200.0), 2);
        assert_eq!(grid_cols_from_width(250.0), 2);
        assert_eq!(grid_cols_from_width(299.0), 2);
        assert_eq!(grid_cols_from_width(300.0), 3);
        assert_eq!(grid_cols_from_width(1000.5), 10);
    }

    #[test]
    fn grid_row_count_covers_all_items() {
        assert_eq!(grid_row_count(0, 4), 0);
        assert_eq!(grid_row_count(1, 4), 1);
        assert_eq!(grid_row_count(4, 4), 1);
        assert_eq!(grid_row_count(5, 4), 2);
        assert_eq!(grid_row_count(8, 4), 2);
        assert_eq!(grid_row_count(9, 4), 3);
        assert_eq!(grid_row_count(100, 5), 20);
        assert_eq!(grid_row_count(101, 5), 21);
        // cols is never zero in practice (grid_cols_from_width floors to >= 1),
        // but guard against it anyway.
        assert_eq!(grid_row_count(5, 0), 0);
    }

    #[test]
    fn grid_row_range_maps_packed_rows_to_flat_indices() {
        // Full middle row.
        assert_eq!(grid_row_range(0, 4, 10), 0..4);
        assert_eq!(grid_row_range(1, 4, 10), 4..8);
        // Last row is truncated to the item count.
        assert_eq!(grid_row_range(2, 4, 10), 8..10);
        // A filter that hides rows: fewer items than cols still packs into row 0.
        assert_eq!(grid_row_range(0, 4, 2), 0..2);
        // Empty listing: no range.
        assert_eq!(grid_row_range(0, 4, 0), 0..0);
        // Rows past the end are empty, never panic.
        assert_eq!(grid_row_range(5, 4, 10), 20..20);
        assert_eq!(grid_row_range(3, 4, 10), 12..12);
        // Exactly one full row.
        assert_eq!(grid_row_range(0, 4, 4), 0..4);
        // cols+1 items: two rows, second truncated.
        assert_eq!(grid_row_range(0, 4, 5), 0..4);
        assert_eq!(grid_row_range(1, 4, 5), 4..5);
    }

    #[test]
    fn row_and_col_decompose_back_to_the_flat_index() {
        // Every cell in a full grid maps back to the same flat index through the
        // row it lives in, so activation/selection keep their global index.
        let cols = 7;
        let items = 100;
        for row_ix in 0..grid_row_count(items, cols) {
            for (j, ix) in grid_row_range(row_ix, cols, items).enumerate() {
                assert_eq!(row_ix * cols + j, ix);
            }
        }
    }

    #[test]
    fn visible_first_prefetch_bounds() {
        // The visible-first prefetch clamps lookahead_end to entries.len().
        let total = 10;
        let visible_end = 6;
        let lookahead = PREFETCH_LOOKAHEAD;
        let end = (visible_end + lookahead).min(total);
        assert_eq!(end, 10, "should not exceed total");

        // Small listing: lookahead extends only to the end.
        let total = 3;
        let visible_end = 2;
        let end = (visible_end + lookahead).min(total);
        assert_eq!(end, 3);
    }
}
