//! Icons for processes, from the `.desktop` files of installed apps.
//!
//! A process is matched to an app by the program its `Exec` line starts
//! (`firefox`), its `StartupWMClass`, or the app id (`org.gnome.Nautilus`
//! matches `nautilus`). Inside Flatpak the host's apps are read through
//! `/run/host` and the Flatpak export directories, and their icon folders
//! are added to the icon theme.

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Shown for processes that belong to no app.
pub const FALLBACK: &str = "application-x-executable-symbolic";

/// Process name (lowercase) to icon.
pub struct AppIcons {
    icons: HashMap<String, gio::Icon>,
    fallback: gio::Icon,
}

impl AppIcons {
    pub fn load() -> Self {
        let flatpak = Path::new("/.flatpak-info").exists();
        let dirs = data_dirs(flatpak);
        if flatpak {
            add_icon_paths(&dirs);
        }
        let mut icons = HashMap::new();
        for dir in &dirs {
            let Ok(entries) = std::fs::read_dir(dir.join("applications")) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "desktop") {
                    read_desktop(&path, flatpak, &mut icons);
                }
            }
        }
        Self {
            icons,
            fallback: gio::ThemedIcon::new(FALLBACK).upcast(),
        }
    }

    pub fn lookup(&self, process: &str) -> gio::Icon {
        let key = process.to_lowercase();
        self.icons
            .get(&key)
            .or_else(|| key.strip_suffix("-bin").and_then(|k| self.icons.get(k)))
            .unwrap_or(&self.fallback)
            .clone()
    }
}

/// Folders with an `applications` subfolder, most specific first, so the
/// user's own apps win.
fn data_dirs(flatpak: bool) -> Vec<PathBuf> {
    let home = glib::home_dir();
    let mut dirs = Vec::new();
    if flatpak {
        // The sandbox's XDG variables point into the sandbox; use the host's.
        dirs.push(home.join(".local/share"));
        dirs.push(home.join(".local/share/flatpak/exports/share"));
        dirs.push(PathBuf::from("/var/lib/flatpak/exports/share"));
        dirs.push(PathBuf::from("/run/host/usr/local/share"));
        dirs.push(PathBuf::from("/run/host/usr/share"));
    } else {
        dirs.push(glib::user_data_dir());
        dirs.extend(glib::system_data_dirs());
    }
    dirs.dedup();
    dirs
}

/// Let the icon theme find the host's icons.
fn add_icon_paths(dirs: &[PathBuf]) {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let theme = gtk::IconTheme::for_display(&display);
    for dir in dirs {
        for sub in ["icons", "pixmaps"] {
            let path = dir.join(sub);
            if path.is_dir() {
                theme.add_search_path(&path);
            }
        }
    }
}

fn read_desktop(path: &Path, flatpak: bool, icons: &mut HashMap<String, gio::Icon>) {
    let file = glib::KeyFile::new();
    if file.load_from_file(path, glib::KeyFileFlags::NONE).is_err() {
        return;
    }
    const GROUP: &str = "Desktop Entry";
    let get = |key: &str| file.string(GROUP, key).ok().map(String::from);
    if get("Type").as_deref() != Some("Application") {
        return;
    }
    let Some(icon) = get("Icon").filter(|i| !i.is_empty()) else {
        return;
    };
    let icon: gio::Icon = if icon.starts_with('/') {
        // A host path; inside Flatpak the host's /usr is under /run/host.
        let path = match icon.strip_prefix("/usr/") {
            Some(rest) if flatpak => format!("/run/host/usr/{rest}"),
            _ => icon,
        };
        gio::FileIcon::new(&gio::File::for_path(path)).upcast()
    } else {
        gio::ThemedIcon::new(&icon).upcast()
    };

    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let mut keys = Vec::new();
    if let Some(exec) = get("Exec") {
        keys.extend(exec_program(&exec));
    }
    keys.extend(get("StartupWMClass"));
    keys.push(id.to_owned());
    if let Some(last) = id.rsplit('.').next() {
        keys.push(last.to_owned());
    }
    for key in keys {
        let key = key.to_lowercase();
        if key.is_empty() {
            continue;
        }
        // The kernel keeps 15 bytes of a name; Flatpak sees only those.
        if let Some(short) = key.get(..15).filter(|_| key.len() > 15) {
            icons
                .entry(short.to_owned())
                .or_insert_with(|| icon.clone());
        }
        icons.entry(key).or_insert_with(|| icon.clone());
    }
}

/// The program an `Exec` line starts: `env A=B foo %U` gives `foo`,
/// `flatpak run --command=foo org.App` gives `foo`, and `flatpak run
/// org.example.App` gives `app` (matched through the app id).
pub fn exec_program(exec: &str) -> Option<String> {
    let mut words = exec.split_whitespace().map(|w| w.trim_matches('"'));
    let mut program = words.next()?;
    if base(program) == "env" {
        program = words.find(|w| !w.contains('=') && !w.starts_with('-'))?;
    }
    if base(program) == "flatpak" {
        let rest: Vec<&str> = words.collect();
        if let Some(cmd) = rest.iter().find_map(|w| w.strip_prefix("--command=")) {
            return Some(base(cmd).to_owned());
        }
        let app = rest.iter().find(|w| !w.starts_with('-') && **w != "run")?;
        return app.rsplit('.').next().map(str::to_owned);
    }
    Some(base(program).to_owned())
}

fn base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::exec_program;

    #[test]
    fn exec_lines_name_their_program() {
        assert_eq!(
            exec_program("/usr/bin/firefox %u").as_deref(),
            Some("firefox")
        );
        assert_eq!(
            exec_program("env GDK_BACKEND=x11 code --new-window").as_deref(),
            Some("code")
        );
        assert_eq!(
            exec_program("/usr/bin/flatpak run --branch=stable --command=telegram-desktop org.telegram.desktop -- %u")
                .as_deref(),
            Some("telegram-desktop")
        );
        assert_eq!(
            exec_program("flatpak run org.gnome.Calculator").as_deref(),
            Some("Calculator")
        );
    }
}
