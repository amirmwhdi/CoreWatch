//! Static CPU facts from `/proc/cpuinfo`, and the per-core frequency from sysfs.

use crate::SysRoot;
use std::collections::HashSet;

/// Facts that do not change while the app runs. Read once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CpuInfo {
    /// Marketing name, e.g. "AMD Ryzen 7 5800X 8-Core Processor".
    pub model: String,
    /// Logical CPUs (threads) listed in cpuinfo.
    pub logical: usize,
    /// Physical cores, when the kernel reports package and core ids (x86).
    pub physical: Option<usize>,
}

const UNKNOWN_MODEL: &str = "Unknown processor";

/// Parse `/proc/cpuinfo`. Never fails: missing fields fall back to defaults.
pub fn parse_cpuinfo(text: &str) -> CpuInfo {
    let mut model: Option<String> = None;
    let mut logical = 0;
    let mut cores = HashSet::new();
    let mut package: Option<String> = None;

    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "processor" => {
                logical += 1;
                package = None;
            }
            // x86 uses "model name"; many ARM kernels only give "Model" or "Hardware".
            "model name" | "Model" | "Hardware" if model.is_none() && !value.is_empty() => {
                model = Some(value.to_owned());
            }
            "physical id" => package = Some(value.to_owned()),
            "core id" => {
                if let Some(p) = &package {
                    cores.insert((p.clone(), value.to_owned()));
                }
            }
            _ => {}
        }
    }

    CpuInfo {
        model: model.unwrap_or_else(|| UNKNOWN_MODEL.to_owned()),
        logical,
        physical: (!cores.is_empty()).then_some(cores.len()),
    }
}

/// Current frequency of logical core `id` in MHz, if the kernel exposes it.
/// Missing in most VMs and on some ARM boards.
pub fn read_freq_mhz(root: &SysRoot, id: usize) -> Option<u32> {
    let khz = root
        .read_sys(&format!(
            "devices/system/cpu/cpu{id}/cpufreq/scaling_cur_freq"
        ))
        .ok()?;
    khz.trim().parse::<u32>().ok().map(|k| k / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    const X86_SMT: &str = "\
processor\t: 0
model name\t: Test CPU 9000
physical id\t: 0
core id\t\t: 0

processor\t: 1
model name\t: Test CPU 9000
physical id\t: 0
core id\t\t: 1

processor\t: 2
model name\t: Test CPU 9000
physical id\t: 0
core id\t\t: 0

processor\t: 3
model name\t: Test CPU 9000
physical id\t: 0
core id\t\t: 1
";

    const ARM: &str = "\
processor\t: 0
BogoMIPS\t: 108.00
CPU implementer\t: 0x41

processor\t: 1
BogoMIPS\t: 108.00
CPU implementer\t: 0x41

Hardware\t: BCM2835
Model\t\t: Raspberry Pi 4 Model B Rev 1.4
";

    #[test]
    fn x86_with_hyperthreading() {
        let info = parse_cpuinfo(X86_SMT);
        assert_eq!(info.model, "Test CPU 9000");
        assert_eq!(info.logical, 4);
        assert_eq!(info.physical, Some(2));
    }

    #[test]
    fn arm_without_model_name() {
        let info = parse_cpuinfo(ARM);
        assert_eq!(info.model, "BCM2835");
        assert_eq!(info.logical, 2);
        assert_eq!(info.physical, None);
    }

    #[test]
    fn empty_input_falls_back() {
        let info = parse_cpuinfo("");
        assert_eq!(info.model, UNKNOWN_MODEL);
        assert_eq!(info.logical, 0);
    }
}
