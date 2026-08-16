//! Ply theme — sharp (radius 0) light/dark tokens mapped from the React explorer design.

use std::rc::Rc;

use gpui::{App, Hsla, Window, px, rgb};
use gpui_component::{ActiveTheme, Theme, ThemeConfig, ThemeConfigColors, ThemeMode};

/// Apply Ply's dark theme (React default). Call after `gpui_component::init`.
pub fn apply(cx: &mut App) {
    apply_mode(ThemeMode::Dark, None, cx);
}

/// Install light + dark Ply configs, then switch to `mode`.
///
/// `Theme::change` runs twice: first so registry defaults exist, then again so
/// `gpui_base` tokens pick up the overlay after we mutate `ThemeConfig`s.
pub fn apply_mode(mode: ThemeMode, window: Option<&mut Window>, cx: &mut App) {
    Theme::change(mode, None, cx);

    let mut light = Theme::global(cx).light_theme.as_ref().clone();
    configure_shell(&mut light, ThemeMode::Light, "Ply Light");
    ply_light_colors(&mut light.colors);
    Theme::global_mut(cx).light_theme = Rc::new(light);

    let mut dark = Theme::global(cx).dark_theme.as_ref().clone();
    configure_shell(&mut dark, ThemeMode::Dark, "Ply Dark");
    ply_dark_colors(&mut dark.colors);
    Theme::global_mut(cx).dark_theme = Rc::new(dark);

    Theme::change(mode, window, cx);

    let theme = Theme::global_mut(cx);
    theme.tile_radius = px(0.);
    theme.tile_shadow = false;
}

