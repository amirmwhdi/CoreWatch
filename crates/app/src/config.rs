use std::time::Duration;

pub const APP_ID: &str = "io.github.amirmwhdi.Corewatch";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default time between two samples.
pub const DEFAULT_SAMPLE_INTERVAL: Duration = Duration::from_millis(1000);

/// Points kept per graph: 60 samples = 60 seconds at the default interval.
pub const HISTORY_LEN: usize = 60;
