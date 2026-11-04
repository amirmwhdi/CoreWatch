//! Which app or service a process belongs to, from its cgroup.
//!
//! systemd puts every app a desktop starts in its own unit, named after the
//! app's ID (the name of its `.desktop` file):
//!
//! ```text
//! 0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-org.gnome.Nautilus-4242.scope
//! 0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-org.mozilla.firefox-77.scope
//! 0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.kde.dolphin@a1b2.service
//! 0::/system.slice/NetworkManager.service
//! ```
//!
//! (`app-[<launcher>-]<id>[-<random>].scope` or `[@<random>].service`, as
//! the XDG standard for systemd units says.) Every process of one app, its
//! helpers and web content included, shares the app's unit. Services are
//! grouped by unit name; kernel threads have no unit.

/// The group of a process: an app ID or a unit name; `None` for kernel
/// threads and processes outside any unit.
pub fn group(cgroup_file: &str) -> Option<String> {
    // cgroup v2: one line "0::<path>". On v1 the "name=systemd" line has
    // the same path.
    let path = cgroup_file
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .or_else(|| {
            cgroup_file
                .lines()
                .find_map(|l| l.split_once(":name=systemd:").map(|(_, p)| p))
        })?;
    let unit = path
        .rsplit('/')
        .find(|c| c.ends_with(".scope") || c.ends_with(".service"))?;
    let unit = unescape(unit);
    let (name, scope) = match unit.strip_suffix(".scope") {
        Some(name) => (name, true),
        None => (unit.strip_suffix(".service")?, false),
    };
    let id = if let Some(app) = name.strip_prefix("app-") {
        app_id(app, scope)
    } else if let Some(rest) = name.strip_prefix("dbus-:") {
        // A D-Bus activated service: "dbus-:1.2-org.gnome.Calendar@0".
        let (_, id) = rest.split_once('-')?;
        without_instance(id).to_owned()
    } else if name == "init" {
        return None; // the user's or the system's service manager itself
    } else {
        without_instance(name).to_owned()
    };
    (!id.is_empty()).then_some(id)
}

/// Launchers that put their own name before the app ID.
const LAUNCHERS: [&str; 8] = [
    "gnome", "flatpak", "kde", "xfce", "cosmic", "lxqt", "mate", "budgie",
];

/// `<launcher>-<id>-<random>` (a scope) or `<launcher>-<id>@<random>` (a
/// service) to `<id>`.
fn app_id(name: &str, scope: bool) -> String {
    let name = match name.split_once('-') {
        Some((launcher, rest)) if LAUNCHERS.contains(&launcher) => rest,
        _ => name,
    };
    let name = without_instance(name);
    let name = if scope {
        // The random part of a scope is a number after the last dash.
        match name.rsplit_once('-') {
            Some((id, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => id,
            _ => name,
        }
    } else {
        name
    };
    name.to_owned()
}

fn without_instance(name: &str) -> &str {
    name.split_once('@').map_or(name, |(base, _)| base)
}

/// systemd escapes a "-" inside a name as `\x2d`.
fn unescape(unit: &str) -> std::borrow::Cow<'_, str> {
    if unit.contains("\\x") {
        let mut out = String::with_capacity(unit.len());
        let mut rest = unit;
        while let Some(i) = rest.find("\\x") {
            out.push_str(&rest[..i]);
            let hex = rest.get(i + 2..i + 4);
            match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(byte) if byte.is_ascii() => {
                    out.push(char::from(byte));
                    rest = &rest[i + 4..];
                }
                _ => {
                    out.push_str("\\x");
                    rest = &rest[i + 2..];
                }
            }
        }
        out.push_str(rest);
        out.into()
    } else {
        unit.into()
    }
}

#[cfg(test)]
mod tests {
    use super::group;

    const USER: &str = "0::/user.slice/user-1000.slice/user@1000.service";

    #[test]
    fn apps_by_id() {
        let cases = [
            (
                "app.slice/app-gnome-org.gnome.Nautilus-4242.scope",
                "org.gnome.Nautilus",
            ),
            ("app.slice/app-gnome-firefox-123.scope", "firefox"),
            (
                "app.slice/app-flatpak-org.mozilla.firefox-77.scope",
                "org.mozilla.firefox",
            ),
            (
                "app.slice/app-org.kde.dolphin@a1b2c3.service",
                "org.kde.dolphin",
            ),
            (
                "app.slice/app-gnome-org.gnome.Evolution\\x2dalarm\\x2dnotify-2367.scope",
                "org.gnome.Evolution-alarm-notify",
            ),
            // A child cgroup inside an app's scope still belongs to the app.
            (
                "app.slice/app-flatpak-com.valvesoftware.Steam-9.scope/game",
                "com.valvesoftware.Steam",
            ),
        ];
        for (path, id) in cases {
            let text = format!("{USER}/{path}\n");
            assert_eq!(group(&text).as_deref(), Some(id), "{path}");
        }
    }

    #[test]
    fn services_by_unit() {
        assert_eq!(
            group("0::/system.slice/NetworkManager.service\n").as_deref(),
            Some("NetworkManager")
        );
        assert_eq!(
            group(&format!(
                "{USER}/session.slice/org.gnome.Shell@wayland.service\n"
            ))
            .as_deref(),
            Some("org.gnome.Shell")
        );
        assert_eq!(
            group(&format!("{USER}/app.slice/app-dbus\\x2d:1.2\\x2dorg.gnome.Calendar.slice/dbus-:1.2-org.gnome.Calendar@0.service\n")).as_deref(),
            Some("org.gnome.Calendar")
        );
        assert_eq!(
            group("0::/user.slice/user-1000.slice/session-3.scope\n").as_deref(),
            Some("session-3")
        );
    }

    #[test]
    fn kernel_threads_and_managers_have_no_group() {
        assert_eq!(group("0::/\n"), None);
        assert_eq!(group(&format!("{USER}/init.scope\n")), None);
        assert_eq!(group("0::/init.scope\n"), None);
        assert_eq!(group(""), None);
    }

    #[test]
    fn cgroup_v1_uses_the_systemd_hierarchy() {
        let text = "12:cpu,cpuacct:/\n1:name=systemd:/system.slice/cron.service\n";
        assert_eq!(group(text).as_deref(), Some("cron"));
    }
}
