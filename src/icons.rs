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
        _ => return None,
    })
}

/// Serves Ply's vendored lucide icons only (no gpui-component asset pack).
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(icon_bytes(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Ico::ALL
            .iter()
            .map(|ico| ico.path())
            .filter(|p| p.starts_with(path))
            .map(SharedString::from)
            .collect())
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
            assert!(
                !bytes.is_empty(),
                "{}: empty asset",
                ico.path()
            );
            let head = std::str::from_utf8(&bytes[..bytes.len().min(64)]).unwrap_or("");
            assert!(
                head.contains("<svg"),
                "{}: not an SVG (head={head:?})",
                ico.path()
            );
        }
    }
}
