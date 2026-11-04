//! Everything about one process, for its details window: read on demand,
//! not on every scan.

use super::parse::{parse_proc_stat, ProcStat};
use super::source::ProcessSource;
use std::io;
use std::process::{Command, Stdio};

/// One process, as its details window shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Details {
    /// The arguments, joined by spaces; empty for a kernel thread.
    pub command_line: String,
    /// The program file; `None` when it cannot be read (another user's
    /// process, a kernel thread).
    pub executable: Option<String>,
    pub ppid: u32,
    pub state: char,
    pub threads: u32,
    /// -20 (runs first) to 19 (runs last).
    pub nice: i32,
    /// Seconds after boot the process started.
    pub started_secs: f64,
    /// Bytes read from and written to storage since the process started;
    /// `None` for other users' processes.
    pub io: Option<(u64, u64)>,
    /// Inodes of its sockets; `None` when its descriptors cannot be read.
    pub sockets: Option<Vec<u32>>,
}

/// Read the details of the process that started at `start_ticks`.
/// [`io::ErrorKind::NotFound`] when it has ended (or its pid was reused).
pub fn details(source: ProcessSource, pid: u32, start_ticks: u64) -> io::Result<Details> {
    let parts = match source {
        ProcessSource::Direct => read_direct(pid),
        ProcessSource::FlatpakHost => read_host(pid)?,
    };
    let stat = parts
        .stat
        .as_deref()
        .and_then(parse_proc_stat)
        .filter(|s| s.start_ticks == start_ticks)
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    Ok(build(stat, parts))
}

/// The raw parts, as read from `/proc/<pid>`.
#[derive(Default)]
struct Parts {
    stat: Option<String>,
    command_line: String,
    executable: Option<String>,
    io: Option<(u64, u64)>,
    sockets: Option<Vec<u32>>,
}

fn build(stat: ProcStat, parts: Parts) -> Details {
    let hz = rustix::param::clock_ticks_per_second().max(1) as f64;
    Details {
        command_line: parts.command_line,
        executable: parts.executable,
        ppid: stat.ppid,
        state: stat.state,
        threads: stat.threads,
        nice: stat.nice,
        started_secs: stat.start_ticks as f64 / hz,
        io: parts.io,
        sockets: parts.sockets,
    }
}

fn read_direct(pid: u32) -> Parts {
    let dir = format!("/proc/{pid}");
    let read = |name: &str| std::fs::read(format!("{dir}/{name}")).ok();
    let text = |bytes: Vec<u8>| String::from_utf8_lossy(&bytes).into_owned();
    let sockets = std::fs::read_dir(format!("{dir}/fd")).ok().map(|fds| {
        fds.flatten()
            .filter_map(|fd| socket_inode(&std::fs::read_link(fd.path()).ok()?.to_string_lossy()))
            .collect()
    });
    Parts {
        stat: read("stat").map(text).map(|s| s.trim_end().to_owned()),
        command_line: read("cmdline").map(|b| join_args(&b)).unwrap_or_default(),
        executable: std::fs::read_link(format!("{dir}/exe"))
            .ok()
            .map(|p| p.to_string_lossy().into_owned()),
        io: read("io").map(text).and_then(|t| parse_io(&t)),
        sockets,
    }
}

/// Inside Flatpak: the same files, read on the host by one shell.
const HOST_DETAILS: &str = r#"
d=/proc/$1
read -r st < "$d/stat" || exit 3
printf 'S %s\n' "$st"
printf 'C '; tr '\0' ' ' < "$d/cmdline" 2>/dev/null; echo
x=$(readlink "$d/exe" 2>/dev/null) && printf 'X %s\n' "$x"
while read -r k v; do
  case $k in read_bytes:|write_bytes:) printf 'I %s %s\n' "$k" "$v" ;; esac
done < "$d/io" 2>/dev/null
if [ -r "$d/fd" ]; then
  echo D
  ls -l "$d/fd" 2>/dev/null | while IFS= read -r l; do
    case $l in *socket:\[*) printf 'F %s\n' "${l##*socket:\[}" ;; esac
  done
