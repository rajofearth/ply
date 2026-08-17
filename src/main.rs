#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod budget;
mod file_clip;
mod fs_ops;
mod icons;
mod listing;
mod mtp;
mod open_with;
mod path_caps;
mod preview;
mod theme;
mod ui;
mod volumes;
mod watch;

use gpui::{AppContext, KeyBinding};

use app::{Ply, window_options};
use ui::{
    Activate, BeginRename, CloseTab, CopySelectedPath, CopySelection, CutSelection,
    DeleteSelection, Dismiss, ExtendDown, ExtendUp, FocusFilter, GoBack, GoForward, GoHome, GoUp,
    NewFolder, NewTab, NewWindow, PasteFiles, QuickLook, Refresh, SelectDown, SelectUp,
    ToggleTheme,
};

fn keybindings() -> Vec<KeyBinding> {
    let mut keys = vec![
        KeyBinding::new("d", ToggleTheme, Some("Ply")),
        KeyBinding::new("escape", Dismiss, Some("Ply")),
        KeyBinding::new("space", QuickLook, Some("Ply")),
        KeyBinding::new("up", SelectUp, Some("Ply")),
        KeyBinding::new("down", SelectDown, Some("Ply")),
        KeyBinding::new("shift-up", ExtendUp, Some("Ply")),
        KeyBinding::new("shift-down", ExtendDown, Some("Ply")),
        KeyBinding::new("backspace", GoBack, Some("Ply")),
    ];
    #[cfg(target_os = "macos")]
    {
        keys.extend([
            KeyBinding::new("cmd-o", Activate, Some("Ply")),
            KeyBinding::new("cmd-down", Activate, Some("Ply")),
            KeyBinding::new("enter", BeginRename, Some("Ply")),
            KeyBinding::new("cmd-x", CutSelection, Some("Ply")),
            KeyBinding::new("cmd-c", CopySelection, Some("Ply")),
            KeyBinding::new("cmd-v", PasteFiles, Some("Ply")),
            KeyBinding::new("cmd-shift-c", CopySelectedPath, Some("Ply")),
            KeyBinding::new("cmd-backspace", DeleteSelection, Some("Ply")),
            KeyBinding::new("cmd-shift-n", NewFolder, Some("Ply")),
            KeyBinding::new("cmd-t", NewTab, Some("Ply")),
            KeyBinding::new("cmd-w", CloseTab, Some("Ply")),
            KeyBinding::new("cmd-n", NewWindow, Some("Ply")),
            KeyBinding::new("cmd-r", Refresh, Some("Ply")),
            KeyBinding::new("cmd-f", FocusFilter, Some("Ply")),
            KeyBinding::new("cmd-[", GoBack, Some("Ply")),
            KeyBinding::new("cmd-]", GoForward, Some("Ply")),
            KeyBinding::new("cmd-up", GoUp, Some("Ply")),
            KeyBinding::new("cmd-shift-h", GoHome, Some("Ply")),
        ]);
    }
    #[cfg(not(target_os = "macos"))]
    {
        keys.extend([
            KeyBinding::new("enter", Activate, Some("Ply")),
            KeyBinding::new("f2", BeginRename, Some("Ply")),
            KeyBinding::new("ctrl-x", CutSelection, Some("Ply")),
            KeyBinding::new("ctrl-c", CopySelection, Some("Ply")),
            KeyBinding::new("ctrl-v", PasteFiles, Some("Ply")),
            KeyBinding::new("ctrl-shift-c", CopySelectedPath, Some("Ply")),
            KeyBinding::new("delete", DeleteSelection, Some("Ply")),
            KeyBinding::new("ctrl-shift-n", NewFolder, Some("Ply")),
            KeyBinding::new("ctrl-t", NewTab, Some("Ply")),
            KeyBinding::new("ctrl-w", CloseTab, Some("Ply")),
            KeyBinding::new("ctrl-n", NewWindow, Some("Ply")),
            KeyBinding::new("f5", Refresh, Some("Ply")),
            KeyBinding::new("ctrl-f", FocusFilter, Some("Ply")),
            KeyBinding::new("alt-left", GoBack, Some("Ply")),
            KeyBinding::new("alt-right", GoForward, Some("Ply")),
            KeyBinding::new("alt-up", GoUp, Some("Ply")),
            KeyBinding::new("alt-home", GoHome, Some("Ply")),
        ]);
    }
    keys
}

fn main() {
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(|cx| {
            gpui_component::init(cx);
            cx.bind_keys(keybindings());
            cx.open_window(window_options(), |window, cx| {
                cx.new(|cx| Ply::new(window, cx))
            })
            .expect("failed to open window");
        });
}
