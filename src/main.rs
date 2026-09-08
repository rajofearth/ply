#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod budget;
mod cache;
mod fs_ops;
mod icons;
mod listing;
mod mtp;
mod path_caps;
mod recycle_bin;
mod theme;
mod thumbs;
mod ui;
mod volumes;
mod watch;

use gpui::{AppContext, KeyBinding, WindowBounds, WindowOptions, actions, point, px, size};

use app::Ply;
use ui::{
    Activate, BeginRename, CopySelectedPath, DeleteSelection, Dismiss, ExtendDown, ExtendLeft,
    ExtendRight, ExtendUp, FocusFilter, GoBack, GoForward, GoHome, GoUp, Refresh, SelectDown,
    SelectLeft, SelectRight, SelectUp, ToggleTheme,
};

// Context-menu keyboard actions, driven by the overlay's selection model
// (`Menu.selected` / `Menu::move_selection`). Root handlers live in `ui`:
// each fires only while a menu is open and focus is not typing, and the
// listing actions below stand down while a menu is open so keys never act
// twice.
actions!(ply, [MenuUp, MenuDown, MenuLeft, MenuRight, MenuActivate,]);

fn main() {
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(|cx| {
            gpui_component::init(cx);
            // Grayscale text: the subpixel text pipeline costs ~100 MB of
            // GPU-shared memory on first paint (measured); grayscale is
            // visually near-identical and skips it.
            cx.set_text_rendering_mode(gpui::TextRenderingMode::Grayscale);

            cx.bind_keys([
                KeyBinding::new("d", ToggleTheme, Some("Ply")),
                KeyBinding::new("alt-left", GoBack, Some("Ply")),
                KeyBinding::new("backspace", GoBack, Some("Ply")),
                KeyBinding::new("alt-right", GoForward, Some("Ply")),
                KeyBinding::new("alt-up", GoUp, Some("Ply")),
                KeyBinding::new("alt-home", GoHome, Some("Ply")),
                KeyBinding::new("escape", Dismiss, Some("Ply")),
                KeyBinding::new("enter", Activate, Some("Ply")),
                KeyBinding::new("f2", BeginRename, Some("Ply")),
                KeyBinding::new("delete", DeleteSelection, Some("Ply")),
                KeyBinding::new("up", SelectUp, Some("Ply")),
                KeyBinding::new("down", SelectDown, Some("Ply")),
                KeyBinding::new("left", SelectLeft, Some("Ply")),
                KeyBinding::new("right", SelectRight, Some("Ply")),
                KeyBinding::new("shift-up", ExtendUp, Some("Ply")),
                KeyBinding::new("shift-down", ExtendDown, Some("Ply")),
                KeyBinding::new("shift-left", ExtendLeft, Some("Ply")),
                KeyBinding::new("shift-right", ExtendRight, Some("Ply")),
                KeyBinding::new("f5", Refresh, Some("Ply")),
                KeyBinding::new("ctrl-f", FocusFilter, Some("Ply")),
                KeyBinding::new("ctrl-c", CopySelectedPath, Some("Ply")),
                KeyBinding::new("up", MenuUp, Some("Ply")),
                KeyBinding::new("down", MenuDown, Some("Ply")),
                KeyBinding::new("left", MenuLeft, Some("Ply")),
                KeyBinding::new("right", MenuRight, Some("Ply")),
                KeyBinding::new("enter", MenuActivate, Some("Ply")),
            ]);

            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(gpui::Bounds {
                        origin: point(px(80.), px(80.)),
                        size: size(px(1280.), px(800.)),
                    })),
                    app_id: Some("app.ply.explorer".into()),
                    window_decorations: Some(gpui::WindowDecorations::Client),
                    titlebar: None,
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Ply::new(window, cx)),
            )
            .expect("failed to open window");
        });
}
