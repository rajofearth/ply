use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, anchored, deferred, div, prelude::FluentBuilder, px,
};

use super::{glyph_icon, icon, icon_slot, thumb_img};
use crate::app::{LoadState, MenuAction, MenuIconSource, MenuItem, MenuRow, MenuStock, Ply};
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
    let flyout = menu.flyout.and_then(|i| match menu.rows.get(i) {
        Some(MenuRow::Item(item)) if !item.children.is_empty() => Some(i),
        _ => None,
    });
    let selected = menu.selected;

    // The app builders always pass an empty toolbar (grep `show_menu` at
    // `src/app/ops.rs`): every row lives in the list. The old lucide-only
    // toolbar strip is deleted; this panel paints rows only.
    let panel = chrome(p).children(paint_rows(ply, &menu.rows, 0, true, selected, cx));

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
        let y_off = flyout_y_offset(&menu.rows, ix);
        let at = gpui::Point::new(menu.at.x + px(MENU_FLYOUT_X), menu.at.y + px(y_off));
        let kids = match &menu.rows[ix] {
            MenuRow::Item(item) => item.children.as_slice(),
            MenuRow::Separator => &[],
        };
        stack.push(
            deferred(
                anchored()
                    .position(at)
                    .snap_to_window_with_margin(px(8.))
                    .child(chrome(p).children(paint_rows(
                        ply,
                        kids,
                        1000 + ix * 20,
                        false,
                        None,
                        cx,
                    ))),
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
        .min_w(px(224.))
        .py(px(4.))
        .bg(p.card)
        .border_1()
        .border_color(p.border)
        .shadow_lg()
}

/// Menu metrics the flyout math must match: 30px rows, 1px separator lines
/// with 4px margins each side (9px total), 4px panel top pad, 1px outer
/// border. Full row-anchored nesting is out of scope; this table only.
const MENU_ROW_H: f32 = 30.;
const MENU_SEP_H: f32 = 9.;
const MENU_PANEL_PAD: f32 = 4.;
const MENU_BORDER_W: f32 = 1.;
/// Panel min-w (224) minus a 2px overlap so the flyout hugs the parent.
const MENU_FLYOUT_X: f32 = 222.;

/// Vertical offset of row `ix` from the panel top: panel pad plus every row
/// above `ix`, minus panel pad + border so the flyout panel top lands on the
/// parent row content instead of drifting below it. Pure so tests pin the
/// alignment without a GPUI context.
fn flyout_y_offset(rows: &[MenuRow], ix: usize) -> f32 {
    let mut y = MENU_PANEL_PAD;
    for row in rows.iter().take(ix) {
        y += match row {
            MenuRow::Separator => MENU_SEP_H,
            MenuRow::Item(_) => MENU_ROW_H,
        };
    }
    y - (MENU_PANEL_PAD + MENU_BORDER_W)
}

fn paint_rows(
    ply: &Ply,
    rows: &[MenuRow],
    base: usize,
    is_top: bool,
    selected: Option<usize>,
    cx: &mut Context<Ply>,
) -> Vec<AnyElement> {
    let hair = ply.palette().border;
    rows.iter()
        .enumerate()
        .map(|(i, row)| match row {
            MenuRow::Separator => div()
                .h(px(1.))
                .my(px(4.))
                .mx(px(10.))
                .bg(hair)
                .into_any_element(),
            // Only top-level rows carry a flyout target: the menu supports a
            // single flyout level, so submenu rows (unique `base` ids) never
            // open a second one. Keyboard selection lives on top-level rows
            // only, so the flyout never highlights.
            MenuRow::Item(item) => menu_row(
                ply,
                base + i,
                if is_top { Some(i) } else { None },
                is_top && selected == Some(i),
                item,
                cx,
            ),
        })
        .collect()
}

/// Shell source for a menu row's leading icon, if the row stands for
/// something the shell has artwork for. Prefers the explicit `shell` field
/// the app layer sets; rows built without one (chrome rows, older callers)
/// fall back to the Open action target so the Open row still paints its
/// raster. `None` means lucide only.
fn menu_shell_source(item: &MenuItem) -> Option<MenuIconSource> {
    if let Some(source) = item.shell.clone() {
        return Some(source);
    }
    match item.action.as_ref()? {
        MenuAction::Open(path) => Some(MenuIconSource::Path(path.clone())),
        _ => None,
    }
}

/// Probe a menu shell source the same way listing rows probe their entries.
/// Callers render Ready as a shell raster, Loading as a fixed blank slot,
/// and Glyph as the themed fallback.
fn probe_menu_shell(
    ply: &Ply,
    source: &MenuIconSource,
    cx: &mut Context<Ply>,
) -> crate::thumbs::IconProbe {
    match source {
        MenuIconSource::Path(path) => probe_shell_target(ply, path, cx),
        MenuIconSource::Class(ext) => {
            // Per-extension class icons are resolved by the listing batch;
            // the menu only reads the shared cache, never extracts. A cold
            // miss settles on the glyph; the next listing notify repaints.
            let cache = ply.thumb_cache().read(cx);
            if let Some(img) = cache.class_icon(ext) {
                crate::thumbs::IconProbe::Ready(img)
            } else if cache.class_is_inflight(ext) {
                crate::thumbs::IconProbe::Loading
            } else {
                crate::thumbs::IconProbe::Glyph
            }
        }
        MenuIconSource::Stock(stock) => crate::thumbs::stock_probe(ply, cx, menu_stock_icon(stock)),
    }
}

/// Pure mapping from menu-layer stock ids to the thumbnail worker's stock
/// ids. Kept by name so the two enums stay in sync; extracted so it is
/// unit-testable without a GPUI context.
fn menu_stock_icon(stock: &MenuStock) -> crate::thumbs::StockIcon {
    match stock {
        MenuStock::RecycleBin => crate::thumbs::StockIcon::RecycleBin,
        MenuStock::Shield => crate::thumbs::StockIcon::Shield,
        MenuStock::Info => crate::thumbs::StockIcon::Info,
        MenuStock::Delete => crate::thumbs::StockIcon::Delete,
        MenuStock::FolderOpen => crate::thumbs::StockIcon::FolderOpen,
        MenuStock::Folder => crate::thumbs::StockIcon::Folder,
        MenuStock::MixedFiles => crate::thumbs::StockIcon::MixedFiles,
    }
}

/// Probe a real path the same way the Open row does: Recycle Bin stock icon,
/// the listing entry probe when the path is a known entry, else a direct
/// path probe (volumes and folders). Callers render Ready as a shell raster,
/// Loading as a fixed blank slot, and Glyph as the themed fallback.
fn probe_shell_target(
    ply: &Ply,
    path: &std::path::Path,
    cx: &mut Context<Ply>,
) -> crate::thumbs::IconProbe {
    if crate::recycle_bin::is_recycle_bin(path) {
        return crate::thumbs::recycle_bin_probe(ply, cx);
    }
    if let LoadState::Ready(snap) = &ply.listing
        && let Some(entry) = snap.entries.iter().find(|e| e.path == path)
    {
        return crate::thumbs::entry_icon_probe(ply, entry, cx, ply.list_generation);
    }
    crate::thumbs::path_icon_probe(ply, path, 0, cx)
}

/// Segoe codepoint for a row's leading box (`MenuItem.glyph`). `Some` paints
/// via [`glyph_icon`]; `None` keeps the lucide `icon`/spacer path intact.
fn menu_glyph(item: &MenuItem) -> Option<char> {
    item.glyph
}

fn menu_row(
    ply: &Ply,
    id: usize,
    flyout_ix: Option<usize>,
    selected: bool,
    item: &MenuItem,
    cx: &mut Context<Ply>,
) -> AnyElement {
    let p = ply.palette();
    let enabled = item.enabled;
    // `flyout_ix` is `Some` for top-level rows only; submenu rows keep their
    // chevron but never open a second-level flyout.
    let flyout_target = match flyout_ix {
        Some(ix) if !item.children.is_empty() => Some(ix),
        _ => None,
    };
    let has_kids = flyout_target.is_some();
    let show_chevron = !item.children.is_empty();
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
    // Keyboard highlight follows `Menu.selected`, driven by
    // `Menu::move_selection` and the hover sync below. Hover paints the same
    // fill, so mouse and keyboard agree.

    div()
        .id(("menu", id))
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(4.))
        .h(px(30.))
        .text_size(px(12.5))
        .cursor_default()
        .text_color(color)
        .when(strong, |el| el.font_weight(FontWeight::MEDIUM))
        .when(selected, |el| el.bg(p.muted))
        .when(enabled, |el| el.hover(|s| s.bg(p.muted)))
        .map(|el| {
            // Shell rasters keep their own colors; the glyph fallback (Segoe
            // Fluent codepoint, else lucide) takes the row tint
            // (danger/disabled). All three states hold a fixed 14px box so
            // rows never shift while icons resolve.
            if let Some(source) = menu_shell_source(item) {
                el.child(match probe_menu_shell(ply, &source, cx) {
                    crate::thumbs::IconProbe::Ready(img) => thumb_img(&img, 14.).into_any_element(),
                    crate::thumbs::IconProbe::Loading => icon_slot(14.).into_any_element(),
                    crate::thumbs::IconProbe::Glyph => match menu_glyph(item) {
                        Some(g) => glyph_icon(g, 14., color).into_any_element(),
                        None => match item.icon {
                            Some(ico) => icon(ico, px(14.), color).into_any_element(),
                            None if strong => icon(Ico::Check, px(14.), color).into_any_element(),
                            None => div().w(px(14.)).into_any_element(),
                        },
                    },
                })
            } else if let Some(g) = menu_glyph(item) {
                el.child(glyph_icon(g, 14., color))
            } else if let Some(ico) = item.icon {
                el.child(icon(ico, px(14.), color))
            } else if strong {
                el.child(icon(Ico::Check, px(14.), color))
            } else {
                el.child(div().w(px(14.)))
            }
        })
        .child(div().flex_1().child(item.label.clone()))
        // Accelerator right column: muted hint at least 16px from the label
        // on the same baseline row. The label flexes so hints right-align
        // into a column; GPUI has no tabular figures, so size carries it.
        .when(item.shortcut.is_some(), |el| {
            el.child(
                div()
                    .ml(px(16.))
                    .text_size(px(12.))
                    .text_color(p.muted_foreground)
                    .child(item.shortcut.clone().unwrap()),
            )
        })
        .when(show_chevron, |el| {
            el.child(icon(Ico::ChevronRight, px(12.), p.muted_foreground))
        })
        .when(
            strong && item.icon.is_some() && item.children.is_empty(),
            |el| el.child(icon(Ico::Check, px(12.), color)),
        )
        .when(enabled && flyout_ix.is_some(), |el| {
            // Hover selects every enabled top-level row so Enter-after-hover
            // runs the hovered row; rows with children additionally open
            // their flyout (direct set, no toggle). Leaves never close an
            // open flyout, so the pointer can travel into it. No timers.
            let ix = flyout_ix.unwrap();
            el.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if !*hovered {
                    return;
                }
                let mut changed = false;
                if let Some(menu) = this.menu.as_mut() {
                    if menu.selected != Some(ix) {
                        menu.selected = Some(ix);
                        changed = true;
                    }
                    if has_kids && menu.flyout != Some(ix) {
                        menu.flyout = Some(ix);
                        changed = true;
                    }
                }
                if changed {
                    cx.notify();
                }
            }))
            .when(has_kids, |el| {
                el.on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.set_flyout(Some(ix), cx);
                }))
            })
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

