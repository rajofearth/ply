use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div, prelude::FluentBuilder, px,
};

use super::icon;
use crate::app::{Ply, ViewMode};
use crate::icons::Ico;

/// Sentence-case filter placeholder: "Filter N items…".
/// Pure so it is unit-testable; the live count is wired in
/// `Ply::sync_filter_placeholder` (owns the window + input state).
/// Intended caller is `sync_filter_placeholder`; allowed dead until that
/// one-line wiring lands (outside these owned files).
#[allow(dead_code)]
pub fn filter_placeholder(count: usize) -> String {
    format!("Filter {count} items…")
}

/// Only shown inside a folder: counts, the filter, and the view toggle.
pub fn render(ply: &Ply, cx: &mut Context<Ply>) -> impl IntoElement {
    let p = ply.palette();
    let shown = ply.visible_len();
    let selected = ply.selection.len();

    let left = match &ply.status {
        Some(message) => message.to_string(),
        None if selected > 0 => format!("{shown} items · {selected} selected"),
        None => format!("{shown} items"),
    };

    div()
        .flex()
        .items_center()
        .justify_between()
        .h(px(26.))
        .px(px(12.))
        .flex_none()
        .border_t_1()
        .border_color(p.border)
        .text_size(px(11.))
        .text_color(p.muted_foreground)
        .child(div().truncate().child(left))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .flex_none()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .w(px(170.))
                        .px(px(8.))
                        .py(px(3.))
                        .border_1()
                        .border_color(p.border)
                        .child(icon(Ico::Search, px(12.), p.muted_foreground))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(p.foreground)
                                .child(ply.filter_field.clone()),
                        )
                        .when(!ply.filter_text.is_empty(), |el| {
                            el.child(
                                div()
                                    .id("filter-clear")
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .justify_center()
                                    .w(px(18.))
                                    .h(px(18.))
                                    .text_size(px(12.))
                                    .text_color(p.muted_foreground)
                                    .cursor_default()
                                    .hover(|s| s.bg(p.muted))
                                    .child("×")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.clear_filter(window, cx);
                                        window.blur();
                                    })),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .border_1()
                        .border_color(p.border)
                        .child(toggle(ply, ViewMode::List, Ico::List, cx))
                        .child(toggle(ply, ViewMode::Grid, Ico::LayoutGrid, cx)),
                ),
        )
}

fn toggle(ply: &Ply, view: ViewMode, ico: Ico, cx: &mut Context<Ply>) -> impl IntoElement {
    let p = ply.palette();
    let on = ply.view == view;
    div()
        .id(match view {
            ViewMode::List => "view-list",
            ViewMode::Grid => "view-grid",
        })
        .flex()
        .px(px(6.))
        .py(px(3.))
        .cursor_default()
        .when(on, |el| el.bg(p.select_strong))
        .when(!on, |el| el.hover(|s| s.bg(p.muted)))
        .child(icon(
            ico,
            px(12.),
            if on { p.foreground } else { p.muted_foreground },
        ))
        .on_click(cx.listener(move |this, _, _, cx| this.set_view(view, cx)))
}

#[cfg(test)]
mod tests {
    use super::filter_placeholder;

    #[test]
    fn placeholder_is_sentence_case_with_ellipsis() {
        assert_eq!(filter_placeholder(0), "Filter 0 items…");
        assert_eq!(filter_placeholder(1), "Filter 1 items…");
        assert_eq!(filter_placeholder(123), "Filter 123 items…");
    }

    #[test]
    fn placeholder_never_lowercase_lead() {
        for n in [0, 2, 42] {
            let s = filter_placeholder(n);
            assert!(s.starts_with("Filter "), "got {s:?}");
            assert!(s.ends_with('…'), "got {s:?}");
            assert!(!s.starts_with("filter "), "must be sentence-case: {s:?}");
        }
    }
}
