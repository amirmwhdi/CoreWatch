//! `/proc/meminfo` parsing.

use super::MemorySample;
use crate::CollectError;
use std::collections::HashMap;

/// Parse `/proc/meminfo` into bytes.
///
/// `used` follows `free(1)`: total minus available. Cached includes
/// reclaimable slab, again like `free(1)`.
pub fn parse_meminfo(text: &str) -> Result<MemorySample, CollectError> {
    let kib: HashMap<&str, u64> = text
        .lines()
        .filter_map(|line| {
            let (key, rest) = line.split_once(':')?;
            let value = rest.split_ascii_whitespace().next()?.parse().ok()?;
            Some((key.trim(), value))
        })
        .collect();
    let get = |key: &str| kib.get(key).copied().unwrap_or(0).saturating_mul(1024);

    let total = get("MemTotal");
    if total == 0 {
        return Err(CollectError::parse("/proc/meminfo", "MemTotal missing"));
    }
    let cached = get("Cached") + get("SReclaimable");
    let buffers = get("Buffers");
    // MemAvailable exists since Linux 3.14; estimate it on anything older.
    let available = if kib.contains_key("MemAvailable") {
        get("MemAvailable")
    } else {
        get("MemFree") + buffers + cached
    }
    .min(total);
    let swap_total = get("SwapTotal");

    Ok(MemorySample {
        total,
        available,
        used: total - available,
        cached,
        buffers,
        swap_total,
        swap_used: swap_total.saturating_sub(get("SwapFree")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    const NORMAL: &str = "\
MemTotal:       16384000 kB
MemFree:         2048000 kB
MemAvailable:    8192000 kB
Buffers:          512000 kB
Cached:          4096000 kB
SwapCached:            0 kB
SReclaimable:     256000 kB
SwapTotal:       4096000 kB
SwapFree:        3072000 kB
";

    #[test]
    fn normal_sample() {
        let m = parse_meminfo(NORMAL).unwrap();
        assert_eq!(m.total, 16_384_000 * 1024);
        assert_eq!(m.available, 8_192_000 * 1024);
        assert_eq!(m.used, 8_192_000 * 1024);
        assert_eq!(m.cached, (4_096_000 + 256_000) * 1024);
        assert_eq!(m.buffers, 512_000 * 1024);
        assert_eq!(m.swap_used, 1_024_000 * 1024);
        assert_eq!(m.used_fraction(), 0.5);
        assert_eq!(m.swap_fraction(), 0.25);
    }

    #[test]
    fn old_kernel_without_memavailable() {
        let m = parse_meminfo(
            "MemTotal: 4194304 kB\nMemFree: 1048576 kB\nBuffers: 0 kB\nCached: 1048576 kB\n",
        )
        .unwrap();
        assert_eq!(m.available, 2 * GIB);
        assert_eq!(m.used, 2 * GIB);
    }

    #[test]
    fn no_swap() {
        let m = parse_meminfo("MemTotal: 1024 kB\nMemAvailable: 512 kB\n").unwrap();
        assert_eq!(m.swap_total, 0);
        assert_eq!(m.swap_fraction(), 0.0);
    }

    #[test]
    fn missing_total_is_an_error() {
        assert!(parse_meminfo("MemFree: 1 kB\n").is_err());
    }
}
