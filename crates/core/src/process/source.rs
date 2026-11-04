//! Where the process list comes from.
//!
//! Outside Flatpak, `/proc` is read directly. Inside Flatpak the sandbox has
//! its own pid namespace and sees only its own processes, so a small POSIX
//! shell script runs on the host through `flatpak-spawn --host` and prints
//! the same files. The script uses shell builtins only: one scan of 500
//! processes starts no new program, and it stays alive between scans.

use super::net::{parse_fd_listing, Owners};
use super::parse::{self, parse_proc_stat, parse_statm, parse_status_uid, RawProcess};
use crate::{CollectError, SysRoot};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// Commands on stdin: `u` prints `/etc/passwd`, `s` scans, `f` lists the
/// sockets of every readable process (see `net::parse_fd_listing`); each ends
/// with `E`. `ls` is the one program it starts, once per `f`.
const HOST_SCRIPT: &str = r#"
while read -r cmd; do
  case $cmd in
  u) while IFS= read -r l; do printf 'U %s\n' "$l"; done < /etc/passwd ;;
  s)
    read -r l < /proc/stat && printf 'S %s\n' "$l"
    for d in /proc/[0-9]*; do
      read -r st < "$d/stat" 2>/dev/null || continue
      read -r sm < "$d/statm" 2>/dev/null || continue
      u=
      while read -r k v _; do
        if [ "$k" = Uid: ]; then u=$v; break; fi
      done < "$d/status" 2>/dev/null
      printf 'P %s\n%s\n%s\n' "$u" "$st" "$sm"
    done ;;
  f)
    ls -l /proc/[0-9]*/fd 2>/dev/null | while IFS= read -r l; do
      case $l in
      /proc/*:) printf 'D %s\n' "$l" ;;
      *socket:\[*) printf 'F %s\n' "${l##*socket:\[}" ;;
      esac
    done ;;
  esac
  echo E
done
"#;

/// How processes are read and signaled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSource {
    /// Read `/proc` of the given root and signal with `kill(2)`.
    Direct,
    /// Inside Flatpak: run on the host with `flatpak-spawn --host`.
    FlatpakHost,
}

impl ProcessSource {
    /// [`ProcessSource::FlatpakHost`] inside a Flatpak sandbox reading the
    /// real machine, otherwise [`ProcessSource::Direct`].
    pub fn detect(root: &SysRoot) -> Self {
        if root.is_host() && std::path::Path::new("/.flatpak-info").exists() {
            Self::FlatpakHost
        } else {
            Self::Direct
        }
    }
}

/// One scan: the aggregate `cpu` line of `/proc/stat` and every process.
pub(super) struct Scan {
    pub cpu_line: Option<String>,
    pub processes: Vec<RawProcess>,
}

pub(super) enum Reader {
    Direct { buf: Vec<u8> },
    Host(Option<HostShell>),
}

impl Reader {
    pub fn new(source: ProcessSource) -> Self {
        match source {
            ProcessSource::Direct => Self::Direct {
                buf: Vec::with_capacity(4096),
            },
            ProcessSource::FlatpakHost => Self::Host(None),
        }
    }

    pub fn passwd(&mut self) -> String {
        match self {
            Self::Direct { .. } => std::fs::read_to_string("/etc/passwd").unwrap_or_default(),
            Self::Host(shell) => with_shell(shell, |s| {
                s.request("u").map(|lines| {
                    lines.iter().filter_map(|l| l.strip_prefix("U ")).fold(
                        String::new(),
                        |mut text, l| {
                            text.push_str(l);
                            text.push('\n');
                            text
                        },
                    )
                })
            })
            .unwrap_or_default(),
        }
    }

    /// Whether GPU usage per process can be read: only directly, not from
    /// inside Flatpak (the host's fdinfo files are not visible there).
    pub fn reads_gpu(&self) -> bool {
        matches!(self, Self::Direct { .. })
    }

    /// Which processes hold GPU descriptors (`/dev/dri/*`), among `pids`.
    pub fn gpu_fds(&self, root: &SysRoot, pids: &[u32]) -> HashMap<u32, Vec<u32>> {
        let mut found = HashMap::new();
        for &pid in pids {
            // Other users' descriptors cannot be read without root.
            let Ok(fds) = std::fs::read_dir(root.proc_path(&format!("{pid}/fd"))) else {
                continue;
            };
            let gpu: Vec<u32> = fds
                .flatten()
                .filter(|fd| {
                    std::fs::read_link(fd.path())
                        .is_ok_and(|target| target.starts_with("/dev/dri/"))
                })
                .filter_map(|fd| fd.file_name().to_str()?.parse().ok())
                .collect();
            if !gpu.is_empty() {
                found.insert(pid, gpu);
            }
        }
        found
    }

    /// One GPU descriptor's fdinfo text.
    pub fn fdinfo(&self, root: &SysRoot, pid: u32, fd: u32) -> Option<String> {
        root.read_proc(&format!("{pid}/fdinfo/{fd}")).ok()
    }

    /// Which process owns which socket. Empty when it cannot be read.
    pub fn socket_owners(&mut self, root: &SysRoot) -> Owners {
        match self {
            Self::Direct { .. } => socket_owners_direct(root),
            Self::Host(shell) => with_shell(shell, |s| s.request("f"))
                .map(|lines| parse_fd_listing(lines.iter().map(String::as_str)))
                .unwrap_or_default(),
        }
    }

    pub fn scan(&mut self, root: &SysRoot) -> Result<Scan, CollectError> {
        match self {
            Self::Direct { buf } => scan_direct(root, buf),
            Self::Host(shell) => {
                let lines =
                    with_shell(shell, |s| s.request("s")).map_err(|source| CollectError::Io {
                        path: "flatpak-spawn --host".into(),
                        source,
                    })?;
                let (cpu_line, processes) =
                    parse::parse_host_scan(lines.iter().map(String::as_str));
                Ok(Scan {
                    cpu_line,
                    processes,
                })
            }
        }
    }
}

/// One scan of `/proc`, in as few system calls as it takes (seven per
/// process): `/proc` is opened once, a process's files are opened relative
/// to it (`openat`) and read in one go into one reused buffer, and the
/// owner comes from one `fstatat`. No paths are allocated per file.
fn scan_direct(root: &SysRoot, buf: &mut Vec<u8>) -> Result<Scan, CollectError> {
    use rustix::fs::{statat, AtFlags, Dir, Mode, OFlags};

    let dir = root.proc_path("");
    let io_error = |source: rustix::io::Errno| CollectError::Io {
        path: dir.clone(),
        source: source.into(),
    };
    let proc_fd = rustix::fs::open(
        &dir,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io_error)?;
    let mut path = String::with_capacity(32);
    let mut processes = Vec::with_capacity(512);
    for entry in Dir::read_from(&proc_fd).map_err(io_error)?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .ok()
            .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue; // not a process: "self", "stat", "."
        };
        let mut file = |name: &str, buf: &mut Vec<u8>| {
            path.clear();
            path.push_str(pid);
            path.push('/');
            path.push_str(name);
            read_at(&proc_fd, &path, buf)
        };
        // A process can exit between listing and reading: skip it quietly.
        // A name need not be UTF-8: read bytes, decode lossily.
        if file("stat", buf).is_err() {
            continue;
        }
        let Some(stat) = parse_proc_stat(String::from_utf8_lossy(buf).trim_end()) else {
            continue;
        };
        if file("statm", buf).is_err() {
            continue;
        }
        let Some(private_pages) = std::str::from_utf8(buf).ok().and_then(parse_statm) else {
            continue;
        };
        // The owner of /proc/<pid> is the process's effective uid; reading
        // `status` would cost one more file per process.
        let uid = statat(&proc_fd, pid, AtFlags::empty())
            .ok()
            .map(|st| st.st_uid)
            .or_else(|| {
                file("status", buf)
                    .ok()
                    .and_then(|()| parse_status_uid(&String::from_utf8_lossy(buf)))
            });
        processes.push(RawProcess {
            stat,
            uid,
            private_pages,
        });
    }
    let cpu_line = root
        .read_proc("stat")
        .ok()
        .and_then(|text| text.lines().next().map(str::to_owned));
    Ok(Scan {
        cpu_line,
        processes,
    })
}

/// Read the file at `path`, relative to the directory `dir`, into `buf`.
///
/// Files in /proc are made whole on the first read, so a read that leaves
/// room in the buffer has everything; only a full buffer asks for more.
fn read_at(dir: &impl std::os::fd::AsFd, path: &str, buf: &mut Vec<u8>) -> rustix::io::Result<()> {
    use rustix::fs::{openat, Mode, OFlags};
    let file = openat(dir, path, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?;
    buf.clear();
    loop {
        if buf.capacity() - buf.len() < 1024 {
            buf.reserve(4096);
        }
        let room = buf.capacity() - buf.len();
        // Appends what was read to `buf`.
        let n = rustix::io::read(&file, rustix::buffer::spare_capacity(buf))?;
        if n == 0 || n < room {
            return Ok(());
        }
    }
}

fn socket_owners_direct(root: &SysRoot) -> Owners {
    let mut owners = Owners::default();
    let Ok(entries) = std::fs::read_dir(root.proc_path("")) else {
        return owners;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse().ok()) else {
            continue;
        };
        // Other users' descriptors cannot be read without root.
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let inode = target
                .to_str()
                .and_then(|t| t.strip_prefix("socket:["))
                .and_then(|t| t.strip_suffix(']'))
                .and_then(|t| t.parse().ok());
            if let Some(inode) = inode {
                owners.sockets.entry(inode).or_insert(pid);
            }
        }
    }
    owners
}

/// Run `f` on the host shell, starting it if needed. A failed request drops
/// the shell, so the next tick starts a fresh one.
fn with_shell<T>(
    shell: &mut Option<HostShell>,
    f: impl FnOnce(&mut HostShell) -> io::Result<T>,
) -> io::Result<T> {
    if shell.is_none() {
        *shell = Some(HostShell::spawn()?);
    }
    let Some(running) = shell.as_mut() else {
        return Err(io::Error::other("host shell did not start"));
    };
    let result = f(running);
    if result.is_err() {
        *shell = None;
    }
    result
}

pub(super) struct HostShell {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl HostShell {
    fn spawn() -> io::Result<Self> {
        let mut child = Command::new("flatpak-spawn")
            .args(["--host", "sh", "-c", HOST_SCRIPT])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(io::Error::other("no pipes to the host shell"));
        };
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::with_capacity(64 * 1024, stdout),
        })
    }

    /// Send one command and collect its lines up to the closing `E`.
    fn request(&mut self, cmd: &str) -> io::Result<Vec<String>> {
        writeln!(self.stdin, "{cmd}")?;
        self.stdin.flush()?;
        let mut lines = Vec::with_capacity(2048);
        let mut line = String::new();
        loop {
            line.clear();
            if self.stdout.read_line(&mut line)? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the host shell exited",
                ));
            }
            let text = line.trim_end_matches('\n');
            if text == "E" {
                return Ok(lines);
            }
            lines.push(text.to_owned());
        }
    }
}

impl Drop for HostShell {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The uid this program runs as; inside Flatpak it is the same as on the host.
pub fn current_uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// Ask a process to stop (`force == false`, SIGTERM) or stop it at once
/// (SIGKILL).
///
/// The process is named by its pid and its start time (`start_ticks` of
/// [`ProcStat`](super::ProcStat)): when the process has ended and its pid
/// now belongs to another one, nothing is sent and the result is
/// [`io::ErrorKind::NotFound`]. Fails with [`io::ErrorKind::PermissionDenied`]
/// for processes of other users.
pub fn send_signal(
    source: ProcessSource,
    pid: u32,
    start_ticks: u64,
    force: bool,
) -> io::Result<()> {
    match source {
        ProcessSource::Direct => signal_direct(pid, start_ticks, force),
        ProcessSource::FlatpakHost => signal_host(pid, start_ticks, force),
    }
}

fn signal_direct(pid: u32, start_ticks: u64, force: bool) -> io::Result<()> {
    use rustix::process::{self, Pid, PidfdFlags, Signal};

    let raw = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let signal = if force { Signal::KILL } else { Signal::TERM };
    // A pidfd holds on to this very process: once its start time is checked,
    // the signal cannot reach a newer process that took over the pid.
    let pidfd = match process::pidfd_open(raw, PidfdFlags::empty()) {
        Ok(fd) => Some(fd),
        Err(rustix::io::Errno::NOSYS) => None, // Linux before 5.3
        Err(e) => return Err(e.into()),
    };
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|_| io::Error::from(io::ErrorKind::NotFound))?;
    let same =
        super::parse_proc_stat(stat.trim_end()).is_some_and(|s| s.start_ticks == start_ticks);
    if !same {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    }
    match pidfd {
        Some(fd) => process::pidfd_send_signal(fd, signal),
        None => process::kill_process(raw, signal),
    }
    .map_err(io::Error::from)
}

/// On the host the start time is checked right before `kill`, by the same
/// shell: field 22 of `/proc/<pid>/stat`. After the name is cut off, field 3
/// comes right after the three arguments, so field 22 is `${23}`.
const HOST_KILL: &str = r#"
s=$(sed 's/.*) //' "/proc/$1/stat" 2>/dev/null) || exit 3
set -- "$1" "$2" "$3" $s
[ "${23}" = "$2" ] || exit 3
exec kill -s "$3" "$1"
"#;

fn signal_host(pid: u32, start_ticks: u64, force: bool) -> io::Result<()> {
    let output = Command::new("flatpak-spawn")
        .args(["--host", "sh", "-c", HOST_KILL, "sh"])
        .args([
            pid.to_string(),
            start_ticks.to_string(),
            (if force { "KILL" } else { "TERM" }).to_owned(),
        ])
        .stdin(Stdio::null())
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr);
    Err(
        if output.status.code() == Some(3) || message.contains("No such process") {
            io::Error::from(io::ErrorKind::NotFound)
        } else if message.contains("not permitted") {
            io::Error::from(io::ErrorKind::PermissionDenied)
        } else {
            io::Error::other(message.trim().to_owned())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A child that sleeps, and its start time.
    fn sleeper() -> (std::process::Child, u32, u64) {
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let start = super::super::parse_proc_stat(stat.trim_end())
            .unwrap()
            .start_ticks;
        (child, pid, start)
    }

    #[test]
    fn a_signal_reaches_only_the_process_that_was_meant() {
        let (mut child, pid, start) = sleeper();
        // Another start time: the pid now names another process.
        let err = send_signal(ProcessSource::Direct, pid, start + 1, false).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(child.try_wait().unwrap().is_none(), "still running");

        send_signal(ProcessSource::Direct, pid, start, false).unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());
    }

    #[test]
    fn the_host_script_checks_the_start_time() {
        // The same script the Flatpak build runs on the host, run here.
        let run = |pid: u32, start: u64| {
            Command::new("sh")
                .args(["-c", HOST_KILL, "sh"])
                .args([pid.to_string(), start.to_string(), "TERM".to_owned()])
                .status()
                .unwrap()
        };
        let (mut child, pid, start) = sleeper();
        assert_eq!(run(pid, start + 1).code(), Some(3));
        assert!(child.try_wait().unwrap().is_none(), "still running");
        assert!(run(pid, start).success());
        assert!(!child.wait().unwrap().success());
    }
}