/// Themed fallback for the Properties header when the shell raster is not
/// available. Prefers the same source the Open row would use: the volume
/// icon, then the listing entry icon, then a folder glyph for folders.
fn properties_fallback(ply: &Ply, path: &std::path::Path, kind: &str) -> Ico {
    if crate::recycle_bin::is_recycle_bin(path) {
        return Ico::Trash;
    }
    if let Some(v) = ply.volumes.iter().find(|v| v.path == path) {
        return v.ico();
    }
    if let LoadState::Ready(snap) = &ply.listing
        && let Some(entry) = snap.entries.iter().find(|e| e.path == path)
    {
        return crate::listing::entry_icon(entry);
    }
    if kind == "Folder" {
        return Ico::Folder;
    }
    Ico::File
}

fn properties(ply: &Ply, cx: &mut Context<Ply>) -> Option<AnyElement> {
    let props = ply.properties.as_ref()?;
    let p = ply.palette();
    let props_path = std::path::PathBuf::from(props.path.to_string());
    let fallback = properties_fallback(ply, &props_path, &props.kind);
    let dirty = ply.properties_dirty();

    // Main facts. The Path row is dropped by design; Contains hides when
    // empty and paints its Calculating… state muted. Parent folder truncates
    // single-line (full path stays in props.path). Size on disk hides when
    // empty (volumes); Created/Accessed hide on "—".
    let mut main: Vec<AnyElement> = Vec::new();
    main.push(field(p, "Type".into(), props.kind.clone(), p.foreground).into_any_element());
    if !props.opens_with.is_empty() {
        let choose_path = props_path.clone();
        main.push(
            div()
                .flex()
                .gap(px(12.))
                .py(px(3.))
                .text_size(px(12.))
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .text_color(p.muted_foreground)
                        .child("Opens with"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(p.foreground)
                                .child(props.opens_with.clone()),
                        )
                        .child(dialog_button(
                            "props-change-app",
                            "Change…".into(),
                            DialogButtonVariant::Default,
                            p,
                            cx.listener(move |this, _, window, cx| {
                                this.run(MenuAction::ChooseApp(choose_path.clone()), window, cx);
                            }),
                        )),
                )
                .into_any_element(),
        );
    }
    main.push(
        field_truncated(
            p,
            "Parent folder".into(),
            props.location.clone(),
            p.foreground,
        )
        .into_any_element(),
    );
    main.push(
        field(
            p,
            "Size".into(),
            size_value(&props.size, &props.size_detail),
            p.foreground,
        )
        .into_any_element(),
    );
    // Size on disk joins its byte-exact detail like Size does; the row
    // hides when the detail is empty (volumes, portable paths, failed
    // reads carry "" per the app model).
    if !props.size_on_disk_detail.is_empty() {
        main.push(
            field(
                p,
                "Size on disk".into(),
                size_value(&props.size_on_disk, &props.size_on_disk_detail),
                p.foreground,
            )
            .into_any_element(),
        );
    }
    if !props.contains.is_empty() {
        let calculating = props.contains.as_str() == CALCULATING;
        main.push(
            field(
                p,
                "Contains".into(),
                props.contains.clone(),
                if calculating {
                    p.muted_foreground
                } else {
                    p.foreground
                },
            )
            .into_any_element(),
        );
    }
    if show_dated_row(props.created.as_str()) {
        main.push(
            field(p, "Created".into(), props.created.clone(), p.foreground).into_any_element(),
        );
    }
    main.push(field(p, "Modified".into(), props.modified.clone(), p.foreground).into_any_element());
    if show_dated_row(props.accessed.as_str()) {
        main.push(
            field(p, "Accessed".into(), props.accessed.clone(), p.foreground).into_any_element(),
        );
    }
    let details: Vec<AnyElement> = props
        .details
        .iter()
        .map(|(label, value)| {
            field(p, label.clone(), value.clone(), p.foreground).into_any_element()
        })
        .collect();
    let hairline = || div().h(px(1.)).bg(p.border).my(px(10.)).into_any_element();
    let readonly = props.readonly;
    let hidden = props.hidden;
    let attrs_note = props.attrs_note;
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
                .on_click(cx.listener(|this, _, _, cx| {
                    if !scrim_should_close(this.properties_dirty()) {
                        return;
                    }
                    this.close_properties(cx);
                }))
                .child(
                    div()
                        .occlude()
                        .w(px(380.))
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
                                        .flex()
                                        .items_center()
                                        .gap(px(10.))
                                        .mb(px(10.))
                                        .child(match probe_shell_target(ply, &props_path, cx) {
                                            crate::thumbs::IconProbe::Ready(img) => {
                                                thumb_img(&img, 48.).into_any_element()
                                            }
                                            crate::thumbs::IconProbe::Loading => {
                                                icon_slot(48.).into_any_element()
                                            }
                                            crate::thumbs::IconProbe::Glyph => {
                                                icon(fallback, px(48.), p.muted_foreground)
                                                    .into_any_element()
                                            }
                                        })
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_size(px(14.))
                                                .truncate()
                                                .child(props.name.clone()),
                                        ),
                                )
                                .children(main)
                                // Shell details are their own section: hairline
                                // above, never per-row. Capped with its own
                                // scroll so Attributes + footer stay pinned.
                                .when(!details.is_empty(), |el| {
                                    el.child(hairline()).child(
                                        div()
                                            .id("props-details")
                                            .max_h(px(160.))
                                            .overflow_y_scroll()
                                            .children(details),
                                    )
                                })
                                .child(hairline())
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(12.))
                                        .py(px(3.))
                                        .text_size(px(12.))
                                        .child(
                                            div()
                                                .w(px(110.))
                                                .flex_none()
                                                .text_color(p.muted_foreground)
                                                .child("Attributes"),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .gap(px(16.))
                                                .child(attr_box(
                                                    p,
                                                    "attr-readonly",
                                                    "Read-only",
                                                    readonly,
                                                    attrs_note,
                                                    cx.listener(|this, _, _, cx| {
                                                        if let Some(props) =
                                                            this.properties.as_mut()
                                                        {
                                                            props.readonly =
                                                                attr_toggle(props.readonly);
                                                            cx.notify();
                                                        }
                                                    }),
                                                ))
                                                .child(attr_box(
                                                    p,
                                                    "attr-hidden",
                                                    "Hidden",
                                                    hidden,
                                                    false,
                                                    cx.listener(|this, _, _, cx| {
                                                        if let Some(props) =
                                                            this.properties.as_mut()
                                                        {
                                                            props.hidden =
                                                                attr_toggle(props.hidden);
                                                            cx.notify();
                                                        }
                                                    }),
                                                )),
                                        ),
                                )
                                .child(hairline())
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap(px(8.))
                                        // OK/Apply write the checkboxes back
                                        // first; all three dismiss. Apply keeps
                                        // its disabled look until the boxes
                                        // differ from the opening bits.
                                        .child(dialog_button(
                                            "props-ok",
                                            "OK".into(),
                                            DialogButtonVariant::Primary,
                                            p,
                                            cx.listener(|this, _, _, cx| {
                                                this.apply_properties(cx);
                                                this.close_properties(cx);
                                            }),
                                        ))
                                        .child(dialog_button(
                                            "props-cancel",
                                            "Cancel".into(),
                                            DialogButtonVariant::Default,
                                            p,
                                            cx.listener(|this, _, _, cx| this.close_properties(cx)),
                                        ))
                                        .child(dialog_button(
                                            "props-apply",
                                            "Apply".into(),
                                            if dirty {
                                                DialogButtonVariant::Default
                                            } else {
                                                DialogButtonVariant::Disabled
                                            },
                                            p,
                                            cx.listener(|this, _, _, cx| {
                                                this.apply_properties(cx);
                                                this.close_properties(cx);
                                            }),
                                        )),
                                ),
                        ),
                ),
        )
        .into_any_element(),
    )
}

