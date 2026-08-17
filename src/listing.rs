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

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Size,
    Kind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SortDir {
    #[default]
    Asc,
    Desc,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SortSpec {
    pub key: SortKey,
    pub dir: SortDir,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ViewMode {
    #[default]
    List,
    Grid,
    Column,
}

impl Snapshot {
    pub fn from_entries(entries: Vec<Entry>) -> Self {
        Self::from_entries_sorted(entries, SortSpec::default())
    }

    pub fn from_entries_sorted(mut entries: Vec<Entry>, spec: SortSpec) -> Self {
        entries.sort_by(|a, b| compare_entries(a, b, spec));
        let fingerprint = entries.iter().map(Entry::fingerprint).collect();
        Self {
            entries,
            fingerprint,
        }
    }

    pub fn resort(&mut self, spec: SortSpec) {
        self.entries.sort_by(|a, b| compare_entries(a, b, spec));
    }
}

fn compare_entries(a: &Entry, b: &Entry, spec: SortSpec) -> std::cmp::Ordering {
    let dir_ord = b.is_directory().cmp(&a.is_directory());
    if dir_ord != std::cmp::Ordering::Equal {
        return dir_ord;
    }
    let primary = match spec.key {
        SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        SortKey::Size => a.size.cmp(&b.size),
        SortKey::Modified => a.modified.cmp(&b.modified),
        SortKey::Kind => kind_label(a).cmp(kind_label(b)),
    };
    match spec.dir {
        SortDir::Asc => primary,
        SortDir::Desc => primary.reverse(),
    }
}

/// Best-effort MIME type from the file name. Used when xdg-mime is missing.
pub fn mime_guess(path: &Path) -> String {
    let ext = Path::new(path.file_name().and_then(|n| n.to_str()).unwrap_or(""))
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" => "text/plain",
        "html" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "rs" | "py" | "js" | "ts" | "c" | "h" | "go" => "text/plain",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "exe" | "msi" => "application/vnd.microsoft.portable-executable",
        _ => "application/octet-stream",
    }
    .into()
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
    let ext = Path::new(&entry.name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
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

pub fn is_hidden(path: &Path, name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            return meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0;
        }
    }
    #[cfg(not(windows))]
    let _ = path;
    false
}

fn entry_from_dirent(dirent: std::fs::DirEntry) -> Option<Entry> {
    let path = dirent.path();
    let name = dirent.file_name().to_string_lossy().into_owned();
    let file_type = dirent
        .file_type()
        .ok()
        .or_else(|| std::fs::symlink_metadata(&path).ok().map(|m| m.file_type()))?;
    let meta = std::fs::symlink_metadata(&path).ok()?;
    let hidden = is_hidden(&path, &name);
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
pub fn format_mtime(t: Option<SystemTime>) -> String {
    let Some(t) = t else {
        return "—".into();
    };
    let when: DateTime<Local> = t.into();
    let now = Local::now();
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
    fn format_size_scales() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2048), "2.0 KB");
    }

    #[test]
    fn sort_keeps_directories_first() {
        let entries = vec![
            file("b.txt"),
            Entry {
                path: PathBuf::from("z"),
                name: "z".into(),
                kind: EntryKind::Directory,
                size: 0,
                modified: None,
                hidden: false,
            },
            file("a.txt"),
        ];
        let snap = Snapshot::from_entries_sorted(
            entries,
            SortSpec {
                key: SortKey::Name,
                dir: SortDir::Asc,
            },
        );
        assert!(snap.entries[0].is_directory());
        assert_eq!(snap.entries[1].name, "a.txt");
        assert_eq!(snap.entries[2].name, "b.txt");
    }
}