fi
"#;

fn read_host(pid: u32) -> io::Result<Parts> {
    let output = Command::new("flatpak-spawn")
        .args(["--host", "sh", "-c", HOST_DETAILS, "sh"])
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if output.status.code() == Some(3) {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    }
    Ok(parse_host(&String::from_utf8_lossy(&output.stdout)))
}

/// The output of [`HOST_DETAILS`].
fn parse_host(text: &str) -> Parts {
    let mut parts = Parts::default();
    let (mut read, mut written) = (None, None);
    for line in text.lines() {
        if let Some(stat) = line.strip_prefix("S ") {
            parts.stat = Some(stat.to_owned());
        } else if let Some(args) = line.strip_prefix("C ") {
            parts.command_line = args.trim_end().to_owned();
        } else if let Some(exe) = line.strip_prefix("X ") {
            parts.executable = Some(exe.to_owned());
        } else if let Some(io) = line.strip_prefix("I ") {
            match io.split_once(' ') {
                Some(("read_bytes:", n)) => read = n.trim().parse().ok(),
                Some(("write_bytes:", n)) => written = n.trim().parse().ok(),
                _ => {}
            }
        } else if line == "D" {
            parts.sockets = Some(Vec::new());
        } else if let Some(inode) = line.strip_prefix("F ") {
            if let (Some(list), Ok(inode)) =
                (&mut parts.sockets, inode.trim_end_matches(']').parse())
            {
                list.push(inode);
            }
        }
    }
    parts.io = read.zip(written);
    parts
}

/// `/proc/<pid>/cmdline`: arguments separated (and ended) by NUL.
fn join_args(raw: &[u8]) -> String {
    raw.split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(String::from_utf8_lossy)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `read_bytes` and `write_bytes` of `/proc/<pid>/io`: what reached storage.
fn parse_io(text: &str) -> Option<(u64, u64)> {
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key))?
            .trim()
            .parse()
            .ok()
    };
    Some((field("read_bytes:")?, field("write_bytes:")?))
}

fn socket_inode(link: &str) -> Option<u32> {
    link.strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_details() {
        let me = std::process::id();
        let stat = std::fs::read_to_string(format!("/proc/{me}/stat")).unwrap();
        let start = parse_proc_stat(stat.trim_end()).unwrap().start_ticks;
        let d = details(ProcessSource::Direct, me, start).unwrap();
        assert!(!d.command_line.is_empty());
        assert!(d.executable.is_some());
        assert!(d.threads >= 1);
        assert!(d.io.is_some());
        assert!(d.sockets.is_some());
        // Another start time: another process, reported as gone.
        let gone = details(ProcessSource::Direct, me, start + 1).unwrap_err();
        assert_eq!(gone.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn the_host_script_reports_the_same() {
        // The script Flatpak runs on the host, run here on this process.
        let me = std::process::id();
        let out = Command::new("sh")
            .args(["-c", HOST_DETAILS, "sh"])
            .arg(me.to_string())
            .output()
            .unwrap();
        let host = parse_host(&String::from_utf8_lossy(&out.stdout));
        let direct = read_direct(me);
        assert_eq!(host.command_line, direct.command_line);
        assert_eq!(host.executable, direct.executable);
        assert!(host.io.is_some());
        assert!(host.sockets.is_some());
        assert!(host.stat.is_some());
    }

    #[test]
    fn command_lines_and_io() {
        assert_eq!(
            join_args(b"/usr/bin/foo\0--bar\0baz qux\0"),
            "/usr/bin/foo --bar baz qux"
        );
        assert_eq!(join_args(b""), "");
        let io = "rchar: 10\nwchar: 20\nsyscr: 1\nsyscw: 1\nread_bytes: 4096\nwrite_bytes: 8192\ncancelled_write_bytes: 0\n";
        assert_eq!(parse_io(io), Some((4096, 8192)));
    }
}
