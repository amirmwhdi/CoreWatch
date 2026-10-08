//! Corewatch: a modern system monitor for Linux.

mod compat;
mod config;
mod modules;
mod widgets;
mod window;

use adw::prelude::*;
use gtk::glib;

fn main() -> glib::ExitCode {
    // Our own flags are handled here, before GApplication sees the arguments.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("corewatch {}", config::VERSION);
        return glib::ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage: corewatch [--version] [--help]");
        println!(
            "Environment: RUST_LOG=debug for logs, COREWATCH_SYSROOT=<dir> to read a fake /proc"
        );
        return glib::ExitCode::SUCCESS;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let app = adw::Application::builder()
        .application_id(config::APP_ID)
        .build();
    app.connect_activate(window::present);
    app.run_with_args::<&str>(&[])
}
