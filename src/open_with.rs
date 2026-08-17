//! OS-known Open with handlers, plus the native picker.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppHandler {
    pub id: String,
    pub name: String,
    pub exec: String,
}

/// A short list of apps the OS already associates with this file.
pub fn handlers_for(path: &Path) -> Vec<AppHandler> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux::handlers_for(path)
    }
    #[cfg(windows)]
    {
        windows::handlers_for(path)
    }
    #[cfg(target_os = "macos")]
    {
        macos::handlers_for(path)
    }
}

pub fn open_with(path: &Path, app: &AppHandler) -> Result<()> {
    if crate::mtp::is_mtp(path) {
        bail!("Open with is not available on a portable device.");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux::open_with(path, app)
    }
    #[cfg(windows)]
    {
        windows::open_with(path, app)
    }
    #[cfg(target_os = "macos")]
    {
        macos::open_with(path, app)
    }
}

/// Native OS "Open with" picker. Last row of the submenu.
pub fn choose_another(path: &Path) -> Result<()> {
    if crate::mtp::is_mtp(path) {
        bail!("Open with is not available on a portable device.");
    }
    #[cfg(windows)]
    {
        let status = Command::new("rundll32")
            .arg("shell32.dll,OpenAs_RunDLL")
            .arg(path)
            .spawn()?;
        let _ = status;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let pick = Command::new("osascript")
            .args(["-e", "POSIX path of (choose application)"])
            .output()?;
        if !pick.status.success() {
            bail!("No application chosen.");
        }
        let app = String::from_utf8_lossy(&pick.stdout).trim().to_string();
        Command::new("open")
            .args(["-a", &app, &path.display().to_string()])
            .spawn()?;
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux::choose_another(path)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod linux {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    pub fn handlers_for(path: &Path) -> Vec<AppHandler> {
        let mime = mime_of(path);
        let mut apps = desktop_apps();
        let mut matched: Vec<AppHandler> = apps
            .iter()
            .filter(|a| a.mimes.iter().any(|m| m == &mime || mime_match(&mime, m)))
            .map(|a| a.handler())
            .collect();
        if matched.is_empty() {
            // Still offer a couple of well-known viewers rather than an empty menu.
            matched = apps
                .drain(..)
                .filter(|a| {
                    matches!(
                        a.handler().id.as_str(),
                        "org.gnome.TextEditor.desktop"
                            | "org.gnome.gedit.desktop"
                            | "code.desktop"
                            | "firefox.desktop"
                            | "org.gnome.Loupe.desktop"
                            | "eog.desktop"
                    )
                })
                .map(|a| a.handler())
                .take(4)
                .collect();
        }
        matched.truncate(6);
        matched
    }

    pub fn open_with(path: &Path, app: &AppHandler) -> Result<()> {
        if let Some(id) = app.id.strip_suffix(".desktop") {
            if Command::new("gtk-launch").arg(id).arg(path).spawn().is_ok() {
                return Ok(());
            }
        }
        launch_exec(&app.exec, path)
    }

    pub fn choose_another(path: &Path) -> Result<()> {
        // Portal OpenURI with ask=true is the native chooser on FreeDesktop.
        if portal_open(path).is_ok() {
            return Ok(());
        }
        // mimeopen -a is the classic CLI picker when perl-file-mimeinfo is present.
        if Command::new("mimeopen")
            .args(["-a", &path.display().to_string()])
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
        bail!("No native application picker is available on this desktop.");
    }

    fn portal_open(path: &Path) -> Result<()> {
        // `--file` passes an fd; many desktops then show the app chooser.
        let status = Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.freedesktop.portal.Desktop",
                "--object-path",
                "/org/freedesktop/portal/desktop",
                "--method",
                "org.freedesktop.portal.OpenURI.OpenFile",
                "",
                &path.display().to_string(),
                "{'ask': <true>}",
            ])
            .status();
        match status {
            Ok(s) if s.success() => Ok(()),
            _ => bail!("portal OpenURI unavailable"),
        }
    }

    struct DesktopApp {
        id: String,
        name: String,
        exec: String,
        mimes: Vec<String>,
        nodisplay: bool,
    }

    impl DesktopApp {
        fn handler(&self) -> AppHandler {
            AppHandler {
                id: self.id.clone(),
                name: self.name.clone(),
                exec: self.exec.clone(),
            }
        }
    }

    fn desktop_apps() -> Vec<DesktopApp> {
        let mut dirs = vec![
            PathBuf::from("/usr/share/applications"),
            PathBuf::from("/usr/local/share/applications"),
        ];
        if let Some(home) = dirs::data_local_dir() {
            dirs.push(home.join("applications"));
        }
        let mut by_id: BTreeMap<String, DesktopApp> = BTreeMap::new();
        for dir in dirs {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for ent in rd.flatten() {
                let path = ent.path();
                if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                    continue;
                }
                if let Some(app) = parse_desktop(&path) {
                    if !app.nodisplay && !app.exec.is_empty() && !app.name.is_empty() {
                        by_id.insert(app.id.clone(), app);
                    }
                }
            }
        }
        by_id.into_values().collect()
    }

    fn parse_desktop(path: &Path) -> Option<DesktopApp> {
        let text = fs::read_to_string(path).ok()?;
        let mut in_entry = false;
        let mut name = String::new();
        let mut exec = String::new();
        let mut mimes = Vec::new();
        let mut nodisplay = false;
        let mut hidden = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_entry {
                continue;
            }
            if let Some(v) = line.strip_prefix("Name=") {
                if name.is_empty() {
                    name = v.to_string();
                }
            } else if let Some(v) = line.strip_prefix("Exec=") {
                exec = v.to_string();
            } else if let Some(v) = line.strip_prefix("MimeType=") {
                mimes = v
                    .split(';')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            } else if let Some(v) = line.strip_prefix("NoDisplay=") {
                nodisplay = v.eq_ignore_ascii_case("true");
            } else if let Some(v) = line.strip_prefix("Hidden=") {
                hidden = v.eq_ignore_ascii_case("true");
            } else if let Some(v) = line.strip_prefix("Type=") {
                if v != "Application" {
                    return None;
                }
            }
        }
        if hidden {
            return None;
        }
        Some(DesktopApp {
            id: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            name,
            exec,
            mimes,
            nodisplay,
        })
    }

    fn mime_of(path: &Path) -> String {
        if let Ok(out) = Command::new("xdg-mime")
            .args(["query", "filetype", &path.display().to_string()])
            .output()
            && out.status.success()
        {
            let mime = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !mime.is_empty() {
                return mime;
            }
        }
        crate::listing::mime_guess(path)
    }

    fn mime_match(file: &str, claimed: &str) -> bool {
        if claimed.ends_with("/*") {
            let prefix = claimed.trim_end_matches('*');
            return file.starts_with(prefix);
        }
        file == claimed
    }

    fn launch_exec(exec: &str, path: &Path) -> Result<()> {
        let mut args = shellish(exec);
        if args.is_empty() {
            bail!("Empty Exec line.");
        }
        let bin = args.remove(0);
        args.retain(|a| !a.starts_with('%'));
        args.push(path.display().to_string());
        Command::new(bin).args(args).spawn()?;
        Ok(())
    }

    fn shellish(exec: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut quote = None::<char>;
        for ch in exec.chars() {
            match (quote, ch) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), c) => cur.push(c),
                (None, '"' | '\'') => quote = Some(ch),
                (None, c) if c.is_whitespace() => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                (None, c) => cur.push(c),
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    pub fn handlers_for(path: &Path) -> Vec<AppHandler> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut apps = Vec::new();
        push(&mut apps, "notepad", "Notepad", "notepad.exe");
        if matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
        ) {
            push(&mut apps, "photos", "Photos", "mspaint.exe");
        }
        if matches!(ext.as_str(), "mp4" | "mkv" | "avi" | "webm") {
            push(
                &mut apps,
                "wmplayer",
                "Windows Media Player",
                "wmplayer.exe",
            );
        }
        apps
    }

    fn push(apps: &mut Vec<AppHandler>, id: &str, name: &str, exec: &str) {
        apps.push(AppHandler {
            id: id.into(),
            name: name.into(),
            exec: exec.into(),
        });
    }

    pub fn open_with(path: &Path, app: &AppHandler) -> Result<()> {
        Command::new(&app.exec).arg(path).spawn()?;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    pub fn handlers_for(path: &Path) -> Vec<AppHandler> {
        let out = Command::new("mdls")
            .args(["-name", "kMDItemContentType", &path.display().to_string()])
            .output()
            .ok();
        let _ = out;
        vec![
            AppHandler {
                id: "preview".into(),
                name: "Preview".into(),
                exec: "Preview".into(),
            },
            AppHandler {
                id: "textedit".into(),
                name: "TextEdit".into(),
                exec: "TextEdit".into(),
            },
        ]
    }

    pub fn open_with(path: &Path, app: &AppHandler) -> Result<()> {
        Command::new("open")
            .args(["-a", &app.exec, &path.display().to_string()])
            .spawn()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handlers_for_text_is_bounded() {
        let dir = std::env::temp_dir().join(format!("ply-ow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.txt");
        std::fs::write(&file, b"hi").unwrap();
        let apps = handlers_for(&file);
        assert!(apps.len() <= 6);
        std::fs::remove_dir_all(&dir).ok();
    }
}
