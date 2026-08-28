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
}

/// Truncate a string from the middle, keeping at most `max` total chars with an
/// ellipsis in the middle so both ends of a long name stay readable.
pub fn truncate_middle(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    // Keep at least one char each side of the ellipsis; never underflows.
    let keep = max.saturating_sub(1) / 2;
    let head: String = s.chars().take(keep).collect();
    let tail: String = s
        .chars()
        .rev()
        .take(keep)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}…{tail}")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SortKey {
    Name,
    #[default]
    Modified,
    Kind,
    Size,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
}

impl Snapshot {
    #[cfg(test)]
    pub fn from_entries(entries: Vec<Entry>) -> Self {
        Self::sorted(entries, SortKey::default())
    }

    pub fn sorted(mut entries: Vec<Entry>, key: SortKey) -> Self {
        entries.sort_by(|a, b| compare_entries(a, b, key));
        Self { entries }
    }

    pub fn resort(&mut self, key: SortKey) {
        self.entries.sort_by(|a, b| compare_entries(a, b, key));
    }

    /// Order-independent equality so a resorted listing is not a "change".
    /// Same-order listings (the usual watch reload) compare in place with no extra alloc.
    pub fn same_contents(&self, other: &Self) -> bool {
        let a = &self.entries;
        let b = &other.entries;
        if a.len() != b.len() {
            return false;
        }
        if a.iter().zip(b).all(|(x, y)| entry_meta_eq(x, y)) {
            return true;
        }
        let mut ia: Vec<_> = (0..a.len()).collect();
        let mut ib: Vec<_> = (0..b.len()).collect();
        ia.sort_unstable_by(|&i, &j| cmp_name(&a[i].name, &a[j].name));
        ib.sort_unstable_by(|&i, &j| cmp_name(&b[i].name, &b[j].name));
        ia.iter()
            .zip(&ib)
            .all(|(&i, &j)| entry_meta_eq(&a[i], &b[j]))
    }
}

fn entry_meta_eq(a: &Entry, b: &Entry) -> bool {
    names_eq(&a.name, &b.name)
        && a.kind == b.kind
        && a.size == b.size
        && a.modified == b.modified
        && a.hidden == b.hidden
}

fn names_eq(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        a.eq_ignore_ascii_case(b)
    } else {
        a.to_lowercase() == b.to_lowercase()
    }
}

/// ASCII names compare without allocating. Non-ASCII still Unicode-casefolds.
fn cmp_name(a: &str, b: &str) -> std::cmp::Ordering {
    if a.is_ascii() && b.is_ascii() {
        a.bytes()
            .map(|b| b.to_ascii_lowercase())
            .cmp(b.bytes().map(|b| b.to_ascii_lowercase()))
    } else {
        a.to_lowercase().cmp(&b.to_lowercase())
    }
}