/// The in-flight marker the app seeds directory Size/Contains with while its
/// background walk runs.
const CALCULATING: &str = "Calculating…";

/// Join the Size row: `size` plus its byte-exact `size_detail`, collapsing to
/// one copy when the detail is empty or identical (the Calculating… and —
/// states). Pure, unit-tested.
fn size_value(size: &gpui::SharedString, detail: &gpui::SharedString) -> gpui::SharedString {
    if detail.is_empty() || detail == size {
        size.clone()
    } else {
        gpui::SharedString::from(format!("{size} {detail}"))
    }
}

/// One Properties grid row: 110px grey label, wrapping left value. Only the
/// header name truncates; sections are separated by hairlines, rows inside a
/// section by spacing.
fn field(
    p: crate::theme::Palette,
    label: gpui::SharedString,
    value: gpui::SharedString,
    value_color: gpui::Hsla,
) -> gpui::Div {
    div()
        .flex()
        .gap(px(12.))
        .py(px(3.))
        .text_size(px(12.))
        .child(
            div()
                .w(px(110.))
                .flex_none()
                .text_color(p.muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(value_color)
                .child(value),
        )
}

/// One Properties grid row with a single-line truncated value (Parent
/// folder). The full path stays in `Properties.path` for copy and for the
/// Change…/ChooseApp target; this only clips display.
fn field_truncated(
    p: crate::theme::Palette,
    label: gpui::SharedString,
    value: gpui::SharedString,
    value_color: gpui::Hsla,
) -> gpui::Div {
    div()
        .flex()
        .gap(px(12.))
        .py(px(3.))
        .text_size(px(12.))
        .child(
            div()
                .w(px(110.))
                .flex_none()
                .text_color(p.muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(value_color)
                .child(value),
        )
}

/// One attribute checkbox with its label: a 14px Square outline holding the
/// [`attr_mark`] overlay (check, or the mixed dash for `None`). Directories
/// show Explorer's folder-only note under Read-only via `with_note`.
fn attr_box(
    p: crate::theme::Palette,
    id: &'static str,
    label: &'static str,
    state: Option<bool>,
    with_note: bool,
    click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let mark = attr_mark(state);
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(6.))
                .cursor_default()
                .child(
                    div()
                        .relative()
                        .size(px(14.))
                        .flex_none()
                        .child(icon(Ico::Square, px(14.), p.muted_foreground))
                        .when(mark.is_some(), |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon(mark.unwrap(), px(10.), p.foreground)),
                            )
                        }),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(p.foreground)
                        .child(label),
                )
                .on_click(click),
        )
        .when(with_note, |el| {
            el.child(
                div()
                    .text_size(px(11.))
                    .text_color(p.muted_foreground)
                    .child("Only applies to files in folder"),
            )
        })
}

