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
      cg=
      read -r cg < "$d/cgroup" 2>/dev/null
      printf 'P %s\n%s\n%s\nC %s\n' "$u" "$st" "$sm" "$cg"
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

    /// A process's cgroup file, for its group (see `cgroup.rs`). Inside
    /// Flatpak the scan itself brings it.
    pub fn cgroup(&self, root: &SysRoot, pid: u32) -> Option<String> {
        match self {
            Self::Direct { .. } => root.read_proc(&format!("{pid}/cgroup")).ok(),
            Self::Host(_) => None,
        }
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
            cgroup: None,
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

/// What [`send_signal`] asks of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSignal {
    /// SIGTERM: ask it to quit, letting it save and clean up.
    End,
    /// SIGKILL: stop it at once.
    ForceStop,
    /// SIGSTOP: freeze it where it is.
    Suspend,
    /// SIGCONT: let a suspended process run again.
    Resume,
}

impl ProcessSignal {
    fn raw(self) -> rustix::process::Signal {
        use rustix::process::Signal;
        match self {
            Self::End => Signal::TERM,
            Self::ForceStop => Signal::KILL,
            Self::Suspend => Signal::STOP,
            Self::Resume => Signal::CONT,
        }
    }

    /// The name `kill -s` takes.
    fn name(self) -> &'static str {
        match self {
            Self::End => "TERM",
            Self::ForceStop => "KILL",
            Self::Suspend => "STOP",
            Self::Resume => "CONT",
        }
    }
}

/// Send `signal` to a process.
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
    signal: ProcessSignal,
) -> io::Result<()> {
    match source {
        ProcessSource::Direct => signal_direct(pid, start_ticks, signal),
        ProcessSource::FlatpakHost => host_command(HOST_KILL, pid, start_ticks, signal.name()),
    }
}

/// Set a process's niceness, -20 (runs first) to 19 (runs last). Without
/// root a process can only be made nicer than it is.
///
/// On Linux a niceness belongs to one thread, and `setpriority` (like
/// `renice -p`) changes only the thread whose id is the pid. A browser or a
/// game does most of its work in other threads, so every thread of the
/// process is changed.
pub fn set_priority(
    source: ProcessSource,
    pid: u32,
    start_ticks: u64,
    nice: i32,
) -> io::Result<()> {
    match source {
        ProcessSource::Direct => {
            let raw = raw_pid(pid)?;
            check_start(pid, start_ticks)?;
            // The main thread first: its error (not allowed, gone) is the
            // answer. Threads that end meanwhile are skipped.
            rustix::process::setpriority_process(Some(raw), nice).map_err(io::Error::from)?;
            for tid in threads(pid).into_iter().filter(|t| *t != pid) {
                if let Ok(tid) = raw_pid(tid) {
                    let _ = rustix::process::setpriority_process(Some(tid), nice);
                }
            }
            Ok(())
        }
        ProcessSource::FlatpakHost => {
            host_command(HOST_RENICE, pid, start_ticks, &nice.to_string())
        }
    }
}

