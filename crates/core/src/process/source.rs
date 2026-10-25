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
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
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
    Direct { buf: String },
    Host(Option<HostShell>),
}

impl Reader {
    pub fn new(source: ProcessSource) -> Self {
        match source {
            ProcessSource::Direct => Self::Direct {
                buf: String::with_capacity(1024),
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

fn scan_direct(root: &SysRoot, buf: &mut String) -> Result<Scan, CollectError> {
    let dir = root.proc_path("");
    let entries = std::fs::read_dir(&dir).map_err(|source| CollectError::Io {
        path: dir.clone(),
        source,
    })?;
    let mut processes = Vec::with_capacity(512);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        // A process can exit between listing and reading: skip it quietly.
        if root.read_proc_into(&format!("{pid}/stat"), buf).is_err() {
            continue;
        }
        let Some(stat) = parse_proc_stat(buf.trim_end()) else {
            continue;
        };
        if root.read_proc_into(&format!("{pid}/statm"), buf).is_err() {
            continue;
        }
        let Some(private_pages) = parse_statm(buf) else {
            continue;
        };
        // The owner of /proc/<pid> is the process's effective uid; reading
        // `status` would cost one more file per process.
        let uid = entry.metadata().ok().map(|m| m.uid()).or_else(|| {
            root.read_proc_into(&format!("{pid}/status"), buf)
                .ok()
                .and_then(|()| parse_status_uid(buf))
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
        owners.readable.insert(pid);
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
/// (SIGKILL). Fails with [`io::ErrorKind::PermissionDenied`] for processes
/// of other users.
pub fn send_signal(source: ProcessSource, pid: u32, force: bool) -> io::Result<()> {
    match source {
        ProcessSource::Direct => {
            let pid = i32::try_from(pid)
                .ok()
                .and_then(rustix::process::Pid::from_raw)
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            let signal = if force {
                rustix::process::Signal::KILL
            } else {
                rustix::process::Signal::TERM
            };
            rustix::process::kill_process(pid, signal).map_err(io::Error::from)
        }
        ProcessSource::FlatpakHost => {
            let output = Command::new("flatpak-spawn")
                .args(["--host", "kill", "-s", if force { "KILL" } else { "TERM" }])
                .arg(pid.to_string())
                .stdin(Stdio::null())
                .output()?;
            if output.status.success() {
                return Ok(());
            }
            let message = String::from_utf8_lossy(&output.stderr);
            Err(if message.contains("not permitted") {
                io::Error::from(io::ErrorKind::PermissionDenied)
            } else if message.contains("No such process") {
                io::Error::from(io::ErrorKind::NotFound)
            } else {
                io::Error::other(message.trim().to_owned())
            })
        }
    }
}
