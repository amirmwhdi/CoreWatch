//! Corewatch: a modern system monitor for Linux.

mod alerts;
mod app_icons;
mod battery_history;
mod compat;
mod config;
mod drive_smart;
mod format;
mod logging;
mod modules;
mod preferences;
mod settings;
mod shortcuts;
mod widgets;
mod window;

use adw::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    english_only();

    // Our own flags are handled here, before GApplication sees the arguments.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |short: &str, long: &str| args.iter().any(|a| a == short || a == long);

    if has("-V", "--version") {
        println!("corewatch {}", config::VERSION);
        return glib::ExitCode::SUCCESS;
    }
    if has("-h", "--help") {
        print_help();
        return glib::ExitCode::SUCCESS;
    }

    // Logging starts before anything else; the guard flushes the log file on exit.
    let _log_guard = logging::init(has("-v", "--verbose"));

    // The UI (Blueprint templates, style.css) is compiled into the binary by build.rs.
    if let Err(error) = register_resources() {
        tracing::error!(%error, "cannot load the built-in UI resources");
        return glib::ExitCode::FAILURE;
    }

    // The name notifications and the window list show.
    glib::set_application_name("Corewatch");
    let app = adw::Application::builder()
        .application_id(config::APP_ID)
        .build();
    // GTK takes the text direction from the system language's script, not
    // from translations: keep the English interface left to right.
    app.connect_startup(|_| gtk::Widget::set_default_direction(gtk::TextDirection::Ltr));
    app.connect_activate(window::present);
    // One Corewatch per session: when one runs already, this process only
    // asks it to show its window and exits. Say so, or a Corewatch that is
    // stuck, hidden or on another display looks like one that never starts.
    match app.register(gio::Cancellable::NONE) {
        Ok(()) if app.is_remote() => already_running(&app),
        Ok(()) => {}
        Err(error) => {
            // GTK then runs without the session bus, as its own instance.
            tracing::warn!(%error, "cannot register on the session bus");
        }
    }
    let code = app.run_with_args::<&str>(&[]);
    tracing::info!("exiting");
    code
}

/// Say that another Corewatch runs and is asked to show its window, and
/// how to end it when no window appears.
fn already_running(app: &adw::Application) {
    // Inside Flatpak the session bus is reached through xdg-dbus-proxy, so
    // the bus names the proxy's process, not Corewatch's: ending that one
    // would not help. `flatpak kill` finds the right one by the app's ID.
    if std::path::Path::new("/.flatpak-info").exists() {
        tracing::info!("already running: asking it to show its window");
        eprintln!(
            "corewatch: already running; asking it to show its window. If no window \
             appears, run `flatpak kill {id}` and start Corewatch again.",
            id = config::APP_ID,
        );
        return;
    }
    let shown = primary_pid(app).map_or_else(|| "?".to_owned(), |p| p.to_string());
    tracing::info!(pid = %shown, "already running: asking it to show its window");
    eprintln!(
        "corewatch: already running (process {shown}); asking it to show its window. \
         If no window appears, end that process and start Corewatch again."
    );
}

/// The process ID of the Corewatch that owns the app's name on the session
/// bus; `None` when the bus does not tell. Outside Flatpak only: inside it,
/// the bus names xdg-dbus-proxy.
fn primary_pid(app: &adw::Application) -> Option<u32> {
    let bus = app.dbus_connection()?;
    let reply = bus
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "GetConnectionUnixProcessID",
            Some(&(config::APP_ID,).to_variant()),
            Some(glib::VariantTy::new("(u)").ok()?),
            gio::DBusCallFlags::NONE,
            1000,
            gio::Cancellable::NONE,
        )
        .ok()?;
    reply.child_value(0).get::<u32>()
}

/// Corewatch is in English only. GTK, libadwaita and GLib bring their own
/// translations (dialog buttons, size units); `LANGUAGE=en` keeps all of it
/// in English, whatever the system language. It must be set before GTK starts, and
/// while no other thread runs: this is the first thing `main` does.
fn english_only() {
    std::env::set_var("LANGUAGE", "en");
}

/// Embed the GResource built by build.rs and register the custom widget
/// types that templates refer to, such as `$CwGraph`.
fn register_resources() -> Result<(), glib::Error> {
    gio::resources_register_include!("corewatch.gresource")?;
    widgets::Graph::ensure_type();
    widgets::PageFrame::ensure_type();
    preferences::PreferencesDialog::ensure_type();
    Ok(())
}

fn print_help() {
    println!(
        "Usage: corewatch [OPTIONS]

Options:
  -v, --verbose   Detailed logs in the terminal and the log file
  -V, --version   Print the version and exit
  -h, --help      Print this help and exit

Environment:
  {log}=<filter>   Log filter, e.g. debug or corewatch_core=trace
  {root}=<dir>  Read <dir>/proc and <dir>/sys instead of the real ones

Log files: {dir}",
        log = logging::LOG_ENV,
        root = corewatch_core::root::SYSROOT_ENV,
        dir = logging::log_dir().display(),
    );
}

#[cfg(test)]
mod tests {
    /// Every template must load: a Blueprint `id` that Rust expects but the
    /// file lacks only shows up at run time. Needs a display (CI runs the
    /// tests under xvfb-run); without one the check is skipped.
    #[test]
    fn templates_load() {
        super::register_resources().expect("resources");
        if gtk::init().is_err() {
            eprintln!("no display: skipping template check");
            return;
        }
        let app = adw::Application::builder()
            .application_id("io.github.amirmwhdi.Corewatch.Test")
            .build();
        let _window = super::window::Window::new(&app);
        let pages = [("cpu".to_owned(), "Processor".to_owned())];
        let _pref = super::preferences::PreferencesDialog::new(
            std::rc::Rc::new(std::cell::RefCell::new(None)),
            &pages,
        );
        for module in super::modules::all() {
            let page = (module.page)();
            let frame = super::widgets::PageFrame::new(&page.widget());
            // Two misses are tolerated, the third shows the banner, data hides it.
            assert!(!frame.set_has_data(false));
            assert!(!frame.set_has_data(false));
            assert!(frame.set_has_data(false));
            assert!(!frame.set_has_data(true));
        }
    }
}