/// The thread ids of a process; empty when they cannot be listed.
fn threads(pid: u32) -> Vec<u32> {
    std::fs::read_dir(format!("/proc/{pid}/task"))
        .map(|tasks| {
            tasks
                .flatten()
                .filter_map(|t| t.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn raw_pid(pid: u32) -> io::Result<rustix::process::Pid> {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))
}

/// `NotFound` unless `pid` is still the process that started at `start_ticks`.
fn check_start(pid: u32, start_ticks: u64) -> io::Result<()> {
    let stat = std::fs::read(format!("/proc/{pid}/stat"))
        .map_err(|_| io::Error::from(io::ErrorKind::NotFound))?;
    let same = super::parse_proc_stat(String::from_utf8_lossy(&stat).trim_end())
        .is_some_and(|s| s.start_ticks == start_ticks);
    if same {
        Ok(())
    } else {
        Err(io::Error::from(io::ErrorKind::NotFound))
    }
}

fn signal_direct(pid: u32, start_ticks: u64, signal: ProcessSignal) -> io::Result<()> {
    use rustix::process::{self, PidfdFlags};

    let raw = raw_pid(pid)?;
    // A pidfd holds on to this very process: once its start time is checked,
    // the signal cannot reach a newer process that took over the pid.
    let pidfd = match process::pidfd_open(raw, PidfdFlags::empty()) {
        Ok(fd) => Some(fd),
        Err(rustix::io::Errno::NOSYS) => None, // Linux before 5.3
        Err(e) => return Err(e.into()),
    };
    check_start(pid, start_ticks)?;
    match pidfd {
        Some(fd) => process::pidfd_send_signal(fd, signal.raw()),
        None => process::kill_process(raw, signal.raw()),
    }
    .map_err(io::Error::from)
}

/// On the host the start time is checked right before acting, by the same
/// shell: field 22 of `/proc/<pid>/stat`. After the name is cut off, field 3
/// comes right after the three arguments, so field 22 is `${23}`.
const HOST_CHECK: &str = r#"
s=$(sed 's/.*) //' "/proc/$1/stat" 2>/dev/null) || exit 3
set -- "$1" "$2" "$3" $s
[ "${23}" = "$2" ] || exit 3
"#;

const HOST_KILL: &str = r#"exec kill -s "$3" "$1""#;
/// Every thread, main thread first (see [`set_priority`]); a thread that
/// ends meanwhile is not an error.
const HOST_RENICE: &str = r#"renice -n "$3" -p "$1" >/dev/null || exit
for t in /proc/$1/task/*; do
  t=${t##*/}; [ "$t" = "$1" ] || renice -n "$3" -p "$t" >/dev/null 2>&1
done
exit 0"#;

/// Run `action` on the host for a process, after checking its start time.
fn host_command(action: &str, pid: u32, start_ticks: u64, arg: &str) -> io::Result<()> {
    let script = format!("{HOST_CHECK}{action}\n");
    let output = Command::new("flatpak-spawn")
        .args(["--host", "sh", "-c", &script, "sh"])
        .args([pid.to_string(), start_ticks.to_string(), arg.to_owned()])
        .stdin(Stdio::null())
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr);
    Err(
        if output.status.code() == Some(3) || message.contains("No such process") {
            io::Error::from(io::ErrorKind::NotFound)
        } else if message.contains("not permitted") || message.contains("ermission denied") {
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

    /// Not a test on its own: the child process of
    /// `a_lower_priority_reaches_every_thread`, which runs this test binary
    /// again with `COREWATCH_THREADS_CHILD` set. It starts a second thread
    /// and waits, so the parent has a process with several threads to
    /// change; the niceness of the test process itself is never touched (a
    /// test started later would inherit it).
    #[test]
    #[ignore]
    fn threads_child() {
        if std::env::var_os("COREWATCH_THREADS_CHILD").is_none() {
            return;
        }
        let worker = std::thread::spawn(|| std::thread::sleep(std::time::Duration::from_secs(30)));
        let _ = worker.join();
    }

    #[test]
    fn a_lower_priority_reaches_every_thread() {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "process::source::tests::threads_child",
                "--ignored",
            ])
            .env("COREWATCH_THREADS_CHILD", "1")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        // Wait until the child runs its extra thread: at least three tasks
        // (main, the test's thread, the worker).
        let tasks = || threads(pid);
        for _ in 0..200 {
            if tasks().len() >= 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let start = super::super::parse_proc_stat(stat.trim_end())
            .unwrap()
            .start_ticks;
        // The lowest priority: allowed without root or CAP_SYS_NICE.
        set_priority(ProcessSource::Direct, pid, start, 19).unwrap();
        let nice_of = |tid: u32| {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/task/{tid}/stat")).unwrap();
            super::super::parse_proc_stat(stat.trim_end()).unwrap().nice
        };
        let all = tasks();
        let nices: Vec<i32> = all.iter().map(|tid| nice_of(*tid)).collect();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(all.len() >= 3, "the child has several threads: {all:?}");
        assert!(nices.iter().all(|n| *n == 19), "every thread: {nices:?}");
    }

    #[test]
    fn a_signal_reaches_only_the_process_that_was_meant() {
        let (mut child, pid, start) = sleeper();
        // Another start time: the pid now names another process.
        let err =
            send_signal(ProcessSource::Direct, pid, start + 1, ProcessSignal::End).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(child.try_wait().unwrap().is_none(), "still running");

        send_signal(ProcessSource::Direct, pid, start, ProcessSignal::End).unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());
    }

    #[test]
    fn a_process_can_be_suspended_resumed_and_made_nicer() {
        let (mut child, pid, start) = sleeper();
        let state = || {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
            super::super::parse_proc_stat(stat.trim_end()).unwrap()
        };
        send_signal(ProcessSource::Direct, pid, start, ProcessSignal::Suspend).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(state().state, 'T');
        send_signal(ProcessSource::Direct, pid, start, ProcessSignal::Resume).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(state().state, 'S');

        set_priority(ProcessSource::Direct, pid, start, 10).unwrap();
        assert_eq!(state().nice, 10);
        let gone = set_priority(ProcessSource::Direct, pid, start + 1, 15).unwrap_err();
        assert_eq!(gone.kind(), io::ErrorKind::NotFound);

        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn the_host_script_checks_the_start_time() {
        // The same script the Flatpak build runs on the host, run here.
        let run = |pid: u32, start: u64| {
            Command::new("sh")
                .args(["-c", &format!("{HOST_CHECK}{HOST_KILL}\n"), "sh"])
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
