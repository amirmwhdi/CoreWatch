//! `/proc/meminfo` parsing.

use super::MemorySample;
use crate::CollectError;

/// Parse `/proc/meminfo` into bytes.
///
/// `used` follows `free(1)`: total minus available. Cached includes
/// reclaimable slab, again like `free(1)`.
pub fn parse_meminfo(text: &str) -> Result<MemorySample, CollectError> {
    let mut mem_total = 0u64;
    let mut mem_free = 0u64;
    let mut mem_available = None;
    let mut buffers = 0u64;
    let mut cached = 0u64;
    let mut sreclaimable = 0u64;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;

    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(val_str) = rest.split_ascii_whitespace().next() else {
            continue;
        };
        let Ok(val) = val_str.parse::<u64>() else {
            continue;
        };
        let bytes = val.saturating_mul(1024);

        match key.trim() {
            "MemTotal" => mem_total = bytes,
            "MemFree" => mem_free = bytes,
            "MemAvailable" => mem_available = Some(bytes),
            "Buffers" => buffers = bytes,
            "Cached" => cached = bytes,
            "SReclaimable" => sreclaimable = bytes,
            "SwapTotal" => swap_total = bytes,
            "SwapFree" => swap_free = bytes,
            _ => {}
        }
    }

    if mem_total == 0 {
        return Err(CollectError::parse("/proc/meminfo", "MemTotal missing"));
    }
    let cached_total = cached + sreclaimable;
    let available = mem_available
        .unwrap_or_else(|| mem_free + buffers + cached_total)
        .min(mem_total);

    Ok(MemorySample {
        total: mem_total,
        available,
        used: mem_total - available,
        cached: cached_total,
        buffers,
        swap_total,
        swap_used: swap_total.saturating_sub(swap_free),
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
