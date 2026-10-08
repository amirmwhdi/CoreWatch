//! `/proc/diskstats` parsing.
//!
//! Each line is `major minor name` followed by counters. Linux 2.6.25+ prints
//! 11 counters, 4.18+ adds 4 discard counters and 5.5+ adds 2 flush counters;
//! only the first 11 are used here, so every layout works.

use crate::CollectError;

/// Bytes per sector in `/proc/diskstats`. The kernel always counts in 512-byte
/// units here, whatever the disk's real block size.
pub const SECTOR_BYTES: u64 = 512;

/// The counters Corewatch needs for one device. All values count up since boot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IoCounters {
    pub sectors_read: u64,
    pub sectors_written: u64,
    /// Milliseconds the device had at least one request in flight.
    pub io_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskStat {
    pub name: String,
    pub counters: IoCounters,
}

/// Parse every usable line of `/proc/diskstats`.
///
/// Lines with fewer than 11 counters (partitions on very old kernels) or with
/// a non-number are skipped. An empty file is valid (no block devices); a
/// non-empty file where no line is usable is an error.
pub fn parse_diskstats(text: &str) -> Result<Vec<DiskStat>, CollectError> {
    let mut stats = Vec::new();
    let mut lines = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        lines += 1;
        if let Some(stat) = parse_line(line) {
            stats.push(stat);
        }
    }
    if lines > 0 && stats.is_empty() {
        return Err(CollectError::parse(
            "/proc/diskstats",
            format!("none of {lines} lines could be read"),
        ));
    }
    Ok(stats)
}

fn parse_line(line: &str) -> Option<DiskStat> {
    let mut fields = line.split_ascii_whitespace();
    let _major = fields.next()?;
    let _minor = fields.next()?;
    let name = fields.next()?;
    let numbers: Vec<u64> = fields.map(|f| f.parse().ok()).collect::<Option<_>>()?;
    if numbers.len() < 11 {
        return None;
    }
    // 1-based counter numbers from the kernel docs: 3 sectors read,
    // 7 sectors written, 10 time spent doing I/O.
    Some(DiskStat {
        name: name.to_owned(),
        counters: IoCounters {
            sectors_read: numbers[2],
            sectors_written: numbers[6],
            io_ms: numbers[9],
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_eleven_fifteen_and_seventeen_counter_layouts() {
        let text = "\
   8       0 sda 500 0 100000 900 100 0 20000 300 0 1100 1200
 259       0 nvme0n1 1000 0 200000 500 2000 0 400000 800 0 1000 1300 0 0 0 0
 259       5 nvme1n1 1 0 8 0 2 0 16 0 0 3 3 0 0 0 0 7 1
";
        let stats = parse_diskstats(text).unwrap();
        assert_eq!(stats.len(), 3);
        assert_eq!(stats[0].name, "sda");
        assert_eq!(
            stats[0].counters,
            IoCounters {
                sectors_read: 100_000,
                sectors_written: 20_000,
                io_ms: 1100
            }
        );
        assert_eq!(stats[1].counters.sectors_written, 400_000);
        assert_eq!(stats[2].counters.io_ms, 3);
    }

    #[test]
    fn skips_short_and_broken_lines() {
        let text = "\
   8       1 sda1 10 20 30 40
   8       2 sda2 x 0 0 0 0 0 0 0 0 0 0
   8       0 sda 1 0 2 0 3 0 4 0 0 5 5
";
        let stats = parse_diskstats(text).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].name, "sda");
    }

    #[test]
    fn large_counters_do_not_overflow() {
        let big = u64::MAX - 1;
        let text = format!("8 0 sda 0 0 {big} 0 0 0 {big} 0 0 {big} 0\n");
        let stats = parse_diskstats(&text).unwrap();
        assert_eq!(stats[0].counters.sectors_read, big);
    }

    #[test]
    fn empty_file_is_fine_but_garbage_is_not() {
        assert!(parse_diskstats("").unwrap().is_empty());
        assert!(parse_diskstats("hello world\n").is_err());
    }
}