fn compare_entries(a: &Entry, b: &Entry, key: SortKey) -> std::cmp::Ordering {
    b.is_directory()
        .cmp(&a.is_directory())
        .then_with(|| match key {
            SortKey::Name => cmp_name(&a.name, &b.name),
            SortKey::Modified => b.modified.cmp(&a.modified),
            SortKey::Kind => kind_label(a)
                .cmp(kind_label(b))
                .then_with(|| cmp_name(&a.name, &b.name)),
            SortKey::Size => b.size.cmp(&a.size).then_with(|| cmp_name(&a.name, &b.name)),
        })
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

/// Whether an entry carries its own shell icon — an executable (.exe/.msi) or
/// a Windows shortcut (.lnk). These resolve to a real per-file icon via
/// `IShellItemImageFactory` (the same path Explorer uses), rather than a
/// generic glyph.
pub fn is_executable_or_shortcut(entry: &Entry) -> bool {
    let mut buf = [0u8; 8];
    matches!(
        extension_lower(&entry.name, &mut buf),
        "exe" | "msi" | "lnk"
    )
}

/// Map an entry to its icon class from [`EntryKind`] / extension, not labels.
pub fn kind_class(entry: &Entry) -> KindClass {
    match entry.kind {
        EntryKind::Directory => KindClass::Folder,
        EntryKind::Symlink { .. } | EntryKind::File => classify_name(&entry.name).0,
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

fn extension_lower<'a>(name: &'a str, buf: &'a mut [u8; 8]) -> &'a str {
    let Some(raw) = Path::new(name).extension().and_then(|e| e.to_str()) else {
        return "";
    };
    if raw.len() > buf.len() {
        return "";
    }
    for (i, b) in raw.bytes().enumerate() {
        buf[i] = b.to_ascii_lowercase();
    }
    std::str::from_utf8(&buf[..raw.len()]).unwrap_or("")
}

/// Human-readable type for the Kind column, e.g. `"JPEG Image"`.
pub fn kind_label(entry: &Entry) -> &'static str {
    match entry.kind {
        EntryKind::Directory => "Folder",
        EntryKind::Symlink { .. } => "Shortcut",
        EntryKind::File => classify_name(&entry.name).1,
    }
}

/// Kind from a bare file name / extension. Prefer [`kind_label`] when an
/// [`Entry`] is available so folders and shortcuts stay correct.
pub fn kind_label_for_name(name: &str) -> &'static str {
    classify_name(name).1
}

fn classify_name(name: &str) -> (KindClass, &'static str) {
    let mut buf = [0u8; 8];
    match extension_lower(name, &mut buf) {
        "jpg" | "jpeg" => (KindClass::Image, "JPEG Image"),
        "png" => (KindClass::Image, "PNG Image"),
        "gif" => (KindClass::Image, "GIF Image"),
        "webp" => (KindClass::Image, "WebP Image"),
        "bmp" => (KindClass::Image, "Bitmap Image"),
        "svg" => (KindClass::Image, "SVG Image"),
        "ico" => (KindClass::Image, "Icon"),
        "mp4" | "m4v" => (KindClass::Video, "MPEG Video"),
        "mkv" => (KindClass::Video, "Matroska Video"),
        "mov" => (KindClass::Video, "QuickTime Video"),
        "avi" | "webm" | "wmv" => (KindClass::Video, "Video"),
        "mp3" => (KindClass::Audio, "MP3 Audio"),
        "wav" => (KindClass::Audio, "Wave Audio"),
        "flac" | "ogg" | "m4a" | "aac" => (KindClass::Audio, "Audio"),
        "m3u" | "m3u8" | "pls" => (KindClass::Audio, "Playlist"),
        "txt" | "log" | "ini" | "cfg" | "toml" | "yaml" | "yml" => {
            (KindClass::Document, "Text Document")
        }
        "md" => (KindClass::Document, "Markdown Document"),
        "json" => (KindClass::Document, "JSON Document"),
        "xml" | "csv" => (KindClass::Document, "Data Document"),
        "doc" | "docx" => (KindClass::Document, "Word Document"),
        "xls" | "xlsx" => (KindClass::Document, "Excel Workbook"),
        "ppt" | "pptx" => (KindClass::Document, "PowerPoint Presentation"),
        "pdf" => (KindClass::Document, "PDF Document"),
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "cpp" | "h" | "go" | "java" | "sh" => {
            (KindClass::Document, "Source File")
        }
        "zip" | "7z" | "rar" | "tar" | "gz" => (KindClass::File, "Archive"),
        "exe" | "msi" => (KindClass::File, "Application"),
        "dll" => (KindClass::File, "System File"),
        "lnk" => (KindClass::File, "Shortcut"),
        _ => (KindClass::File, "File"),
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
/// The Recycle Bin is a virtual shell namespace with no path either.
fn entries_of(path: &Path) -> anyhow::Result<Vec<Entry>> {
    if crate::recycle_bin::is_recycle_bin(path) {
        return crate::recycle_bin::list();
    }
    if crate::mtp::is_mtp(path) {
        return crate::mtp::list(path);
    }
    collect_entries(path)
}

pub fn list_sorted(path: &Path, key: SortKey) -> anyhow::Result<Snapshot> {
    Ok(Snapshot::sorted(entries_of(path)?, key))
}

/// Direct subdirectories only. Does not recurse or follow symlinks.
pub fn list_dirs(path: &Path) -> anyhow::Result<Snapshot> {
    let dirs = entries_of(path)?
        .into_iter()
        .filter(Entry::is_directory)
        .collect();
    Ok(Snapshot::sorted(dirs, SortKey::Name))
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
        assert_eq!(kind_label(&file("a.zip")), "Archive");
        assert_eq!(kind_label(&file("setup.EXE")), "Application");
        assert_eq!(kind_label(&file("ntdll.dll")), "System File");
        assert_eq!(kind_label(&file("link.lnk")), "Shortcut");
    }

    #[test]
    fn kind_label_uses_entry_kind_first() {
        let mut dir = file("Pictures.jpg");
        dir.kind = EntryKind::Directory;
        assert_eq!(kind_label(&dir), "Folder");
    }

    #[test]
    fn is_executable_or_shortcut_detects_common_types() {
        assert!(is_executable_or_shortcut(&file("setup.exe")));
        assert!(is_executable_or_shortcut(&file("installer.msi")));
        assert!(is_executable_or_shortcut(&file("App.lnk")));
        assert!(!is_executable_or_shortcut(&file("photo.jpg")));
        assert!(!is_executable_or_shortcut(&file("doc.txt")));
        assert!(!is_executable_or_shortcut(&file("noext")));
        let mut dir = file("App.exe");
        dir.kind = EntryKind::Directory;
        assert!(is_executable_or_shortcut(&dir), "shortcut check is name-based");
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
        let snap = list_sorted(&dir, SortKey::default()).unwrap();
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
        let snap = list_sorted(&dir, SortKey::default()).unwrap();
        assert!(!snap.entries.iter().any(|e| e.name == "secret.txt"));
        let _ = Command::new("attrib")
            .args(["-H", &path.to_string_lossy()])
            .status();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncate_middle_short_string_unchanged() {
        assert_eq!(truncate_middle("short.txt", 12), "short.txt");
    }

    #[test]
    fn truncate_middle_long_ascii_halves() {
        let result = truncate_middle(&"a".repeat(60), 30);
        // keep = (max - 1) / 2 = 14 each side plus the ellipsis -> 29 chars.
        assert_eq!(result.chars().count(), 29);
        assert!(result.starts_with("aaa"));
        assert!(result.ends_with("aaa"));
        assert!(result.contains('…'));
    }

    #[test]
    fn truncate_middle_counts_chars_not_bytes() {
        // 7 CJK chars, each 3 bytes. A byte-gated version would mis-truncate.
        let name = "界界界界界界界";
        let result = truncate_middle(name, 5);
        assert_eq!(result.chars().count(), 5);
    }

    #[test]
    fn truncate_middle_zero_max_does_not_panic() {
        let _ = truncate_middle("abc", 0);
    }

    #[test]
    fn truncate_middle_at_max_is_unchanged() {
        assert_eq!(truncate_middle("abc", 3), "abc");
    }

    #[test]
    fn truncate_middle_long_ascii_is_symmetric() {
        // keep = (max - 1) / 2 = 14 per side, plus one ellipsis -> 29 chars,
        // all 'a's, exactly one '…' in the middle.
        let result = truncate_middle(&"a".repeat(60), 30);
        assert_eq!(result.chars().count(), 29);
        assert_eq!(result, format!("{}{}{}", "a".repeat(14), "…", "a".repeat(14)));
        assert_eq!(result.matches('…').count(), 1);
    }

    #[test]
    fn truncate_middle_cjk_counts_chars_not_bytes() {
        // 7 CJK chars, each 3 bytes. A byte-gated version would truncate to
        // fewer than 5 chars; the char-gated version must keep exactly 5.
        let result = truncate_middle(&"界".repeat(7), 5);
        assert_eq!(result.chars().count(), 5);
        assert!(!result.ends_with("界界界界界"), "must actually truncate the long tail");
    }

    #[test]
    fn truncate_middle_tiny_max_is_bounded_and_nevers_panics() {
        for (s, max) in [("abc", 0), ("abc", 1), ("abcd", 1)] {
            let result = truncate_middle(s, max);
            assert!(
                result.chars().count() <= 1,
                "{s:?} with max {max} must collapse to at most 1 char, got {result:?}"
            );
        }
    }

    #[test]
    fn truncate_middle_empty_input_stays_empty() {
        assert_eq!(truncate_middle("", 10), "");
    }

    #[test]
    fn default_sort_is_dirs_then_newest_mtime() {
        use std::time::{Duration, UNIX_EPOCH};
        let older = UNIX_EPOCH + Duration::from_secs(10);
        let newer = UNIX_EPOCH + Duration::from_secs(50);
        let mut a = file("a.txt");
        a.modified = Some(older);
        let mut b = file("b.txt");
        b.modified = Some(newer);
        let mut dir = file("z-folder");
        dir.kind = EntryKind::Directory;
        dir.modified = Some(older);
        let snap = Snapshot::from_entries(vec![a, b, dir]);
        let names: Vec<_> = snap.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["z-folder", "b.txt", "a.txt"]);
    }

    #[test]
    fn same_contents_ignores_order_and_name_case() {
        let mut a = file("Readme.TXT");
        a.size = 12;
        let mut b = file("notes.md");
        b.size = 3;
        let left = Snapshot {
            entries: vec![a.clone(), b.clone()],
        };
        let mut b2 = b;
        b2.name = "NOTES.md".into();
        let right = Snapshot {
            entries: vec![b2, a.clone()],
        };
        assert!(left.same_contents(&right));
        let mut other = file("notes.md");
        other.size = 99;
        assert!(!left.same_contents(&Snapshot {
            entries: vec![a, other],
        }));
    }

    #[test]
    fn name_sort_is_case_insensitive() {
        let snap = Snapshot::sorted(
            vec![file("b.txt"), file("A.txt"), file("c.txt")],
            SortKey::Name,
        );
        let names: Vec<_> = snap.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["A.txt", "b.txt", "c.txt"]);
    }
}
