use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// Every lucide icon used by the UI.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ico {
    ChevronRight,
    ChevronDown,
    Folder,
    Image,
    Music,
    Video,
    HardDrive,
    Search,
    LayoutGrid,
    List,
    Columns,
    Home,
    ArrowLeft,
    ArrowRight,
    Sun,
    Moon,
    Minus,
    Square,
    X,
    Usb,
    Network,
    File,
    FileText,
    Scissors,
    Copy,
    ClipboardPaste,
    Pencil,
    Trash,
    Terminal,
    FilePlus,
    FolderPlus,
    Refresh,
    Info,
    Shield,
    Pin,
    PinOff,
    ExternalLink,
    AppWindow,
    Plus,
}

impl Ico {
    /// Asset path, e.g. `"icons/chevron-right.svg"`.
    pub fn path(self) -> &'static str {
        match self {
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::Folder => "icons/folder.svg",
            Self::Image => "icons/image.svg",
            Self::Music => "icons/music.svg",
            Self::Video => "icons/video.svg",
            Self::HardDrive => "icons/hard-drive.svg",
            Self::Search => "icons/search.svg",
            Self::LayoutGrid => "icons/layout-grid.svg",
            Self::List => "icons/list.svg",
            Self::Columns => "icons/columns.svg",
            Self::Home => "icons/home.svg",
            Self::ArrowLeft => "icons/arrow-left.svg",
            Self::ArrowRight => "icons/arrow-right.svg",
            Self::Sun => "icons/sun.svg",
            Self::Moon => "icons/moon.svg",
            Self::Minus => "icons/minus.svg",
            Self::Square => "icons/square.svg",
            Self::X => "icons/x.svg",
            Self::Usb => "icons/usb.svg",
            Self::Network => "icons/network.svg",
            Self::File => "icons/file.svg",
            Self::FileText => "icons/file-text.svg",
            Self::Scissors => "icons/scissors.svg",
            Self::Copy => "icons/copy.svg",
            Self::ClipboardPaste => "icons/clipboard-paste.svg",
            Self::Pencil => "icons/pencil.svg",
            Self::Trash => "icons/trash.svg",
            Self::Terminal => "icons/terminal.svg",
            Self::FilePlus => "icons/file-plus.svg",
            Self::FolderPlus => "icons/folder-plus.svg",
            Self::Refresh => "icons/refresh.svg",
            Self::Info => "icons/info.svg",
            Self::Shield => "icons/shield.svg",
            Self::Pin => "icons/pin.svg",
            Self::PinOff => "icons/pin-off.svg",
            Self::ExternalLink => "icons/external-link.svg",
            Self::AppWindow => "icons/app-window.svg",
            Self::Plus => "icons/plus.svg",
        }
    }

    const ALL: &'static [Ico] = &[
        Self::ChevronRight,
        Self::ChevronDown,
        Self::Folder,
        Self::Image,
        Self::Music,
        Self::Video,
        Self::HardDrive,
        Self::Search,
        Self::LayoutGrid,
        Self::List,
        Self::Columns,
        Self::Home,
        Self::ArrowLeft,
        Self::ArrowRight,
        Self::Sun,
        Self::Moon,
        Self::Minus,
        Self::Square,
        Self::X,
        Self::Usb,
        Self::Network,
        Self::File,
        Self::FileText,
        Self::Scissors,
        Self::Copy,
        Self::ClipboardPaste,
        Self::Pencil,
        Self::Trash,
        Self::Terminal,
        Self::FilePlus,
        Self::FolderPlus,
        Self::Refresh,
        Self::Info,
        Self::Shield,
        Self::Pin,
        Self::PinOff,
        Self::ExternalLink,
        Self::AppWindow,
        Self::Plus,
    ];
}