/// Overlay mark for a tri-state attribute box: checked, nothing, or the
/// mixed dash for `None`. Pure, unit-tested.
fn attr_mark(state: Option<bool>) -> Option<Ico> {
    match state {
        Some(true) => Some(Ico::Check),
        Some(false) => None,
        None => Some(Ico::Minus),
    }
}

/// Click cycle for a tri-state attribute box: mixed checks, then toggles.
/// Pure, unit-tested; the caller writes the result back and notifies so the
/// footer repaints.
fn attr_toggle(state: Option<bool>) -> Option<bool> {
    match state {
        None => Some(true),
        Some(b) => Some(!b),
    }
}

/// Shared dialog footer button (Properties + confirm). Ply hand-rolls
/// controls rather than pulling in a component lib. Sharp corners per the
/// zero-radius token (no `rounded` call). Ids stay namespaced by the caller
/// (`props-*` vs label-keyed confirm ids) so the two layers never collide.
/// Primary is filled neutral (OK only); Default/Danger stay bordered;
/// Disabled is hairline + muted and never fires (caller skips on_click).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DialogButtonVariant {
    Primary,
    Default,
    Danger,
    Disabled,
}

/// Whether a dialog button fires its click. Disabled never fires; the caller
/// skips `on_click` entirely so a disabled Apply cannot write. Pure,
/// unit-tested.
fn dialog_button_enabled(variant: DialogButtonVariant) -> bool {
    variant != DialogButtonVariant::Disabled
}

