//! Parsing of `/proc/<pid>/stat`, `/proc/<pid>/statm`, `/etc/passwd`, and
//! the output of the Flatpak host script.

use std::collections::HashMap;

/// The fields Corewatch needs from `/proc/<pid>/stat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcStat {
    pub pid: u32,
    /// The kernel's short name (`comm`), at most 15 bytes.
    pub comm: String,
    /// Single-letter state: `R` running, `S` sleeping, `Z` zombie, …
    pub state: char,
    /// The parent's pid.
    pub ppid: u32,
    /// Scheduling niceness, -20 (first) to 19 (last).
    pub nice: i32,
    /// User plus system time, in clock ticks.
    pub cpu_ticks: u64,
    pub threads: u32,
    /// Start time after boot, in clock ticks. With the pid it identifies a
    /// process even when the pid is reused.
    pub start_ticks: u64,
}

/// Parse one `/proc/<pid>/stat` line.
///
/// The name sits in parentheses and may itself contain spaces or `)`, so
/// the fields are counted from the last `)`.
pub fn parse_proc_stat(line: &str) -> Option<ProcStat> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    let pid = line[..open].trim().parse().ok()?;
    let comm = line.get(open + 1..close)?.to_owned();
    let fields: Vec<&str> = line.get(close + 1..)?.split_ascii_whitespace().collect();
    // `fields` starts at field 3 of proc(5).
    let field = |n: usize| fields.get(n - 3).copied();
    let state = field(3)?.chars().next()?;
    let ppid = field(4)?.parse().ok()?;
    let utime: u64 = field(14)?.parse().ok()?;
    let stime: u64 = field(15)?.parse().ok()?;
    let nice = field(19)?.parse().ok()?;
    let threads = field(20)?.parse().ok()?;
    let start_ticks = field(22)?.parse().ok()?;
    Some(ProcStat {
        pid,
        comm,
        state,
        ppid,
        nice,
        cpu_ticks: utime.saturating_add(stime),
        threads,
        start_ticks,
    })
}

/// Private memory in pages from `/proc/<pid>/statm`: resident minus shared,
/// the figure GNOME System Monitor shows as "Memory".
pub fn parse_statm(line: &str) -> Option<u64> {
    let mut fields = line.split_ascii_whitespace().skip(1);
    let resident: u64 = fields.next()?.parse().ok()?;
    let shared: u64 = fields.next()?.parse().ok()?;
    Some(resident.saturating_sub(shared))
}

/// User names by uid from `/etc/passwd`.
pub fn parse_passwd(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            let uid = fields.nth(1)?.parse().ok()?;
            Some((uid, name.to_owned()))
        })
        .collect()
}

/// The `Uid:` line of `/proc/<pid>/status`: the real uid.
pub fn parse_status_uid(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|l| l.strip_prefix("Uid:"))?
        .split_ascii_whitespace()
        .next()?
        .parse()
        .ok()
}

/// One process as read raw, before any math.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawProcess {
    pub stat: ProcStat,
    pub uid: Option<u32>,
    pub private_pages: u64,
    /// The cgroup line, when the scan brought it (the Flatpak host script);
    /// otherwise read once per new process.
    pub cgroup: Option<String>,
}

/// One scan of the Flatpak host script (see `source.rs`):
///
/// ```text
/// S cpu  4705 356 584 3699 23 23 0 0 0 0
/// P 1000
/// 1234 (bash) S 1 …
/// 3522 1200 900 …
/// C 0::/user.slice/…/app-gnome-org.gnome.Nautilus-42.scope
/// E
/// ```
///
/// A `P` line starts each process; a process whose lines do not parse (a name
/// with a newline in it) is skipped, and the next `P` line starts afresh.
pub fn parse_host_scan<'a>(
    lines: impl IntoIterator<Item = &'a str>,
) -> (Option<String>, Vec<RawProcess>) {
    let mut cpu_line = None;
    let mut processes = Vec::new();
    let mut lines = lines.into_iter().peekable();
    while let Some(line) = lines.next() {
        if let Some(cpu) = line.strip_prefix("S ") {
            cpu_line = Some(cpu.to_owned());
        } else if let Some(uid) = line.strip_prefix("P ") {
            let uid = uid.trim().parse().ok();
            let stat = lines
                .next_if(|l| !l.starts_with("P "))
                .and_then(parse_proc_stat);
            let pages = lines
                .next_if(|l| !l.starts_with("P "))
                .and_then(parse_statm);
            let cgroup = lines
                .next_if(|l| l.starts_with("C "))
                .map(|l| format!("{}\n", &l[2..]));
            if let (Some(stat), Some(private_pages)) = (stat, pages) {
                processes.push(RawProcess {
                    stat,
                    uid,
                    private_pages,
                    cgroup,
                });
            }
        }
    }
    (cpu_line, processes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "1234 (Web Content) S 1000 1234 1234 0 -1 4194560 \
        500 0 0 0 1500 250 0 0 20 0 27 0 98765 2000000000 30000 \
        18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0";

    #[test]
    fn reads_stat_fields_after_the_name() {
        let s = parse_proc_stat(STAT).unwrap();
        assert_eq!(s.pid, 1234);
        assert_eq!(s.comm, "Web Content");
        assert_eq!(s.state, 'S');
        assert_eq!(s.ppid, 1000);
        assert_eq!(s.nice, 0);
        assert_eq!(s.cpu_ticks, 1750);
        assert_eq!(s.threads, 27);
        assert_eq!(s.start_ticks, 98765);
    }

    #[test]
    fn names_with_parentheses_are_kept_whole() {
        let line = STAT.replace("(Web Content)", "(a) b (c))");
        assert_eq!(parse_proc_stat(&line).unwrap().comm, "a) b (c)");
        assert!(parse_proc_stat("12 (short) S 1").is_none());
    }

    #[test]
    fn statm_is_resident_minus_shared() {
        assert_eq!(parse_statm("5000 3000 1200 10 0 900 0"), Some(1800));
        assert_eq!(parse_statm("5000"), None);
    }

    #[test]
    fn passwd_and_status_give_user_names() {
        let users =
            parse_passwd("root:x:0:0:root:/root:/bin/bash\nami:x:1000:1000::/home/ami:/bin/sh\n");
        assert_eq!(users[&1000], "ami");
        assert_eq!(
            parse_status_uid("Name:\tbash\nUid:\t1000\t1000\t1000\t1000\n"),
            Some(1000)
        );
    }

    #[test]
    fn host_scan_skips_a_broken_process_and_resyncs() {
        let text = format!(
            "S cpu  1 2 3 4\nP 1000\n{STAT}\n3000 1200 900\nP 0\n77 (bro\nken) S\nP 0\n{}\n10 5 2\n",
            STAT.replacen("1234", "99", 1)
        );
        let (cpu, procs) = parse_host_scan(text.lines());
        assert_eq!(cpu.as_deref(), Some("cpu  1 2 3 4"));
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].uid, Some(1000));
        assert_eq!(procs[0].private_pages, 300);
        assert_eq!(procs[1].stat.pid, 99);
    }

    #[test]
    fn host_scan_brings_the_cgroup() {
        let text = format!(
            "P 1000\n{STAT}\n3000 1200 900\nC 0::/system.slice/cron.service\nP 0\n{STAT}\n1 1 1\n"
        );
        let (_, procs) = parse_host_scan(text.lines());
        assert_eq!(
            procs[0].cgroup.as_deref(),
            Some("0::/system.slice/cron.service\n")
        );
        // An older host script without the line.
        assert_eq!(procs[1].cgroup, None);
    }
}
