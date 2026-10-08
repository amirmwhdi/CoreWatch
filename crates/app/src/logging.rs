//! Diagnostic logging: terminal, rotating log file, startup banner, panic hook.
//!
//! - stderr: `warn` by default, `debug` with `--verbose`
//! - file: `info` by default, `debug` with `--verbose`, in
//!   `$XDG_STATE_HOME/corewatch/logs` (usually `~/.local/state/corewatch/logs`),
//!   rotated daily, 7 files kept, written on a background thread
//! - `COREWATCH_LOG=<filter>` overrides both levels (same syntax as `RUST_LOG`)

use crate::config;
use corewatch_core::SysRoot;
use std::path::{Path, PathBuf};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Environment variable with a tracing filter, e.g. `corewatch_core=debug`.
pub const LOG_ENV: &str = "COREWATCH_LOG";

/// Log files kept before the oldest is deleted (one per day).
const KEEP_FILES: usize = 7;

/// Keep this alive until the app exits, or buffered log lines are lost.
pub struct LogGuard {
    _file: Option<WorkerGuard>,
}

/// `$XDG_STATE_HOME/corewatch/logs`, falling back to `~/.local/state/...`.
/// Inside Flatpak, `XDG_STATE_HOME` already points into the app's sandbox.
pub fn log_dir() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    state.join("corewatch").join("logs")
}

/// Set up logging. Call first in `main`, before GTK starts.
pub fn init(verbose: bool) -> LogGuard {
    let filter = |default: &str| {
        EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new(default))
    };

    let stderr = fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(filter(if verbose { "debug" } else { "warn" }));

    let dir = log_dir();
    let (file, guard) = match open_log_file(&dir) {
        Ok(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = fmt::layer()
                .with_ansi(false)
                .with_thread_names(true)
                .with_writer(writer)
                .with_filter(filter(if verbose { "debug" } else { "info" }));
            (Some(layer), Some(guard))
        }
        Err(error) => {
            // The subscriber does not exist yet, so this is the one place
            // where printing directly is correct.
            eprintln!(
                "corewatch: file logging disabled ({}): {error}",
                dir.display()
            );
            (None, None)
        }
    };

    tracing_subscriber::registry()
        .with(stderr)
        .with(file)
        .init();
    install_panic_hook();
    LogGuard { _file: guard }
}

fn open_log_file(
    dir: &Path,
) -> Result<tracing_appender::rolling::RollingFileAppender, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    Ok(Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("corewatch")
        .filename_suffix("log")
        .max_log_files(KEEP_FILES)
        .build(dir)?)
}

/// Write panics (message, thread, backtrace) to the log before the default
/// handler prints them and the process ends.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(
            thread = thread.name().unwrap_or("unnamed"),
            panic = %info,
            %backtrace,
            "panic"
        );
        default_hook(info);
    }));
}

/// One `info` line that answers "what system is this?" in every bug report.
pub fn log_startup_banner(sysroot: &SysRoot) {
    let distro = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|os| {
            os.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_owned())
        })
        .unwrap_or_else(|| "unknown".into());
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "unknown".into());
    let env_or = |key: &str| std::env::var(key).unwrap_or_else(|_| "unknown".into());

    tracing::info!(
        version = config::VERSION,
        app_id = config::APP_ID,
        build_level = "stable",
        gtk = %format!("{}.{}.{}", gtk::major_version(), gtk::minor_version(), gtk::micro_version()),
        adw = %format!("{}.{}.{}", adw::major_version(), adw::minor_version(), adw::micro_version()),
        %distro,
        %kernel,
        flatpak = Path::new("/.flatpak-info").exists(),
        session = %env_or("XDG_SESSION_TYPE"),
        desktop = %env_or("XDG_CURRENT_DESKTOP"),
        ?sysroot,
        log_dir = %log_dir().display(),
        "starting"
    );
}
