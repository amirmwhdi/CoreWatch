//! Keyboard shortcuts: one table, used both to set the accelerators and to
//! fill the Keyboard Shortcuts window, so the two never disagree.

use crate::i18n::gettext;

/// One row of the Keyboard Shortcuts window.
pub struct Shortcut {
    /// Translated title shown in the window.
    pub title: String,
    /// Accelerators as GTK parses them, e.g. `"<primary>q"`. The first one is
    /// shown; a range like `"<primary>1...<primary>9"` is only shown.
    pub accels: &'static [&'static str],
    /// Detailed action the accelerators activate, or `None` when the page
    /// handles the key itself (Delete on the process list).
    pub action: Option<&'static str>,
}

/// A titled group of shortcuts.
pub struct Group {
    pub title: String,
    pub shortcuts: Vec<Shortcut>,
}

/// Pages Ctrl+1 … Ctrl+9 can reach.
pub const NUMBERED_PAGES: i32 = 9;

pub fn groups() -> Vec<Group> {
    vec![
        Group {
            title: gettext("General"),
            shortcuts: vec![
                Shortcut {
                    title: gettext("Preferences"),
                    accels: &["<primary>comma"],
                    action: Some("app.preferences"),
                },
                Shortcut {
                    title: gettext("Keyboard Shortcuts"),
                    accels: &["<primary>question"],
                    action: Some("app.shortcuts"),
                },
                Shortcut {
                    title: gettext("Close Window"),
                    accels: &["<primary>w"],
                    action: Some("window.close"),
                },
                Shortcut {
                    title: gettext("Quit"),
                    accels: &["<primary>q"],
                    action: Some("app.quit"),
                },
            ],
        },
        Group {
            title: gettext("Navigation"),
            shortcuts: vec![Shortcut {
                // Translators: Ctrl+1 opens the first page of the sidebar, Ctrl+2 the second, …
                title: gettext("Go to Page 1 to 9"),
                accels: &["<primary>1...<primary>9"],
                // Set one by one in `set_accels`.
                action: None,
            }],
        },
        Group {
            title: gettext("Processes"),
            shortcuts: vec![
                Shortcut {
                    title: gettext("Search Processes"),
                    accels: &["<primary>f"],
                    action: Some("app.find"),
                },
                Shortcut {
                    title: gettext("End Selected Process"),
                    accels: &["Delete"],
                    action: None,
                },
            ],
        },
    ]
}

/// Set every accelerator of the table on `app`.
pub fn set_accels(app: &adw::Application) {
    use gtk::prelude::GtkApplicationExt;
    for group in groups() {
        for shortcut in group.shortcuts {
            if let Some(action) = shortcut.action {
                app.set_accels_for_action(action, shortcut.accels);
            }
        }
    }
    for n in 1..=NUMBERED_PAGES {
        app.set_accels_for_action(
            &format!("app.show-page-number({n})"),
            &[&format!("<primary>{n}")],
        );
    }
}