fn icon_bytes(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        "icons/chevron-right.svg" => include_bytes!("../assets/icons/chevron-right.svg"),
        "icons/chevron-down.svg" => include_bytes!("../assets/icons/chevron-down.svg"),
        "icons/folder.svg" => include_bytes!("../assets/icons/folder.svg"),
        "icons/image.svg" => include_bytes!("../assets/icons/image.svg"),
        "icons/music.svg" => include_bytes!("../assets/icons/music.svg"),
        "icons/video.svg" => include_bytes!("../assets/icons/video.svg"),
        "icons/hard-drive.svg" => include_bytes!("../assets/icons/hard-drive.svg"),
        "icons/search.svg" => include_bytes!("../assets/icons/search.svg"),
        "icons/layout-grid.svg" => include_bytes!("../assets/icons/layout-grid.svg"),
        "icons/list.svg" => include_bytes!("../assets/icons/list.svg"),
        "icons/columns.svg" => include_bytes!("../assets/icons/columns.svg"),
        "icons/home.svg" => include_bytes!("../assets/icons/home.svg"),
        "icons/arrow-left.svg" => include_bytes!("../assets/icons/arrow-left.svg"),
        "icons/arrow-right.svg" => include_bytes!("../assets/icons/arrow-right.svg"),
        "icons/sun.svg" => include_bytes!("../assets/icons/sun.svg"),
        "icons/moon.svg" => include_bytes!("../assets/icons/moon.svg"),
        "icons/minus.svg" => include_bytes!("../assets/icons/minus.svg"),
        "icons/square.svg" => include_bytes!("../assets/icons/square.svg"),
        "icons/x.svg" => include_bytes!("../assets/icons/x.svg"),
        "icons/usb.svg" => include_bytes!("../assets/icons/usb.svg"),
        "icons/network.svg" => include_bytes!("../assets/icons/network.svg"),
        "icons/file.svg" => include_bytes!("../assets/icons/file.svg"),
        "icons/file-text.svg" => include_bytes!("../assets/icons/file-text.svg"),
        "icons/scissors.svg" => include_bytes!("../assets/icons/scissors.svg"),
        "icons/copy.svg" => include_bytes!("../assets/icons/copy.svg"),
        "icons/clipboard-paste.svg" => include_bytes!("../assets/icons/clipboard-paste.svg"),
        "icons/pencil.svg" => include_bytes!("../assets/icons/pencil.svg"),
        "icons/trash.svg" => include_bytes!("../assets/icons/trash.svg"),
        "icons/terminal.svg" => include_bytes!("../assets/icons/terminal.svg"),
        "icons/file-plus.svg" => include_bytes!("../assets/icons/file-plus.svg"),
        "icons/folder-plus.svg" => include_bytes!("../assets/icons/folder-plus.svg"),
        "icons/refresh.svg" => include_bytes!("../assets/icons/refresh.svg"),
        "icons/info.svg" => include_bytes!("../assets/icons/info.svg"),
        "icons/shield.svg" => include_bytes!("../assets/icons/shield.svg"),
        "icons/pin.svg" => include_bytes!("../assets/icons/pin.svg"),
        "icons/pin-off.svg" => include_bytes!("../assets/icons/pin-off.svg"),
        "icons/external-link.svg" => include_bytes!("../assets/icons/external-link.svg"),
        "icons/app-window.svg" => include_bytes!("../assets/icons/app-window.svg"),
        "icons/plus.svg" => include_bytes!("../assets/icons/plus.svg"),
        _ => return None,
    })
}

/// Serves Ply's vendored lucide icons, falling back to gpui-component's assets.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = icon_bytes(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths: Vec<SharedString> = Ico::ALL
            .iter()
            .map(|ico| ico.path())
            .filter(|p| p.starts_with(path))
            .map(SharedString::from)
            .collect();

        for p in gpui_component_assets::Assets.list(path)? {
            if !paths.iter().any(|ours| ours.as_ref() == p.as_ref()) {
                paths.push(p);
            }
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ico_loads_nonempty_bytes() {
        for ico in Ico::ALL {
            let bytes = Assets
                .load(ico.path())
                .unwrap_or_else(|e| panic!("{}: load error: {e}", ico.path()))
                .unwrap_or_else(|| panic!("{}: missing asset", ico.path()));
            assert!(!bytes.is_empty(), "{}: empty asset", ico.path());
            let head = std::str::from_utf8(&bytes[..bytes.len().min(64)]).unwrap_or("");
            assert!(
                head.contains("<svg"),
                "{}: not an SVG (head={head:?})",
                ico.path()
            );
        }
    }
}
