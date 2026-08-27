use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($name:ident => $file:literal),+ $(,)?) => {
        /// Every lucide icon used by the UI.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Ico {
            $($name,)+
        }

        impl Ico {
            /// Asset path, e.g. `"icons/chevron-right.svg"`.
            pub fn path(self) -> &'static str {
                match self {
                    $(Self::$name => concat!("icons/", $file),)+
                }
            }

            const ALL: &'static [Ico] = &[$(Self::$name,)+];
        }

        fn icon_bytes(path: &str) -> Option<&'static [u8]> {
            Some(match path {
                $(concat!("icons/", $file) => {
                    include_bytes!(concat!("../assets/icons/", $file))
                })+
                _ => return None,
            })
        }
    };
}

icons! {
    ChevronRight => "chevron-right.svg",
    ChevronDown => "chevron-down.svg",
    Folder => "folder.svg",
    Image => "image.svg",
    Music => "music.svg",
    Video => "video.svg",
    HardDrive => "hard-drive.svg",
    Search => "search.svg",
    LayoutGrid => "layout-grid.svg",
    List => "list.svg",
    Home => "home.svg",
    ArrowLeft => "arrow-left.svg",
    ArrowRight => "arrow-right.svg",
    Sun => "sun.svg",
    Moon => "moon.svg",
    Minus => "minus.svg",
    Square => "square.svg",
    X => "x.svg",
    Usb => "usb.svg",
    Network => "network.svg",
    File => "file.svg",
    FileText => "file-text.svg",
    Scissors => "scissors.svg",
    Copy => "copy.svg",
    Pencil => "pencil.svg",
    Trash => "trash.svg",
    Pin => "pin.svg",
    PinOff => "pin-off.svg",
    Info => "info.svg",
    FolderPlus => "folder-plus.svg",
    ClipboardPaste => "clipboard-paste.svg",
    Terminal => "terminal.svg",
    Shield => "shield.svg",
    ExternalLink => "external-link.svg",
    Refresh => "refresh.svg",
    ArrowUpDown => "arrow-up-down.svg",
    Check => "check.svg",
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
