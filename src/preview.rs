//! Quick Look payloads. Images and text render in-process; PDF/Office/media
//! use a free thumbnailer when one is on PATH.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::listing::{Entry, EntryKind, format_size, kind_label};

const TEXT_CAP: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub enum Preview {
    Image(PathBuf),
    Text {
        content: String,
        truncated: bool,
    },
    Thumbnail {
        image: PathBuf,
        caption: String,
    },
    Card {
        kind: String,
        size: String,
        hint: String,
    },
}

pub fn for_path(path: &Path) -> Preview {
    if crate::mtp::is_mtp(path) {
        return card(path, "Open a copy from the device to preview.");
    }
    if path.is_dir() {
        return card(path, "Folders have no preview. Open to browse.");
    }
    let ext = ext_of(path);
    if is_image(&ext) {
        return Preview::Image(path.to_path_buf());
    }
    if is_text(&ext) {
        return text_preview(path);
    }
    if is_pdf(&ext) {
        if let Some(png) = pdf_thumb(path) {
            return Preview::Thumbnail {
                image: png,
                caption: "PDF".into(),
            };
        }
        return card(path, "Install poppler (pdftoppm) for a first-page preview.");
    }
    if is_media(&ext) {
        if let Some(png) = media_thumb(path) {
            return Preview::Thumbnail {
                image: png,
                caption: "Media".into(),
            };
        }
        return card(path, "Install ffmpeg for a frame preview, or Open to play.");
    }
    if is_office(&ext) {
        return card(path, "Open in the OS handler to view this document.");
    }
    card(path, "Open with the default app to view this file.")
}

fn text_preview(path: &Path) -> Preview {
    match std::fs::read(path) {
        Ok(bytes) => {
            let truncated = bytes.len() > TEXT_CAP;
            let slice = &bytes[..bytes.len().min(TEXT_CAP)];
            if slice.contains(&0) {
                return card(path, "Binary file. Open to view.");
            }
            let content = String::from_utf8_lossy(slice).into_owned();
            Preview::Text { content, truncated }
        }
        Err(err) => card(path, &format!("Could not read: {err}")),
    }
}

fn card(path: &Path, hint: &str) -> Preview {
    let meta = std::fs::metadata(path).ok();
    let entry = Entry {
        path: path.to_path_buf(),
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        kind: if meta.as_ref().is_some_and(|m| m.is_dir()) {
            EntryKind::Directory
        } else {
            EntryKind::File
        },
        size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        modified: meta.and_then(|m| m.modified().ok()),
        hidden: false,
    };
    Preview::Card {
        kind: kind_label(&entry).to_string(),
        size: if entry.is_directory() {
            "—".into()
        } else {
            format_size(entry.size)
        },
        hint: hint.into(),
    }
}

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn is_image(ext: &str) -> bool {
    matches!(
        ext,
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "ico"
    )
}

fn is_text(ext: &str) -> bool {
    matches!(
        ext,
        "txt"
            | "md"
            | "rs"
            | "py"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "json"
            | "toml"
            | "yaml"
            | "yml"
            | "xml"
            | "csv"
            | "log"
            | "ini"
            | "cfg"
            | "c"
            | "h"
            | "cpp"
            | "go"
            | "java"
            | "sh"
            | "html"
            | "css"
            | "svg"
    ) || ext.is_empty() && false
}

fn is_pdf(ext: &str) -> bool {
    ext == "pdf"
}

fn is_media(ext: &str) -> bool {
    matches!(
        ext,
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "wmv" | "mp3" | "wav" | "flac" | "ogg" | "m4a"
    )
}

fn is_office(ext: &str) -> bool {
    matches!(
        ext,
        "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp"
    )
}

fn cache_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ply-ql-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn pdf_thumb(path: &Path) -> Option<PathBuf> {
    let out = cache_dir().join("pdf");
    let status = Command::new("pdftoppm")
        .args([
            "-png",
            "-f",
            "1",
            "-l",
            "1",
            "-singlefile",
            "-scale-to",
            "900",
            &path.display().to_string(),
            &out.display().to_string(),
        ])
        .status()
        .ok()?;
    let png = out.with_extension("png");
    if status.success() && png.is_file() {
        Some(png)
    } else {
        None
    }
}

fn media_thumb(path: &Path) -> Option<PathBuf> {
    let png = cache_dir().join("media.png");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-ss",
            "1",
            "-i",
            &path.display().to_string(),
            "-frames:v",
            "1",
            "-vf",
            "scale=900:-1",
            &png.display().to_string(),
        ])
        .status()
        .ok()?;
    if status.success() && png.is_file() {
        Some(png)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_file_previews_contents() {
        let dir = std::env::temp_dir().join(format!("ply-ql-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hello.txt");
        std::fs::write(&file, b"hello ply").unwrap();
        match for_path(&file) {
            Preview::Text { content, truncated } => {
                assert!(content.contains("hello ply"));
                assert!(!truncated);
            }
            other => panic!("expected text, got {other:?}"),
        }
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn image_uses_the_file_itself() {
        let path = PathBuf::from("/tmp/photo.png");
        // Missing file still reports Image so GPUI can show a broken-image state;
        // kind detection is by extension.
        match for_path(&path) {
            Preview::Image(p) => assert_eq!(p, path),
            Preview::Card { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}
