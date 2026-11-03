//! The only place for code that depends on the GTK or libadwaita version.
//!
//! Corewatch builds against a floor (GTK 4.18, libadwaita 1.7, as in Debian 13)
//! and adds newer APIs behind cumulative Cargo features (`adw-1-8`, ...).
//! Feature code calls neutral functions defined here; only this module uses
//! `#[cfg(feature = ...)]` for library versions.

use crate::shortcuts::Group;
use gtk::prelude::*;

/// Show the Keyboard Shortcuts window for `parent`.
///
/// GTK 4.18 deprecates `GtkShortcutsWindow` in favour of `AdwShortcutsDialog`,
/// which only arrives with libadwaita 1.8. At the floor the GTK window is the
/// one that exists; an `adw-1-8` feature can switch to the dialog later.
#[allow(deprecated)]
pub fn show_shortcuts(parent: &gtk::Window, groups: Vec<Group>) {
    let section = gtk::ShortcutsSection::builder()
        .section_name("main")
        .max_height(12)
        .build();
    for group in groups {
        let box_ = gtk::ShortcutsGroup::builder().title(group.title).build();
        for shortcut in group.shortcuts {
            box_.add_shortcut(
                &gtk::ShortcutsShortcut::builder()
                    .title(shortcut.title)
                    .accelerator(shortcut.accels[0])
                    .build(),
            );
        }
        section.add_group(&box_);
    }
    let window = gtk::ShortcutsWindow::builder()
        .transient_for(parent)
        .modal(true)
        .build();
    window.add_section(&section);
    window.present();
}
