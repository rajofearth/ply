//! Solid dark chrome for Ply — Zed/Zeron density, no glass.

use std::rc::Rc;

use gpui::{App, Hsla, px};
use gpui_component::{ActiveTheme, Theme, ThemeConfigColors, ThemeMode};

/// Switch to dark mode, then overlay Ply's semantic colors and tight radius.
///
/// Call after `gpui_component::init`. `Theme::change` is used twice: first so a
/// dark `ThemeConfig` exists, then again so `gpui_base` tokens pick up the
/// overlay (mutating `Theme` fields alone would leave Base out of sync).
pub fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);

    let mut config = Theme::global(cx).dark_theme.as_ref().clone();
    config.name = "Ply".into();
    config.radius = Some(2);
    config.radius_lg = Some(4);
    config.shadow = Some(false);
    config.font_size = Some(13.0);
    config.mono_font_size = Some(12.0);
    ply_colors(&mut config.colors);

    Theme::global_mut(cx).dark_theme = Rc::new(config);
    Theme::change(ThemeMode::Dark, None, cx);

    let theme = Theme::global_mut(cx);
    theme.tile_radius = px(0.);
    theme.tile_shadow = false;
}

/// Solid listing / preview surface (slightly above window chrome).
pub fn pane_bg(cx: &App) -> Hsla {
    cx.theme().table
}

/// Slightly lifted tree / sidebar surface.
pub fn sidebar_bg(cx: &App) -> Hsla {
    cx.theme().sidebar
}

fn ply_colors(c: &mut ThemeConfigColors) {
    // Near-black chrome, not pure #000.
    c.background = Some("#141416".into());
    c.title_bar = Some("#141416".into());
    c.title_bar_border = Some("#2a2a2e".into());
    c.status_bar = Some("#141416".into());
    c.status_bar_border = Some("#2a2a2e".into());
    c.window_border = Some("#2a2a2e".into());
    c.border = Some("#2a2a2e".into());
    c.input = Some("#2a2a2e".into());
    c.ring = Some("#3d5a80".into());

    c.foreground = Some("#e4e4e7".into());
    c.muted = Some("#27272a".into());
    c.muted_foreground = Some("#8b8b93".into());

    // Sidebar / tree: one step up from chrome.
    c.sidebar = Some("#18181b".into());
    c.sidebar_border = Some("#2a2a2e".into());
    c.sidebar_foreground = Some("#d4d4d8".into());
    c.sidebar_accent = Some("#2f6fed33".into());
    c.sidebar_accent_foreground = Some("#e4e4e7".into());
    c.list = Some("#18181b".into());
    c.list_even = Some("#18181b".into());
    c.list_head = Some("#18181b".into());
    c.list_hover = Some("#222226".into());
    c.list_active = Some("#2f6fed33".into());
    c.list_active_border = Some("#2f6fed66".into());

    // Content / table: solid lifted surface.
    c.table = Some("#1c1c1f".into());
    c.table_even = Some("#1c1c1f".into());
    c.table_head = Some("#1c1c1f".into());
    c.table_head_foreground = Some("#8b8b93".into());
    c.table_hover = Some("#252528".into());
    c.table_active = Some("#2f6fed33".into());
    c.table_active_border = Some("#2f6fed66".into());
    c.table_row_border = Some("#2a2a2e80".into());
    c.accordion = Some("#1c1c1f".into());
    c.tiles = Some("#1c1c1f".into());
    c.popover = Some("#1c1c1f".into());
    c.popover_foreground = Some("#e4e4e7".into());
    c.group_box = Some("#1c1c1f".into());

    // Accent only for selection / focus, not as a flood fill.
    c.accent = Some("#252a33".into());
    c.accent_foreground = Some("#e4e4e7".into());
    c.selection = Some("#2f6fed55".into());
    c.primary = Some("#3d6ea8".into());
    c.primary_foreground = Some("#f4f4f5".into());
    c.primary_hover = Some("#4a7db8".into());
    c.primary_active = Some("#2f5a8a".into());

    c.scrollbar = Some("#14141600".into());
    c.scrollbar_thumb = Some("#3f3f46cc".into());
    c.scrollbar_thumb_hover = Some("#52525b".into());

    c.tab_bar = Some("#141416".into());
    c.tab = Some("#14141600".into());
    c.tab_active = Some("#1c1c1f".into());
    c.tab_active_foreground = Some("#e4e4e7".into());
    c.tab_foreground = Some("#8b8b93".into());
    c.tab_bar_segmented = Some("#18181b".into());

    c.overlay = Some("#00000066".into());
}
