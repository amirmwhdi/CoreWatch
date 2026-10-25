//! `/proc/net/dev` and `/proc/net/wireless` parsing.
//!
//! `net/dev` has two header lines, then one line per interface:
//! `name: rx_bytes rx_packets … (8 receive counters) tx_bytes …`.

use crate::CollectError;
use std::collections::HashMap;

/// Byte counters of one interface, counting up since it came up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetCounters {
    pub rx: u64,
    pub tx: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetDevStat {
    pub name: String,
    pub counters: NetCounters,
}

/// Parse every usable line of `/proc/net/dev`.
///
/// Header lines (no `:`) are skipped. A file with interface lines of which
/// none can be read is an error.
pub fn parse_net_dev(text: &str) -> Result<Vec<NetDevStat>, CollectError> {
    let mut stats = Vec::new();
    let mut lines = 0usize;
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        lines += 1;
        let mut fields = rest.split_ascii_whitespace().map(str::parse::<u64>);
        let rx = fields.next().and_then(Result::ok);
        // Receive has 8 counters; transmit bytes is the 9th number.
        let tx = fields.nth(7).and_then(Result::ok);
        if let (Some(rx), Some(tx)) = (rx, tx) {
            stats.push(NetDevStat {
                name: name.trim().to_owned(),
                counters: NetCounters { rx, tx },
            });
        }
    }
    if lines > 0 && stats.is_empty() {
        return Err(CollectError::parse(
            "/proc/net/dev",
            format!("none of {lines} interface lines could be read"),
        ));
    }
    Ok(stats)
}

/// Link quality per Wi-Fi interface, 0.0..=1.0.
///
/// Lines look like `wlan0: 0000   54.  -56.  -256 …`: status, then the link
/// quality out of 70 (the scale every current driver uses).
pub fn parse_wireless(text: &str) -> HashMap<String, f32> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let quality: f32 = rest
                .split_ascii_whitespace()
                .nth(1)?
                .trim_end_matches('.')
                .parse()
                .ok()?;
            Some((name.trim().to_owned(), (quality / 70.0).clamp(0.0, 1.0)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET_DEV: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:  123456     100    0    0    0     0          0         0   123456     100    0    0    0     0       0          0
wlp2s0: 9876543210 6000000 0 12 0 0 0 0 1234567890 3000000 0 0 0 0 0 0
";

    #[test]
    fn reads_receive_and_transmit_bytes() {
        let stats = parse_net_dev(NET_DEV).unwrap();
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].name, "lo");
        assert_eq!(
            stats[1],
            NetDevStat {
                name: "wlp2s0".into(),
                counters: NetCounters {
                    rx: 9_876_543_210,
                    tx: 1_234_567_890
                }
            }
        );
    }

    #[test]
    fn empty_file_is_fine_but_garbage_is_not() {
        assert!(parse_net_dev("header only\n").unwrap().is_empty());
        assert!(parse_net_dev("eth0: x y z\n").is_err());
    }

    #[test]
    fn wireless_quality_is_a_share_of_seventy() {
        let text = "\
Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE
 face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22
wlp2s0: 0000   56.  -54.  -256        0      0      0      0     12        0
";
        let q = parse_wireless(text);
        assert_eq!(q.len(), 1);
        assert!((q["wlp2s0"] - 0.8).abs() < 1e-6);
    }
}
