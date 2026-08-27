use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px,
};

use super::{icon, section_label};
use crate::app::Ply;
use crate::listing::format_size;
use crate::volumes::{self, Volume};

/// The idle dashboard: drives and devices only, and no status bar.
pub fn render(ply: &Ply, cx: &mut Context<Ply>) -> impl IntoElement {
    let p = ply.palette();
    let (drives, devices) = volumes::partition_drives_devices(&ply.volumes);
    let has_devices = !devices.is_empty();

    div()
        .id("home")
        .flex_1()
        .p(px(20.))
        .overflow_y_scroll()
        .child(section_label("Drives", p.muted_foreground))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(10.))
                .mb(px(24.))
                .children(drives.into_iter().map(|v| card(ply, v, cx))),
        )
        .child(section_label("Devices & network", p.muted_foreground))
        .when(!has_devices, |el| {
            el.child(
                div()
                    .text_size(px(12.5))
                    .text_color(p.muted_foreground)
                    .child("No devices or network drives connected."),
            )
        })
        .when(has_devices, |el| {
            el.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .children(devices.into_iter().map(|v| card(ply, v, cx))),
            )
        })
}

/// One volume: name, a capacity bar, and free/total.
fn card(ply: &Ply, v: &Volume, cx: &mut Context<Ply>) -> AnyElement {
    let p = ply.palette();
    let ico = v.ico();
    let pct = v.pct_used();
    let fill = if pct >= 81.0 { p.destructive } else { p.chart_bar };
    let path = v.path.clone();
    let id = super::stable_id(&v.path);

    div()
        .id(("card", id))
        .w(px(240.))
        .p(px(14.))
        .border_1()
        .border_color(p.border)
        .cursor_default()
        .hover(|s| s.bg(p.muted))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .mb(px(10.))
                .child(icon(ico, px(16.), p.muted_foreground))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(v.name.clone()),
                ),
        )
        .child(
            div()
                .h(px(4.))
                .mb(px(6.))
                .bg(p.chart_bar_track)
                .child(div().h_full().w(gpui::relative(pct / 100.)).bg(fill)),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .text_size(px(11.))
                .text_color(p.muted_foreground)
                .child(format!("{} free", format_size(v.free)))
                .child(format_size(v.total)),
        )
        .on_click({
            let path = path.clone();
            cx.listener(move |this, _, window, cx| this.open_folder(path.clone(), window, cx))
        })
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                this.open_menu(ev.position, path.clone(), cx);
            }),
        )
        .into_any_element()
}
