//! System module: uptime and load average, from `/proc/uptime` and
//! `/proc/loadavg`. Two tiny files, read every tick.

use crate::{CollectError, Collector, Snapshot, SysRoot};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SystemSample {
    /// Seconds since boot.
    pub uptime_secs: u64,
    /// Average number of runnable tasks over 1, 5 and 15 minutes.
    pub load: [f32; 3],
}

/// Parse `/proc/uptime`: `"12345.67 45678.90"`, seconds up and idle.
pub fn parse_uptime(text: &str) -> Result<u64, CollectError> {
    text.split_ascii_whitespace()
        .next()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| v as u64)
        .ok_or_else(|| CollectError::parse("/proc/uptime", text.trim().to_owned()))
}

/// Parse `/proc/loadavg`: `"0.52 0.58 0.59 2/812 12345"`.
pub fn parse_loadavg(text: &str) -> Result<[f32; 3], CollectError> {
    let mut fields = text.split_ascii_whitespace().map(str::parse::<f32>);
    let mut load = [0.0; 3];
    for slot in &mut load {
        *slot = fields
            .next()
            .and_then(Result::ok)
            .ok_or_else(|| CollectError::parse("/proc/loadavg", text.trim().to_owned()))?;
    }
    Ok(load)
}

pub struct SystemCollector;

impl Collector for SystemCollector {
    fn name(&self) -> &'static str {
        "system"
    }

    fn collect(&mut self, root: &SysRoot, out: &mut Snapshot) -> Result<(), CollectError> {
        out.system = Some(SystemSample {
            uptime_secs: parse_uptime(&root.read_proc("uptime")?)?,
            load: parse_loadavg(&root.read_proc("loadavg")?)?,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_uptime_and_load() {
        assert_eq!(parse_uptime("93784.51 300000.12\n").unwrap(), 93784);
        assert_eq!(
            parse_loadavg("0.52 1.50 2.25 2/812 12345\n").unwrap(),
            [0.52, 1.5, 2.25]
        );
        assert!(parse_uptime("").is_err());
        assert!(parse_loadavg("0.5 x").is_err());
    }
}
