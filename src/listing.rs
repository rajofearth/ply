use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Shared so the icon batch (`app/ops.rs` `reload`) borrows the same
    /// allocation the listing paints from: handing the background task a
    /// clone bumps a refcount instead of cloning up to 512 full `Entry`s.
    /// Reads (`iter`, `len`, `get`, `&entries as &[Entry]`) work unchanged
    /// through deref; sorting goes through `resort`, which sorts in place
    /// when uniquely owned and only clones on genuine sharing.
    pub entries: Arc<Vec<Entry>>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            entries: Arc::new(Vec::new()),
        }
    }
}

impl Snapshot {
    #[cfg(test)]
    pub fn from_entries(entries: Vec<Entry>) -> Self {
        Self::sorted(entries, SortKey::default())
    }

    pub fn sorted(mut entries: Vec<Entry>, key: SortKey) -> Self {
        sort_entries(&mut entries, key);
        Self {
            entries: Arc::new(entries),
        }
    }

    pub fn resort(&mut self, key: SortKey) {
        sort_entries(Arc::make_mut(&mut self.entries), key);
    }

    /// Order-independent equality so a resorted listing is not a "change".
    /// Same-order listings (the usual watch reload) compare in place with no extra alloc.
    pub fn same_contents(&self, other: &Self) -> bool {
        let a = &self.entries;
        let b = &other.entries;
        if a.len() != b.len() {
            return false;
        }
        if a.iter().zip(b.iter()).all(|(x, y)| entry_meta_eq(x, y)) {
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

/// Lowercase once per reload/resort/keystroke. ASCII folds byte-wise with no
/// Unicode tables; anything else uses full casefold. For pure-ASCII input
/// this equals `to_lowercase`, so mixed comparisons stay identical to the old
/// lower-on-every-compare path (locked by `lowered_name_matches_lowercase`).
fn lowered_name(name: &str) -> String {
    if name.is_ascii() {
        name.to_ascii_lowercase()
    } else {
        name.to_lowercase()
    }
}

/// Sort with keys precomputed once, not once per comparison. Lowering (which
/// allocates) inside the comparator costs O(n log n) allocations; this lowers
/// each name once per reload/resort and reorders by moves only. A new
/// generation always builds a new `Snapshot`, so precomputed keys can never
/// go stale across reloads; `resort` recomputes them for the new key. Order
/// is identical to `compare_entries` (locked by
/// `precomputed_sort_matches_compare_entries`).
fn sort_entries(entries: &mut Vec<Entry>, key: SortKey) {
    struct Prep {
        dir: bool,
        lower: String,
        kind: &'static str,
    }
    let mut prep: Vec<Prep> = Vec::with_capacity(entries.len());
    // `Modified` orders by timestamp alone, so it skips the text keys.
    let need_text = !matches!(key, SortKey::Modified);
    for entry in entries.iter() {
        prep.push(Prep {
            dir: entry.is_directory(),
            lower: if need_text {
                lowered_name(&entry.name)
            } else {
                String::new()
            },
            kind: if need_text { kind_label(entry) } else { "" },
        });
    }
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|&a, &b| {
        let (pa, pb) = (&prep[a], &prep[b]);
        pb.dir.cmp(&pa.dir).then_with(|| match key {
            SortKey::Name => pa.lower.cmp(&pb.lower),
            SortKey::Modified => entries[b].modified.cmp(&entries[a].modified),
            SortKey::Kind => pa.kind.cmp(pb.kind).then_with(|| pa.lower.cmp(&pb.lower)),
            SortKey::Size => entries[b]
                .size
                .cmp(&entries[a].size)
                .then_with(|| pa.lower.cmp(&pb.lower)),
        })
    });
    // Permute by moves, keeping the existing allocation: each entry travels
    // exactly once, nothing is cloned.
    let mut slots: Vec<Option<Entry>> = entries.drain(..).map(Some).collect();
    for i in order {
        entries.push(slots[i].take().expect("sort order is a permutation"));
    }
}

/// Indices into `entries` surviving `needle`, in listing order, written into
/// `out` after clearing it so keystrokes reuse the allocation. The needle is
/// lowered once per call; ASCII names then match with zero per-entry
/// allocation (the old code called `to_lowercase` on every entry per
/// keystroke). Results equal
/// `name.to_lowercase().contains(needle.to_lowercase())` on all inputs
/// (locked by `filter_matches_reference`, including Kelvin/ß/İ edges).
pub fn filter_indices_into(entries: &[Entry], needle: &str, out: &mut Vec<usize>) {
    out.clear();
    if needle.is_empty() {
        out.extend(0..entries.len());
        return;
    }
    let lowered = lowered_name(needle);
    out.reserve(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        if name_contains_lowered(&entry.name, &lowered) {
            out.push(i);
        }
    }
}

/// Substring test against an already-lowered needle. ASCII/ASCII pairs fold
/// bytes inline with no allocation; anything else lowers the haystack once,
/// the only remaining per-entry alloc, and only for non-ASCII names. Safe
/// when the lowered needle is ASCII but the raw needle was not (e.g. Kelvin
/// K folds to `k`): with both lowered sides ASCII the reference comparison
/// reduces to ASCII-lowercase byte matching, which is exactly this path.
fn name_contains_lowered(name: &str, lowered_needle: &str) -> bool {
    if lowered_needle.is_empty() {
        return true;
    }
    if name.is_ascii() && lowered_needle.is_ascii() {
        ascii_contains(name.as_bytes(), lowered_needle.as_bytes())
    } else {
        lowered_name(name).contains(lowered_needle)
    }
}

/// ASCII case-insensitive substring search with no allocation and no
/// per-byte closures (plain loops stay fast in debug builds too, where the
/// `windows().any()` + `all()` closure nest measured 3x slower than the
/// old allocating filter on 10k entries).
fn ascii_contains(hay: &[u8], ndl: &[u8]) -> bool {
    if ndl.is_empty() {
        return true;
    }
    if ndl.len() > hay.len() {
        return false;
    }
    let first = ndl[0];
    let mut i = 0;
    while i + ndl.len() <= hay.len() {
        if hay[i].to_ascii_lowercase() == first {
            let mut j = 1;
            while j < ndl.len() && hay[i + j].to_ascii_lowercase() == ndl[j] {
                j += 1;
            }
            if j == ndl.len() {
                return true;
            }
        }
        i += 1;
    }
    false
}

// Reference order for `precomputed_sort_matches_compare_entries`. Kept out of
// release builds: the shipping path is `sort_entries` above.
#[cfg(test)]
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
    let read = std::fs::read_dir(path)?;
    // Grow once, not per entry: the `ReadDir` hint is 0 on most platforms, so
    // floor it at a typical folder size.
    let mut entries = Vec::with_capacity(read.size_hint().0.max(256));
    for dirent in read {
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

/// Full Explorer-style stamp for the Properties modal, e.g.
/// `Monday, December 1, 2025, 2:08:28 PM`. Unlike [`format_mtime`] this never
/// shortens by recency. Pure; `now` is taken for signature symmetry only.
pub fn format_full_datetime(when: Option<SystemTime>, now: DateTime<Local>) -> String {
    let _ = now;
    let Some(t) = when else {
        return "—".into();
    };
    let when: DateTime<Local> = t.into();
    when.format("%A, %B %-d, %Y, %-I:%M:%S %p").to_string()
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
        assert_eq!(
            result,
            format!("{}{}{}", "a".repeat(14), "…", "a".repeat(14))
        );
        assert_eq!(result.matches('…').count(), 1);
    }

    #[test]
    fn truncate_middle_cjk_counts_chars_not_bytes() {
        // 7 CJK chars, each 3 bytes. A byte-gated version would truncate to
        // fewer than 5 chars; the char-gated version must keep exactly 5.
        let result = truncate_middle(&"界".repeat(7), 5);
        assert_eq!(result.chars().count(), 5);
        assert!(
            !result.ends_with("界界界界界"),
            "must actually truncate the long tail"
        );
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
            entries: Arc::new(vec![a.clone(), b.clone()]),
        };
        let mut b2 = b;
        b2.name = "NOTES.md".into();
        let right = Snapshot {
            entries: Arc::new(vec![b2, a.clone()]),
        };
        assert!(left.same_contents(&right));
        let mut other = file("notes.md");
        other.size = 99;
        assert!(!left.same_contents(&Snapshot {
            entries: Arc::new(vec![a, other]),
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

    #[test]
    fn lowered_name_matches_lowercase() {
        // Includes the edges where ASCII-only folding would diverge: Kelvin
        // sign (folds to ASCII `k`), sharp s (no-op lower), Turkish dotted
        // capital I (expands to `i` + combining dot), CJK, emoji, empty.
        for name in [
            "",
            "Readme.TXT",
            "Makefile",
            "café.TXT",
            "界界.txt",
            "K",
            "k",
            "STRASSE.txt",
            "straße.txt",
            "İSTANBUL.txt",
            "istanbul.txt",
            "📁 Photos.JPG",
            "aBcDeF 123 !@#",
        ] {
            assert_eq!(lowered_name(name), name.to_lowercase(), "{name:?}");
        }
    }

    fn mixed_entries() -> Vec<Entry> {
        use std::time::{Duration, UNIX_EPOCH};
        let mut entries = Vec::new();
        let mut add = |name: &str, size: u64, secs: u64, dir: bool| {
            let mut e = file(name);
            e.size = size;
            e.modified = Some(UNIX_EPOCH + Duration::from_secs(secs));
            if dir {
                e.kind = EntryKind::Directory;
            }
            entries.push(e);
        };
        add("b.txt", 30, 30, false);
        add("A.txt", 10, 10, false);
        add("c.TXT", 20, 20, false);
        add("z-folder", 0, 5, true);
        add("a-folder", 0, 50, true);
        add("café.md", 15, 15, false);
        add("CAFÉ.md", 25, 25, false);
        add("photo.JPG", 100, 40, false);
        add("notes.zip", 60, 35, false);
        add("K", 5, 45, false);
        add("k", 6, 46, false);
        add("界.txt", 70, 12, false);
        entries
    }

    fn reference_sorted(mut entries: Vec<Entry>, key: SortKey) -> Vec<String> {
        entries.sort_by(|a, b| compare_entries(a, b, key));
        entries.into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn precomputed_sort_matches_compare_entries() {
        // Fresh sorts start from insertion order on both sides.
        for key in [
            SortKey::Name,
            SortKey::Modified,
            SortKey::Kind,
            SortKey::Size,
        ] {
            let expected = reference_sorted(mixed_entries(), key);
            let got: Vec<String> = Snapshot::sorted(mixed_entries(), key)
                .entries
                .iter()
                .map(|e| e.name.clone())
                .collect();
            assert_eq!(got, expected, "fresh sort with {key:?}");
        }
        // Resorts start from Modified order on both sides: ties keep input
        // order under any stable sort, so the reference must see the same
        // input as `resort` (this bit the first version of this test, which
        // compared a resort against an insertion-ordered fresh sort).
        let base = Snapshot::sorted(mixed_entries(), SortKey::Modified);
        for key in [
            SortKey::Name,
            SortKey::Modified,
            SortKey::Kind,
            SortKey::Size,
        ] {
            let mut reference: Vec<Entry> = base.entries.iter().cloned().collect();
            reference.sort_by(|a, b| compare_entries(a, b, key));
            let expected: Vec<String> = reference.into_iter().map(|e| e.name).collect();
            let mut snap = base.clone();
            snap.resort(key);
            let got: Vec<String> = snap.entries.iter().map(|e| e.name.clone()).collect();
            assert_eq!(got, expected, "resort with {key:?}");
        }
    }

    fn reference_matches(name: &str, needle: &str) -> bool {
        name.to_lowercase().contains(&needle.to_lowercase())
    }

    #[test]
    fn filter_matches_reference() {
        let names = [
            "alpha.txt",
            "ALPHA2.TXT",
            "beta.mp4",
            "almanac.png",
            "delta.log",
            "café.md",
            "CAFÉ AU LAIT.txt",
            "K",
            "k.txt",
            "Kelvin.txt",
            "STRASSE.txt",
            "straße.txt",
            "界alpha.txt",
            "Makefile",
            "a",
            "",
        ];
        let entries: Vec<Entry> = names.iter().map(|n| file(n)).collect();
        let needles = [
            "",
            "a",
            "AL",
            "alpha",
            "pha",
            "mp4",
            "xyz",
            "m",
            "é",
            "E",
            "k",
            "K",
            "ß",
            "SS",
            "界",
            "alpha.txt",
            " ",
            ".",
        ];
        let mut scratch = Vec::new();
        for needle in needles {
            filter_indices_into(&entries, needle, &mut scratch);
            let expected: Vec<usize> = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| reference_matches(&e.name, needle))
                .map(|(i, _)| i)
                .collect();
            assert_eq!(scratch, expected, "needle {needle:?}");
            // Reuse: a second pass over the same scratch must agree.
            let mut again = std::mem::take(&mut scratch);
            filter_indices_into(&entries, needle, &mut again);
            assert_eq!(again, expected, "reused scratch, needle {needle:?}");
            scratch = again;
        }
    }

    #[test]
    fn same_contents_still_holds_under_shared_ownership() {
        // Cloning a Snapshot shares the allocation; equality still compares
        // contents, and resorting the clone must not disturb the original.
        // Names are case-unique here: `same_contents` pairs case-tied names
        // positionally, so entries differing only by case need identical
        // metadata to pair up (pre-existing semantics, unchanged).
        let unique: Vec<Entry> = mixed_entries()
            .into_iter()
            .filter(|e| e.name != "k" && e.name != "CAFÉ.md")
            .collect();
        let snap = Snapshot::sorted(unique, SortKey::Name);
        let before: Vec<String> = snap.entries.iter().map(|e| e.name.clone()).collect();
        let shared = snap.clone();
        assert!(snap.same_contents(&shared));
        let mut moved = shared;
        moved.resort(SortKey::Size);
        assert!(snap.same_contents(&moved));
        assert_eq!(snap.entries.len(), moved.entries.len());
        // Copy-on-write: resorting the clone leaves the original's order alone.
        let after: Vec<String> = snap.entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn format_full_datetime_matches_explorer_style() {
        use chrono::TimeZone;
        let evening = Local.with_ymd_and_hms(2025, 12, 1, 14, 8, 28).unwrap();
        let evening_sys: SystemTime = chrono::DateTime::<chrono::Utc>::from(evening).into();
        assert_eq!(
            format_full_datetime(Some(evening_sys), evening),
            "Monday, December 1, 2025, 2:08:28 PM"
        );
        let morning = Local.with_ymd_and_hms(2025, 1, 5, 9, 5, 3).unwrap();
        let morning_sys: SystemTime = chrono::DateTime::<chrono::Utc>::from(morning).into();
        assert_eq!(
            format_full_datetime(Some(morning_sys), morning),
            "Sunday, January 5, 2025, 9:05:03 AM"
        );
    }

    #[test]
    fn format_full_datetime_none_is_em_dash() {
        assert_eq!(format_full_datetime(None, Local::now()), "—");
    }
}
