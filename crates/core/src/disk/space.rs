//! Free and used space of a mounted filesystem.
//!
//! `statvfs` reads the real filesystem and cannot be pointed at a fake root,
//! so it sits behind [`SpaceProbe`] and tests swap in their own numbers.

use std::path::Path;

/// Space on one filesystem, in bytes, counted like `df`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Space {
    pub total: u64,
    pub used: u64,
    /// Free space an unprivileged user can still write; smaller than
    /// total − used because of blocks reserved for root.
    pub available: u64,
}

impl Space {
    /// Build from `statvfs` block counts.
    pub fn from_blocks(blocks: u64, free: u64, available: u64, block_size: u64) -> Self {
        Self {
            total: blocks.saturating_mul(block_size),
            used: blocks.saturating_sub(free).saturating_mul(block_size),
            available: available.saturating_mul(block_size),
        }
    }

    /// Used share as `df` shows it: used / (used + available), 0.0..=1.0.
    pub fn used_fraction(&self) -> f32 {
        let whole = self.used.saturating_add(self.available);
        if whole == 0 {
            0.0
        } else {
            (self.used as f64 / whole as f64).clamp(0.0, 1.0) as f32
        }
    }
}

/// Where the space numbers come from.
pub trait SpaceProbe: Send {
    /// `None` when the mount point cannot be asked (gone, no permission).
    fn space(&self, mount_point: &Path) -> Option<Space>;
}

/// The real probe: `statvfs(2)`. Only ever called on local, disk-backed
/// filesystems, so it does not block on a dead network server.
#[derive(Debug, Default, Clone, Copy)]
pub struct Statvfs;

impl SpaceProbe for Statvfs {
    fn space(&self, mount_point: &Path) -> Option<Space> {
        match rustix::fs::statvfs(mount_point) {
            Ok(s) => Some(Space::from_blocks(
                s.f_blocks, s.f_bfree, s.f_bavail, s.f_frsize,
            )),
            Err(error) => {
                tracing::debug!(mount_point = %mount_point.display(), %error, "statvfs failed");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_like_df() {
        // 1000 blocks of 4 KiB, 400 free, 350 of those usable by normal users.
        let s = Space::from_blocks(1000, 400, 350, 4096);
        assert_eq!(s.total, 4_096_000);
        assert_eq!(s.used, 600 * 4096);
        assert_eq!(s.available, 350 * 4096);
        assert!((s.used_fraction() - 600.0 / 950.0).abs() < 1e-6);
    }

    #[test]
    fn empty_filesystem_has_zero_share() {
        assert_eq!(Space::default().used_fraction(), 0.0);
    }

    #[test]
    fn root_directory_can_be_asked() {
        let s = Statvfs.space(Path::new("/")).expect("statvfs on /");
        assert!(s.total > 0);
    }
}
