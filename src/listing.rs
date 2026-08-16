use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// An Entry listed under the Workspace.
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

    pub fn same_as(&self, other: &Self) -> bool {
        self.fingerprint == other.fingerprint
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

fn collect_entries(path: &Path, show_hidden: bool) -> anyhow::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for dirent in std::fs::read_dir(path)? {
        let dirent = dirent?;
        let Some(entry) = entry_from_dirent(dirent) else {
            continue;
        };
        if entry.hidden && !show_hidden {
            continue;
        }
        entries.push(entry);
    }
    Ok(entries)
}

pub fn list_dir(path: &Path, show_hidden: bool) -> anyhow::Result<Snapshot> {
    Ok(Snapshot::from_entries(collect_entries(path, show_hidden)?))
}

/// Direct children that are Directory entries only. Does not recurse or follow symlinks.
pub fn list_dirs(path: &Path, show_hidden: bool) -> anyhow::Result<Snapshot> {
    let dirs = collect_entries(path, show_hidden)?
        .into_iter()
        .filter(|entry| entry.is_directory())
        .collect();
    Ok(Snapshot::from_entries(dirs))
}

pub fn sort_snapshot(mut snapshot: Snapshot, column: &str, ascending: bool) -> Snapshot {
    snapshot.entries.sort_by(|a, b| {
        let ord = match column {
            "name" => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            "size" => a.size.cmp(&b.size),
            "modified" => a.modified.cmp(&b.modified),
            "kind" => kind_discriminant(&a.kind).cmp(&kind_discriminant(&b.kind)),
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        if ascending { ord } else { ord.reverse() }
    });
    snapshot.fingerprint = snapshot.entries.iter().map(Entry::fingerprint).collect();
    snapshot
}

pub fn parent_in_workspace(current: &Path, workspace: &Path) -> Option<PathBuf> {
    let parent = current.parent()?;
    if parent.starts_with(workspace) || parent == workspace {
        Some(parent.to_path_buf())
    } else {
        None
    }
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

/// Short label for the Kind column (Folder, JPEG Image, Text Document, …).
pub fn entry_kind_label(entry: &Entry) -> String {
    match &entry.kind {
        EntryKind::Directory => "Folder".into(),
        EntryKind::Symlink { .. } => "Link".into(),
        EntryKind::File => {
            let ext = entry
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            match ext.as_str() {
                "jpg" | "jpeg" => "JPEG Image".into(),
                "png" => "PNG Image".into(),
                "gif" => "GIF Image".into(),
                "webp" | "bmp" | "tif" | "tiff" | "heic" | "avif" => "Image".into(),
                "mp4" | "mpeg" | "mpg" | "m4v" | "mov" | "mkv" | "webm" | "avi" => {
                    "MPEG Video".into()
                }
                "txt" | "md" | "markdown" | "log" | "csv" | "tsv" | "json" | "toml" | "yaml"
                | "yml" | "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "html" | "css" | "xml" => {
                    "Text Document".into()
                }
                "doc" | "docx" | "odt" | "rtf" => "Word Document".into(),
                "exe" | "msi" | "com" | "bat" | "cmd" | "ps1" | "app" | "appimage" | "bin"
                | "run" | "sh" | "deb" | "rpm" => "Application".into(),
                "m3u" | "m3u8" | "pls" | "xspf" | "wpl" => "Playlist".into(),
                _ => "File".into(),
            }
        }
    }
}

/// ISO-like UTC timestamp (`YYYY-MM-DD HH:MM`), not raw unix seconds.
pub fn format_mtime(t: Option<SystemTime>) -> String {
    let Some(t) = t else {
        return "—".into();
    };
    let Ok(dur) = t.duration_since(std::time::UNIX_EPOCH) else {
        return "—".into();
    };
    let secs = dur.as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400) as u32;
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    let (year, month, day) = civil_from_unix_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}")
}

/// Howard Hinnant's `civil_from_days` with unix epoch day 0 = 1970-01-01.
fn civil_from_unix_days(unix_days: i64) -> (i32, u32, u32) {
    let z = unix_days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_entry(name: &str) -> Entry {
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
    fn entry_kind_label_by_extension() {
        assert_eq!(
            entry_kind_label(&Entry {
                path: PathBuf::from("docs"),
                name: "docs".into(),
                kind: EntryKind::Directory,
                size: 0,
                modified: None,
                hidden: false,
            }),
            "Folder"
        );
        assert_eq!(
            entry_kind_label(&Entry {
                path: PathBuf::from("link"),
                name: "link".into(),
                kind: EntryKind::Symlink {
                    target: PathBuf::from("x")
                },
                size: 0,
                modified: None,
                hidden: false,
            }),
            "Link"
        );
        assert_eq!(entry_kind_label(&file_entry("photo.JPEG")), "JPEG Image");
        assert_eq!(entry_kind_label(&file_entry("a.png")), "PNG Image");
        assert_eq!(entry_kind_label(&file_entry("clip.mp4")), "MPEG Video");
        assert_eq!(entry_kind_label(&file_entry("notes.txt")), "Text Document");
        assert_eq!(
            entry_kind_label(&file_entry("resume.docx")),
            "Word Document"
        );
        assert_eq!(entry_kind_label(&file_entry("app.exe")), "Application");
        assert_eq!(entry_kind_label(&file_entry("mix.m3u")), "Playlist");
        assert_eq!(entry_kind_label(&file_entry("data.bin.bak")), "File");
    }
}
