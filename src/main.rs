#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod fs_ops;
mod icons;
mod listing;
mod mtp;
mod path_caps;
mod theme;
mod ui;
mod volumes;
mod watch;

use gpui::{AppContext, KeyBinding, WindowBounds, WindowOptions, point, px, size};

use app::Ply;
use ui::{
    Activate, BeginRename, CopySelectedPath, DeleteSelection, Dismiss, ExtendDown, ExtendUp,
    FocusFilter, GoBack, GoForward, GoHome, GoUp, Refresh, SelectDown, SelectUp, ToggleTheme,
};

fn main() {
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(|cx| {
            gpui_component::init(cx);

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
                KeyBinding::new("shift-up", ExtendUp, Some("Ply")),
                KeyBinding::new("shift-down", ExtendDown, Some("Ply")),
                KeyBinding::new("f5", Refresh, Some("Ply")),
                KeyBinding::new("ctrl-f", FocusFilter, Some("Ply")),
                KeyBinding::new("ctrl-c", CopySelectedPath, Some("Ply")),
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
