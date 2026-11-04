//! Pressure stall information (PSI) from `/proc/pressure/{cpu,memory,io}`.
//!
//! The kernel counts how long tasks could not run because they waited for a
//! resource: `some` is the share of time at least one task waited, `full`
//! the share when every task did (not reported for the CPU as a whole). It
//! answers "why is the system slow" better than the load average does.
//!
//! The values used are `avg10`, the kernel's average over the last ten
//! seconds. Kernels built without PSI (or booted with `psi=0`) have no such
//! files, or fail to read them; the pressure is then `None`.

use crate::SysRoot;

/// Waiting shares over the last ten seconds, 0.0..=1.0.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pressure {
    /// At least one task waited.
    pub some: f32,
    /// All non-idle tasks waited at once; `None` for the CPU on most kernels.
    pub full: Option<f32>,
}

/// The resources the kernel reports pressure for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    Cpu,
    Memory,
    Io,
}

impl Resource {
    fn file(self) -> &'static str {
        match self {
            Self::Cpu => "pressure/cpu",
            Self::Memory => "pressure/memory",
            Self::Io => "pressure/io",
        }
    }
}

/// Read one resource's pressure; `None` when the kernel has no PSI.
pub fn read(root: &SysRoot, resource: Resource) -> Option<Pressure> {
    parse(&root.read_proc(resource.file()).ok()?)
}

/// Parse a pressure file:
///
/// ```text
/// some avg10=1.23 avg60=0.80 avg300=0.25 total=123456
/// full avg10=0.00 avg60=0.00 avg300=0.00 total=0
/// ```
pub fn parse(text: &str) -> Option<Pressure> {
    let avg10 = |kind: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(kind)?.strip_prefix(' '))?
            .split_ascii_whitespace()
            .find_map(|field| field.strip_prefix("avg10="))?
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .map(|percent| (percent / 100.0).clamp(0.0, 1.0))
    };
    Some(Pressure {
        some: avg10("some")?,
        full: avg10("full"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_ten_second_averages() {
        let p = parse(
            "some avg10=12.50 avg60=3.00 avg300=1.00 total=5000\n\
             full avg10=2.00 avg60=0.50 avg300=0.10 total=100\n",
        )
        .unwrap();
        assert!((p.some - 0.125).abs() < 1e-6);
        assert!((p.full.unwrap() - 0.02).abs() < 1e-6);
    }

    #[test]
    fn full_is_optional_and_garbage_is_none() {
        let p = parse("some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n").unwrap();
        assert_eq!(p.some, 0.0);
        assert_eq!(p.full, None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("some avg10=x\n"), None);
    }
}