/// Whether the Properties scrim may dismiss. Dirty blocks the scrim (require
/// OK/Cancel/Apply); Cancel itself still closes. Pure, unit-tested.
fn scrim_should_close(dirty: bool) -> bool {
    !dirty
}

/// Whether a dated row (Created/Accessed) shows. Volumes carry "—". Pure,
/// unit-tested.
fn show_dated_row(value: &str) -> bool {
    value != "—"
}

/// Border/text colors for a [`DialogButtonVariant`]. Pure, unit-tested:
/// primary fills neutral (border foreground, text background), default
/// borders foreground (secondary emphasis without hue), danger borders
/// destructive, disabled borders the hairline and mutes the text.
fn dialog_button_colors(
    variant: DialogButtonVariant,
    p: crate::theme::Palette,
) -> (gpui::Hsla, gpui::Hsla) {
    match variant {
        DialogButtonVariant::Primary => (p.foreground, p.background),
        DialogButtonVariant::Default => (p.foreground, p.foreground),
        DialogButtonVariant::Danger => (p.destructive, p.destructive),
        DialogButtonVariant::Disabled => (p.border, p.muted_foreground),
    }
}

fn dialog_button(
    id: impl Into<gpui::ElementId>,
    label: gpui::SharedString,
    variant: DialogButtonVariant,
    p: crate::theme::Palette,
    click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let (border, text) = dialog_button_colors(variant, p);
    let enabled = dialog_button_enabled(variant);
    let primary = variant == DialogButtonVariant::Primary;
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(64.))
        .px(px(12.))
        .py(px(4.))
        .text_size(px(12.))
        .text_color(text)
        .border_1()
        .border_color(border)
        .cursor_default()
        .when(primary, |el| {
            el.bg(p.foreground).font_weight(FontWeight::MEDIUM)
        })
        .when(enabled && !primary, |el| {
            el.hover(|s| s.bg(p.muted))
                .active(|s| s.bg(p.select_strong))
        })
        .when(!enabled, |el| el.opacity(0.4))
        .child(label)
        .map(|el| if enabled { el.on_click(click) } else { el })
}