/// Toggle between light and dark Ply themes and refresh the window.
pub fn toggle(window: &mut Window, cx: &mut App) {
    let next = if Theme::global(cx).is_dark() {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    apply_mode(next, Some(window), cx);
}

/// Listing / preview surface (card / table).
pub fn pane_bg(cx: &App) -> Hsla {
    cx.theme().table
}

/// Tree / sidebar surface.
pub fn sidebar_bg(cx: &App) -> Hsla {
    cx.theme().sidebar
}

/// Progress / chart fill.
pub fn chart_bar(cx: &App) -> Hsla {
    if cx.theme().is_dark() {
        rgb(0xa3a3a3).into()
    } else {
        rgb(0x737373).into()
    }
}

/// Progress / chart track.
pub fn chart_bar_track(cx: &App) -> Hsla {
    if cx.theme().is_dark() {
        rgb(0x525252).into()
    } else {
        rgb(0xebebeb).into()
    }
}

/// Strong selection fill (list / table active).
pub fn select_strong(cx: &App) -> Hsla {
    cx.theme().list_active
}

/// Elevated card / popover surface.
pub fn card_bg(cx: &App) -> Hsla {
    cx.theme().popover
}

fn configure_shell(config: &mut ThemeConfig, mode: ThemeMode, name: &str) {
    config.name = name.into();
    config.mode = mode;
    config.radius = Some(0);
    config.radius_lg = Some(0);
    config.shadow = Some(false);
    config.font_size = Some(13.0);
    config.mono_font_size = Some(12.0);
    // gpui system UI font sentinel (platform native: Segoe UI / SF / etc.).
    config.font_family = Some(".SystemUIFont".into());
}

fn ply_light_colors(c: &mut ThemeConfigColors) {
    // Surfaces
    c.background = Some("#ffffff".into());
    c.title_bar = Some("#ffffff".into());
    c.title_bar_border = Some("#ebebeb".into());
    c.status_bar = Some("#ffffff".into());
    c.status_bar_border = Some("#ebebeb".into());
    c.window_border = Some("#ebebeb".into());
    c.border = Some("#ebebeb".into());
    c.input = Some("#ebebeb".into());
    c.ring = Some("#737373".into());

    c.foreground = Some("#252525".into());
    c.muted = Some("#f7f7f7".into());
    c.muted_foreground = Some("#737373".into());

    c.accent = Some("#f7f7f7".into());
    c.accent_foreground = Some("#333333".into());

    // Card surfaces
    c.popover = Some("#ffffff".into());
    c.popover_foreground = Some("#252525".into());
    c.table = Some("#ffffff".into());
    c.table_even = Some("#ffffff".into());
    c.table_head = Some("#fafafa".into());
    c.table_head_foreground = Some("#737373".into());
    c.table_hover = Some("#f7f7f7".into());
    c.table_row_border = Some("#ebebeb".into());
    c.accordion = Some("#ffffff".into());
    c.tiles = Some("#ffffff".into());
    c.group_box = Some("#ffffff".into());
    c.group_box_foreground = Some("#252525".into());

    // Sidebar
    c.sidebar = Some("#fafafa".into());
    c.sidebar_border = Some("#ebebeb".into());
    c.sidebar_foreground = Some("#252525".into());
    c.sidebar_accent = Some("#ebebeb".into());
    c.sidebar_accent_foreground = Some("#252525".into());
    c.sidebar_primary = Some("#252525".into());
    c.sidebar_primary_foreground = Some("#ffffff".into());

    // Lists share sidebar chrome
    c.list = Some("#fafafa".into());
    c.list_even = Some("#fafafa".into());
    c.list_head = Some("#fafafa".into());
    c.list_hover = Some("#f7f7f7".into());

    // selectStrong
    let select = "#ebebeb";
    c.list_active = Some(select.into());
    c.list_active_border = Some(select.into());
    c.table_active = Some(select.into());
    c.table_active_border = Some(select.into());
    c.selection = Some(select.into());

    c.danger = Some("#dc2626".into());
    c.danger_foreground = Some("#ffffff".into());
    c.danger_hover = Some("#b91c1c".into());
    c.danger_active = Some("#991b1b".into());

    c.primary = Some("#252525".into());
    c.primary_foreground = Some("#ffffff".into());
    c.primary_hover = Some("#333333".into());
    c.primary_active = Some("#141414".into());

    c.secondary = Some("#f7f7f7".into());
    c.secondary_foreground = Some("#252525".into());
    c.secondary_hover = Some("#ebebeb".into());
    c.secondary_active = Some("#e0e0e0".into());

    c.progress_bar = Some("#737373".into());
    c.chart_1 = Some("#737373".into());

    c.scrollbar = Some("#ffffff00".into());
    c.scrollbar_thumb = Some("#ebebebcc".into());
    c.scrollbar_thumb_hover = Some("#a3a3a3".into());

    c.tab_bar = Some("#ffffff".into());
    c.tab = Some("#ffffff00".into());
    c.tab_active = Some("#ffffff".into());
    c.tab_active_foreground = Some("#252525".into());
    c.tab_foreground = Some("#737373".into());
    c.tab_bar_segmented = Some("#fafafa".into());

    c.overlay = Some("#00000033".into());
}

fn ply_dark_colors(c: &mut ThemeConfigColors) {
    // Surfaces
    c.background = Some("#252525".into());
    c.title_bar = Some("#252525".into());
    c.title_bar_border = Some("#ffffff1a".into());
    c.status_bar = Some("#252525".into());
    c.status_bar_border = Some("#ffffff1a".into());
    c.window_border = Some("#ffffff1a".into());
    c.border = Some("#ffffff1a".into());
    c.input = Some("#ffffff1a".into());
    c.ring = Some("#a3a3a3".into());

    c.foreground = Some("#fafafa".into());
    c.muted = Some("#444444".into());
    c.muted_foreground = Some("#a3a3a3".into());

    c.accent = Some("#444444".into());
    c.accent_foreground = Some("#fafafa".into());

    // Card / popover / table
    c.popover = Some("#333333".into());
    c.popover_foreground = Some("#fafafa".into());
    c.table = Some("#333333".into());
    c.table_even = Some("#333333".into());
    c.table_head = Some("#333333".into());
    c.table_head_foreground = Some("#a3a3a3".into());
    c.table_hover = Some("#444444".into());
    c.table_row_border = Some("#ffffff1a".into());
    c.accordion = Some("#333333".into());
    c.tiles = Some("#333333".into());
    c.group_box = Some("#333333".into());
    c.group_box_foreground = Some("#fafafa".into());

    // Sidebar
    c.sidebar = Some("#333333".into());
    c.sidebar_border = Some("#ffffff1a".into());
    c.sidebar_foreground = Some("#fafafa".into());
    c.sidebar_accent = Some("#525252".into());
    c.sidebar_accent_foreground = Some("#fafafa".into());
    c.sidebar_primary = Some("#fafafa".into());
    c.sidebar_primary_foreground = Some("#252525".into());

    c.list = Some("#333333".into());
    c.list_even = Some("#333333".into());
    c.list_head = Some("#333333".into());
    c.list_hover = Some("#444444".into());

    // selectStrong
    let select = "#525252";
    c.list_active = Some(select.into());
    c.list_active_border = Some(select.into());
    c.table_active = Some(select.into());
    c.table_active_border = Some(select.into());
    c.selection = Some(select.into());

    c.danger = Some("#dc2626".into());
    c.danger_foreground = Some("#fafafa".into());
    c.danger_hover = Some("#ef4444".into());
    c.danger_active = Some("#b91c1c".into());

    c.primary = Some("#fafafa".into());
    c.primary_foreground = Some("#252525".into());
    c.primary_hover = Some("#ffffff".into());
    c.primary_active = Some("#e5e5e5".into());

    c.secondary = Some("#444444".into());
    c.secondary_foreground = Some("#fafafa".into());
    c.secondary_hover = Some("#525252".into());
    c.secondary_active = Some("#3a3a3a".into());

    c.progress_bar = Some("#a3a3a3".into());
    c.chart_1 = Some("#a3a3a3".into());

    c.scrollbar = Some("#25252500".into());
    c.scrollbar_thumb = Some("#525252cc".into());
    c.scrollbar_thumb_hover = Some("#a3a3a3".into());

    c.tab_bar = Some("#252525".into());
    c.tab = Some("#25252500".into());
    c.tab_active = Some("#333333".into());
    c.tab_active_foreground = Some("#fafafa".into());
    c.tab_foreground = Some("#a3a3a3".into());
    c.tab_bar_segmented = Some("#333333".into());

    c.overlay = Some("#00000066".into());
}
