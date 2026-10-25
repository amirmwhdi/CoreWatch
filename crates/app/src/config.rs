use std::time::Duration;

pub const APP_ID: &str = "io.github.amirmwhdi.Corewatch";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default time between two samples.
pub const DEFAULT_SAMPLE_INTERVAL: Duration = Duration::from_millis(1000);

/// Points kept per graph: 60 samples = 60 seconds at the default interval.
pub const HISTORY_LEN: usize = 60;

/// The last commit this build came from: hash, subject, blank line, body.
/// Empty when the source had no git history (see `build.rs`).
const COMMIT: &str = include_str!(concat!(env!("OUT_DIR"), "/commit.txt"));

/// The last commit, split into its parts.
pub struct Commit {
    pub hash: &'static str,
    pub subject: &'static str,
    /// Paragraphs of the body, each joined into one line.
    pub paragraphs: Vec<String>,
}

pub fn commit() -> Option<Commit> {
    let mut lines = COMMIT.lines();
    let hash = lines.next().filter(|h| !h.is_empty())?;
    let subject = lines.next().unwrap_or_default();
    let body: Vec<&str> = lines.collect();
    let paragraphs = body
        .split(|line| line.trim().is_empty())
        .filter(|p| !p.is_empty())
        .map(|p| p.iter().map(|l| l.trim()).collect::<Vec<_>>().join(" "))
        .collect();
    Some(Commit {
        hash,
        subject,
        paragraphs,
    })
}