/// A modal confirm step. Dismisses on Cancel, clicking the scrim, or Esc; the
/// confirming action runs only on the explicit Confirm button.
fn confirm_dialog(ply: &Ply, cx: &mut Context<Ply>) -> Option<AnyElement> {
    let dialog = ply.confirm.as_ref()?;
    let p = ply.palette();
    let confirm_variant = if dialog.danger {
        DialogButtonVariant::Danger
    } else {
        DialogButtonVariant::Default
    };
    let confirm_text = dialog.confirm_text.clone();
    let confirm_id = confirm_text.to_string();
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
                                        .child(dialog_button(
                                            cancel_text.to_string(),
                                            cancel_text,
                                            DialogButtonVariant::Default,
                                            p,
                                            cx.listener(|this, _, _, cx| this.cancel_confirm(cx)),
                                        ))
                                        .child(dialog_button(
                                            confirm_id,
                                            confirm_text,
                                            confirm_variant,
                                            p,
                                            cx.listener(|this, _, _, cx| this.run_confirm(cx)),
                                        )),
                                ),
                        ),
                ),
        )
        .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item_row(label: &str) -> MenuRow {
        MenuRow::Item(MenuItem {
            label: label.into(),
            icon: None,
            shell: None,
            action: None,
            children: Vec::new(),
            enabled: true,
            danger: false,
            strong: false,
            shortcut: None,
            glyph: None,
        })
    }

    #[test]
    fn menu_stock_maps_to_same_named_stock_icon() {
        let cases = [
            (MenuStock::RecycleBin, crate::thumbs::StockIcon::RecycleBin),
            (MenuStock::Shield, crate::thumbs::StockIcon::Shield),
            (MenuStock::Info, crate::thumbs::StockIcon::Info),
            (MenuStock::Delete, crate::thumbs::StockIcon::Delete),
            (MenuStock::FolderOpen, crate::thumbs::StockIcon::FolderOpen),
            (MenuStock::Folder, crate::thumbs::StockIcon::Folder),
            (MenuStock::MixedFiles, crate::thumbs::StockIcon::MixedFiles),
        ];
        for (stock, expected) in cases {
            assert_eq!(menu_stock_icon(&stock), expected);
        }
    }

    #[test]
    fn flyout_offset_first_row_lands_on_content() {
        let rows = [item_row("Open"), item_row("Rename")];
        // Panel pad cancels (both panels share it); minus the 1px outer
        // border so the flyout panel top lands on the parent row content.
        assert_eq!(flyout_y_offset(&rows, 0), -MENU_BORDER_W);
    }

    #[test]
    fn flyout_offset_sums_items() {
        let rows = [item_row("a"), item_row("b"), item_row("c")];
        assert_eq!(flyout_y_offset(&rows, 2), 2. * MENU_ROW_H - MENU_BORDER_W);
    }

    #[test]
    fn flyout_offset_counts_separators_above_ix() {
        let rows = [
            item_row("a"),
            MenuRow::Separator,
            item_row("b"),
            MenuRow::Separator,
            item_row("c"),
        ];
        // Rows above ix 2: item + separator.
        assert_eq!(
            flyout_y_offset(&rows, 2),
            MENU_ROW_H + MENU_SEP_H - MENU_BORDER_W
        );
        // Rows above ix 4: item, sep, item, sep.
        assert_eq!(
            flyout_y_offset(&rows, 4),
            2. * MENU_ROW_H + 2. * MENU_SEP_H - MENU_BORDER_W
        );
    }

    #[test]
    fn flyout_offset_two_adjacent_seps_above_ix() {
        let rows = [
            item_row("a"),
            MenuRow::Separator,
            MenuRow::Separator,
            item_row("b"),
        ];
        // Two separators stacked above ix 3: item + sep + sep.
        assert_eq!(
            flyout_y_offset(&rows, 3),
            MENU_ROW_H + 2. * MENU_SEP_H - MENU_BORDER_W
        );
    }

    #[test]
    fn flyout_offset_ignores_separator_at_ix_and_rows_below() {
        let rows = [item_row("a"), MenuRow::Separator, item_row("b")];
        // Only the item above ix 1 counts; the separator at ix and the row
        // below never do.
        assert_eq!(flyout_y_offset(&rows, 1), MENU_ROW_H - MENU_BORDER_W);
    }

    #[test]
    fn dialog_button_colors_follow_variant() {
        let p = crate::theme::Mode::Light.palette();
        assert_eq!(
            dialog_button_colors(DialogButtonVariant::Primary, p),
            (p.foreground, p.background)
        );
        assert_eq!(
            dialog_button_colors(DialogButtonVariant::Default, p),
            (p.foreground, p.foreground)
        );
        assert_eq!(
            dialog_button_colors(DialogButtonVariant::Danger, p),
            (p.destructive, p.destructive)
        );
        assert_eq!(
            dialog_button_colors(DialogButtonVariant::Disabled, p),
            (p.border, p.muted_foreground)
        );
    }

    #[test]
    fn dialog_button_enabled_only_when_not_disabled() {
        assert!(dialog_button_enabled(DialogButtonVariant::Primary));
        assert!(dialog_button_enabled(DialogButtonVariant::Default));
        assert!(dialog_button_enabled(DialogButtonVariant::Danger));
        assert!(!dialog_button_enabled(DialogButtonVariant::Disabled));
    }

    #[test]
    fn scrim_blocks_close_when_dirty() {
        assert!(scrim_should_close(false));
        assert!(!scrim_should_close(true));
    }

    #[test]
    fn dated_rows_hide_dash() {
        assert!(!show_dated_row("—"));
        assert!(show_dated_row("Monday, December 1, 2025, 2:08:28 PM"));
        assert!(show_dated_row(""));
    }

    #[test]
    fn attr_mark_maps_tri_state() {
        assert_eq!(attr_mark(Some(true)), Some(Ico::Check));
        assert_eq!(attr_mark(Some(false)), None);
        assert_eq!(attr_mark(None), Some(Ico::Minus));
    }

    #[test]
    fn attr_toggle_checks_mixed_then_flips() {
        assert_eq!(attr_toggle(None), Some(true));
        assert_eq!(attr_toggle(Some(true)), Some(false));
        assert_eq!(attr_toggle(Some(false)), Some(true));
    }

    #[test]
    fn size_value_joins_detail_and_collapses_dupes() {
        let size = gpui::SharedString::from("553 MB");
        let detail = gpui::SharedString::from("(580,833,358 bytes)");
        assert_eq!(size_value(&size, &detail), "553 MB (580,833,358 bytes)");
        let calculating = gpui::SharedString::from("Calculating…");
        assert_eq!(size_value(&calculating, &calculating), "Calculating…");
        let dash = gpui::SharedString::from("—");
        assert_eq!(size_value(&dash, &dash), "—");
        let empty = gpui::SharedString::from("");
        assert_eq!(size_value(&size, &empty), "553 MB");
    }
}
