//! Corewatch: a modern system monitor for Linux.

mod compat;
mod config;
mod i18n;
mod logging;
mod modules;
mod widgets;
mod window;

use adw::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
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
    i18n::init();

    // The UI (Blueprint templates, style.css) is compiled into the binary by build.rs.
    if let Err(error) = register_resources() {
        tracing::error!(%error, "cannot load the built-in UI resources");
        return glib::ExitCode::FAILURE;
    }

    let app = adw::Application::builder()
        .application_id(config::APP_ID)
        .build();
    app.connect_activate(window::present);
    let code = app.run_with_args::<&str>(&[]);
    tracing::info!("exiting");
    code
}

/// Embed the GResource built by build.rs and register the custom widget
/// types that templates refer to, such as `$CwGraph`.
fn register_resources() -> Result<(), glib::Error> {
    gio::resources_register_include!("corewatch.gresource")?;
    widgets::Graph::ensure_type();
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
  {log}=<filter>        Log filter, e.g. debug or corewatch_core=trace
  {root}=<dir>    Read <dir>/proc and <dir>/sys instead of the real ones

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
        for module in super::modules::all() {
            let _page = (module.page)();
        }
    }
}
