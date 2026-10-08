//! Where `/proc` and `/sys` are read from.
//!
//! Every read goes through [`SysRoot`], so tests, Flatpak and a fake `/proc`
//! work without touching collector code.

use crate::CollectError;
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

    /// [`SysRoot::at`] the value of [`SYSROOT_ENV`] if set, otherwise [`SysRoot::host`].
    pub fn from_env() -> Self {
        std::env::var_os(SYSROOT_ENV)
            .map(Self::at)
            .unwrap_or_else(Self::host)
    }

    /// Read a file relative to the proc root, e.g. `"stat"`.
    pub fn read_proc(&self, rel: &str) -> Result<String, CollectError> {
        read(self.proc.join(rel))
    }

    /// Read a file relative to the sys root, e.g. `"devices/system/cpu/online"`.
    pub fn read_sys(&self, rel: &str) -> Result<String, CollectError> {
        read(self.sys.join(rel))
    }
}

fn read(path: PathBuf) -> Result<String, CollectError> {
    std::fs::read_to_string(&path).map_err(|source| CollectError::Io { path, source })
}
