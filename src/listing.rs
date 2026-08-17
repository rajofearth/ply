use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Datelike, Local};

/// A file, directory, or symlink in the Current Folder.
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink { target: PathBuf },
}

impl Entry {
    pub fn is_directory(&self) -> bool {
        matches!(self.kind, EntryKind::Directory)
    }

    pub fn fingerprint(&self) -> EntryFingerprint {
        EntryFingerprint {
            name: self.name.clone(),
            kind: kind_discriminant(&self.kind),
            size: self.size,
            modified: self.modified,
            hidden: self.hidden,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryFingerprint {
    pub name: String,
    pub kind: u8,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub hidden: bool,
}

fn kind_discriminant(kind: &EntryKind) -> u8 {
    match kind {
        EntryKind::File => 0,
        EntryKind::Directory => 1,
        EntryKind::Symlink { .. } => 2,
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
    pub fingerprint: Vec<EntryFingerprint>,
}

impl Snapshot {
    pub fn from_entries(mut entries: Vec<Entry>) -> Self {
        entries.sort_by(|a, b| {
            b.is_directory()
                .cmp(&a.is_directory())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let fingerprint = entries.iter().map(Entry::fingerprint).collect();
        Self {
            entries,
            fingerprint,
        }
    }

}

/// Coarse kind used for icons — never derived by parsing [`kind_label`] text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindClass {
    Folder,
    Image,
    Video,
    Audio,
    Document,
    File,
}

/// Map an entry to its icon class from [`EntryKind`] / extension, not labels.
pub fn kind_class(entry: &Entry) -> KindClass {
    match entry.kind {
        EntryKind::Directory => return KindClass::Folder,
        EntryKind::Symlink { .. } | EntryKind::File => {}
    }
    kind_class_for_name(&entry.name)
}

fn kind_class_for_name(name: &str) -> KindClass {
    let ext = extension_lower(name);
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "ico" => KindClass::Image,
        "mp4" | "m4v" | "mkv" | "mov" | "avi" | "webm" | "wmv" => KindClass::Video,
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "m3u" | "m3u8" | "pls" => {
            KindClass::Audio
        }
        "txt" | "log" | "ini" | "cfg" | "toml" | "yaml" | "yml" | "md" | "json" | "xml"
        | "csv" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "pdf" | "rs" | "py"
        | "js" | "ts" | "tsx" | "jsx" | "c" | "cpp" | "h" | "go" | "java" | "sh" => {
            KindClass::Document
        }
        _ => KindClass::File,
    }
}

/// Lucide icon for an entry, via [`kind_class`] (not kind-label string matching).
pub fn entry_icon(entry: &Entry) -> crate::icons::Ico {
    use crate::icons::Ico;
    match kind_class(entry) {
        KindClass::Folder => Ico::Folder,
        KindClass::Image => Ico::Image,
        KindClass::Video => Ico::Video,
        KindClass::Audio => Ico::Music,
        KindClass::Document => Ico::FileText,
        KindClass::File => Ico::File,
    }
}

fn extension_lower(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Human-readable type for the Kind column, e.g. `"JPEG Image"`.
pub fn kind_label(entry: &Entry) -> &'static str {
    match entry.kind {
        EntryKind::Directory => return "Folder",
        EntryKind::Symlink { .. } => return "Shortcut",
        EntryKind::File => {}
    }
    // From the name, not the path: on a portable device the path component is
    // an opaque object ID and carries no extension.
    kind_label_for_name(&entry.name)
}

/// Kind from a bare file name / extension. Prefer [`kind_label`] when an
/// [`Entry`] is available so folders and shortcuts stay correct.
pub fn kind_label_for_name(name: &str) -> &'static str {
    match extension_lower(name).as_str() {
        "jpg" | "jpeg" => "JPEG Image",
        "png" => "PNG Image",
        "gif" => "GIF Image",
        "webp" => "WebP Image",
        "bmp" => "Bitmap Image",
        "svg" => "SVG Image",
        "ico" => "Icon",
        "mp4" | "m4v" => "MPEG Video",
        "mkv" => "Matroska Video",
        "mov" => "QuickTime Video",
        "avi" | "webm" | "wmv" => "Video",
        "mp3" => "MP3 Audio",
        "wav" => "Wave Audio",
        "flac" | "ogg" | "m4a" | "aac" => "Audio",
        "m3u" | "m3u8" | "pls" => "Playlist",
        "txt" | "log" | "ini" | "cfg" | "toml" | "yaml" | "yml" => "Text Document",
        "md" => "Markdown Document",
        "json" => "JSON Document",
        "xml" | "csv" => "Data Document",
        "doc" | "docx" => "Word Document",
        "xls" | "xlsx" => "Excel Workbook",
        "ppt" | "pptx" => "PowerPoint Presentation",
        "pdf" => "PDF Document",
        "zip" | "7z" | "rar" | "tar" | "gz" => "Archive",
        "exe" | "msi" => "Application",
        "dll" => "System File",
        "lnk" => "Shortcut",
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "cpp" | "h" | "go" | "java" | "sh" => {
            "Source File"
        }
        "" => "File",
        _ => "File",
    }
}

/// Leading `.` is always hidden. On Windows, also check `FILE_ATTRIBUTE_HIDDEN`
/// from `meta` when provided — avoids a second `symlink_metadata` per entry.
pub fn is_hidden(path: &Path, name: &str, meta: Option<&Metadata>) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if let Some(meta) = meta {
            return meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0;
        }
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            return meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0;
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (path, meta);
    }
    false
}

fn entry_from_dirent(dirent: std::fs::DirEntry) -> Option<Entry> {
    let path = dirent.path();
    let name = dirent.file_name().to_string_lossy().into_owned();
    // Prefer DirEntry::metadata: on Windows this reuses FindFirstFile data (no
    // re-stat). Falls back to symlink_metadata if the cached data is gone.
    let meta = dirent
        .metadata()
        .ok()
        .or_else(|| std::fs::symlink_metadata(&path).ok())?;
    let file_type = meta.file_type();
    let hidden = is_hidden(&path, &name, Some(&meta));
    let kind = if file_type.is_symlink() {
        let target = std::fs::read_link(&path).unwrap_or_default();
        EntryKind::Symlink { target }
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else {
        EntryKind::File
    };
    Some(Entry {
        path,
        name,
        kind,
        size: meta.len(),
        modified: meta.modified().ok(),
        hidden,
    })
}

/// Hidden entries are always skipped; Ply exposes no Show Hidden control.
fn collect_entries(path: &Path) -> anyhow::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for dirent in std::fs::read_dir(path)? {
        let Some(entry) = entry_from_dirent(dirent?) else {
            continue;
        };
        if !entry.hidden {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// Portable devices have no filesystem path, so those listings come from WPD.
fn entries_of(path: &Path) -> anyhow::Result<Vec<Entry>> {
    if crate::mtp::is_mtp(path) {
        return crate::mtp::list(path);
    }
    collect_entries(path)
}

pub fn list_dir(path: &Path) -> anyhow::Result<Snapshot> {
    Ok(Snapshot::from_entries(entries_of(path)?))
}

/// Direct subdirectories only. Does not recurse or follow symlinks.
pub fn list_dirs(path: &Path) -> anyhow::Result<Snapshot> {
    let dirs = entries_of(path)?
        .into_iter()
        .filter(Entry::is_directory)
        .collect();
    Ok(Snapshot::from_entries(dirs))
}

pub fn format_size(n: u64) -> String {
    const KB: f64 = 1024.0;
    let n = n as f64;
    if n < KB {
        format!("{n:.0} B")
    } else if n < KB * KB {
        format!("{:.1} KB", n / KB)
    } else if n < KB * KB * KB {
        format!("{:.1} MB", n / (KB * KB))
    } else {
        format!("{:.1} GB", n / (KB * KB * KB))
    }
}

/// Local-time stamp, shortened by recency: `Today, 14:02`, `Aug 12, 09:30`,
/// then `Aug 12, 2024` once the date is outside the current year.
///
/// Pass a shared `now` when formatting many rows in one paint so `Local::now`
/// runs once.
pub fn format_mtime(t: Option<SystemTime>, now: DateTime<Local>) -> String {
    let Some(t) = t else {
        return "—".into();
    };
    let when: DateTime<Local> = t.into();
    let today = now.date_naive();
    let date = when.date_naive();

    if date == today {
        when.format("Today, %H:%M").to_string()
    } else if (today - date).num_days() == 1 {
        when.format("Yesterday, %H:%M").to_string()
    } else if date.year() == today.year() {
        when.format("%b %-d, %H:%M").to_string()
    } else {
        when.format("%b %-d, %Y").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> Entry {
        Entry {
            path: PathBuf::from(name),
            name: name.into(),
            kind: EntryKind::File,
            size: 0,
            modified: None,
            hidden: false,
        }
    }

    #[test]
    fn kind_label_maps_extensions() {
        assert_eq!(kind_label(&file("a.JPG")), "JPEG Image");
        assert_eq!(kind_label(&file("a.mp4")), "MPEG Video");
        assert_eq!(kind_label(&file("a.m3u")), "Playlist");
        assert_eq!(kind_label(&file("a.docx")), "Word Document");
        assert_eq!(kind_label(&file("a.unknown")), "File");
        assert_eq!(kind_label(&file("noext")), "File");
    }

    #[test]
    fn kind_label_uses_entry_kind_first() {
        let mut dir = file("Pictures.jpg");
        dir.kind = EntryKind::Directory;
        assert_eq!(kind_label(&dir), "Folder");
    }

    #[test]
    fn kind_class_from_extension_not_label() {
        assert_eq!(kind_class(&file("a.JPG")), KindClass::Image);
        assert_eq!(kind_class(&file("a.mp4")), KindClass::Video);
        assert_eq!(kind_class(&file("a.m3u")), KindClass::Audio);
        assert_eq!(kind_class(&file("a.docx")), KindClass::Document);
        assert_eq!(kind_class(&file("a.rs")), KindClass::Document);
        assert_eq!(kind_class(&file("a.zip")), KindClass::File);
        let mut dir = file("Pictures.jpg");
        dir.kind = EntryKind::Directory;
        assert_eq!(kind_class(&dir), KindClass::Folder);
    }

    #[test]
    fn format_size_scales() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2048), "2.0 KB");
    }

    #[test]
    fn is_hidden_leading_dot() {
        assert!(is_hidden(Path::new("."), ".gitignore", None));
        assert!(is_hidden(Path::new("."), ".hidden", None));
        assert!(!is_hidden(Path::new("."), "visible.txt", None));
    }

    #[test]
    fn list_dir_skips_dotfiles() {
        let dir = std::env::temp_dir().join(format!("ply-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("visible.txt"), b"a").unwrap();
        std::fs::write(dir.join(".hidden"), b"b").unwrap();
        let snap = list_dir(&dir).unwrap();
        let names: Vec<_> = snap.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"visible.txt"));
        assert!(!names.contains(&".hidden"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn is_hidden_uses_file_attributes_from_meta() {
        use std::process::Command;

        let dir = std::env::temp_dir().join(format!("ply-attr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret.txt");
        std::fs::write(&path, b"x").unwrap();
        assert!(
            Command::new("attrib")
                .args(["+H", &path.to_string_lossy()])
                .status()
                .unwrap()
                .success()
        );
        let meta = std::fs::symlink_metadata(&path).unwrap();
        assert!(is_hidden(&path, "secret.txt", Some(&meta)));
        // Listing should also exclude it without a second independent path.
        let snap = list_dir(&dir).unwrap();
        assert!(!snap.entries.iter().any(|e| e.name == "secret.txt"));
        let _ = Command::new("attrib")
            .args(["-H", &path.to_string_lossy()])
            .status();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
