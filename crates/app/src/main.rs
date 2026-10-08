//! Corewatch: a modern system monitor for Linux.

mod compat;
mod config;
mod logging;
mod modules;
mod widgets;
mod window;

use adw::prelude::*;
use gtk::glib;

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

    let app = adw::Application::builder()
        .application_id(config::APP_ID)
        .build();
    app.connect_activate(window::present);
    let code = app.run_with_args::<&str>(&[]);
    tracing::info!("exiting");
    code
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
