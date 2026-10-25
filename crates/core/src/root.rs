//! Where `/proc` and `/sys` are read from.
//!
//! Every read goes through [`SysRoot`], so tests, Flatpak and a fake `/proc`
//! work without touching collector code.

use crate::CollectError;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Name of the environment variable that points Corewatch at a fake root.
pub const SYSROOT_ENV: &str = "COREWATCH_SYSROOT";

#[derive(Clone, Debug)]
pub struct SysRoot {
    proc: PathBuf,
    sys: PathBuf,
}

impl SysRoot {
    /// The real `/proc` and `/sys` of this machine.
    pub fn host() -> Self {
        Self {
            proc: "/proc".into(),
            sys: "/sys".into(),
        }
    }

    /// `<base>/proc` and `<base>/sys`; used by tests and [`SYSROOT_ENV`].
    pub fn at(base: impl AsRef<Path>) -> Self {
        let base = base.as_ref();
        Self {
            proc: base.join("proc"),
            sys: base.join("sys"),
        }
    }

    /// Separate proc and sys roots; lets tests pair one `/sys` with several
    /// `/proc` states.
    pub fn new(proc: impl Into<PathBuf>, sys: impl Into<PathBuf>) -> Self {
        Self {
            proc: proc.into(),
            sys: sys.into(),
        }
    }

    /// [`SysRoot::at`] the value of [`SYSROOT_ENV`] if set, otherwise [`SysRoot::host`].
    pub fn from_env() -> Self {
        std::env::var_os(SYSROOT_ENV)
            .map(Self::at)
            .unwrap_or_else(Self::host)
    }

    /// Whether this is the real `/proc` and `/sys`, not a test tree. Data that
    /// does not come from a file (IP addresses, processes on the Flatpak
    /// host) is read only for the real machine.
    pub fn is_host(&self) -> bool {
        self.proc == Path::new("/proc") && self.sys == Path::new("/sys")
    }

    /// Read a file relative to the proc root, e.g. `"stat"`.
    pub fn read_proc(&self, rel: &str) -> Result<String, CollectError> {
        read(self.proc.join(rel))
    }

    /// Read a file relative to the sys root, e.g. `"devices/system/cpu/online"`.
    pub fn read_sys(&self, rel: &str) -> Result<String, CollectError> {
        read(self.sys.join(rel))
    }

    /// Read a file relative to the proc root into `buf`, replacing its
    /// contents. Lets a collector that reads hundreds of small files per tick
    /// (one per process) reuse one buffer.
    pub fn read_proc_into(&self, rel: &str, buf: &mut String) -> Result<(), CollectError> {
        buf.clear();
        let path = self.proc.join(rel);
        File::open(&path)
            .and_then(|mut file| file.read_to_string(buf))
            .map(drop)
            .map_err(|source| CollectError::Io { path, source })
    }

    /// Full path of an entry under the proc root, for listing directories.
    pub fn proc_path(&self, rel: &str) -> PathBuf {
        self.proc.join(rel)
    }

    /// Full path of an entry under the sys root, for existence checks.
    pub fn sys_path(&self, rel: &str) -> PathBuf {
        self.sys.join(rel)
    }
}

fn read(path: PathBuf) -> Result<String, CollectError> {
    let mut file = File::open(&path).map_err(|source| CollectError::Io {
        path: path.clone(),
        source,
    })?;
    // Files in /proc and /sys report st_size == 0, causing std::fs::read_to_string
    // to repeatedly reallocate buffer memory. Pre-allocating a 2KB buffer accommodates
    // typical procfs entries in a single step without reallocations.
    let mut buf = String::with_capacity(2048);
    file.read_to_string(&mut buf)
        .map_err(|source| CollectError::Io { path, source })?;
    Ok(buf)
}
