//! `/proc/self/mountinfo` parsing and the choice of filesystems to show.
//!
//! Line layout (see proc(5)):
//! `id parent major:minor root mount-point options [optional...] - type source super-options`

use crate::CollectError;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    /// Directory inside the filesystem that is mounted (`/` for the whole thing).
    pub root: String,
    pub mount_point: PathBuf,
    pub fs_type: String,
    pub source: String,
}

/// Parse every well-formed line; malformed lines are skipped. A non-empty file
/// where no line is usable is an error.
pub fn parse_mountinfo(text: &str) -> Result<Vec<MountEntry>, CollectError> {
    let mut entries = Vec::new();
    let mut lines = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        lines += 1;
        if let Some(entry) = parse_line(line) {
            entries.push(entry);
        }
    }
    if lines > 0 && entries.is_empty() {
        return Err(CollectError::parse(
            "/proc/self/mountinfo",
            format!("none of {lines} lines could be read"),
        ));
    }
    Ok(entries)
}

fn parse_line(line: &str) -> Option<MountEntry> {
    let fields: Vec<&str> = line.split(' ').collect();
    // The optional fields end at a lone "-"; there may be none of them.
    let sep = fields.iter().skip(6).position(|f| *f == "-")? + 6;
    if fields.len() < sep + 3 {
        return None;
    }
    Some(MountEntry {
        root: unescape(fields[3]),
        mount_point: PathBuf::from(unescape(fields[4])),
        fs_type: fields[sep + 1].to_owned(),
        source: unescape(fields[sep + 2]),
    })
}

/// The kernel writes space, tab, newline and backslash as `\040`, `\011`,
/// `\012` and `\134`.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if let (b'\\', Some(digits)) = (bytes[i], bytes.get(i + 1..i + 4)) {
            if let Some(value) = octal_byte(digits) {
                out.push(value);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Three octal digits (`"040"`) to the byte they name, if they fit in a byte.
fn octal_byte(digits: &[u8]) -> Option<u8> {
    digits.iter().try_fold(0u8, |acc, d| match d {
        b'0'..=b'7' => acc.checked_mul(8)?.checked_add(d - b'0'),
        _ => None,
    })
}

/// Local, disk-backed filesystems worth showing, one per source, sorted by
/// mount point.
///
/// Keeps sources under `/dev/` (so network filesystems, which could make
/// `statvfs` hang, are never touched) and drops `squashfs` (snap packages).
/// A source mounted several times (bind mounts, btrfs subvolumes) is kept once:
/// whole-filesystem mounts first, then the shortest mount point.
pub fn local_filesystems(entries: &[MountEntry]) -> Vec<MountEntry> {
    let mut best: HashMap<&str, &MountEntry> = HashMap::new();
    for entry in entries
        .iter()
        .filter(|e| e.source.starts_with("/dev/") && e.fs_type != "squashfs")
    {
        let key = |e: &MountEntry| (e.root != "/", e.mount_point.as_os_str().len());
        best.entry(entry.source.as_str())
            .and_modify(|current| {
                if key(entry) < key(current) {
                    *current = entry;
                }
            })
            .or_insert(entry);
    }
    let mut chosen: Vec<MountEntry> = best.into_values().cloned().collect();
    chosen.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
22 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
23 22 259:1 / /boot/efi rw,relatime shared:2 - vfat /dev/nvme0n1p1 rw,fmask=0077
24 22 0:5 / /proc rw,nosuid - proc proc rw
25 22 8:1 / /mnt/My\\040Data rw,relatime - ext4 /dev/sda1 rw
26 22 7:0 / /snap/core/1 ro,relatime shared:9 - squashfs /dev/loop0 ro
27 22 259:2 /home /srv/home rw,relatime - ext4 /dev/nvme0n1p2 rw
28 22 0:40 / /net rw - nfs4 server:/export rw
";

    #[test]
    fn parses_fields_with_and_without_optional_tags() {
        let entries = parse_mountinfo(SAMPLE).unwrap();
        assert_eq!(entries.len(), 7);
        assert_eq!(entries[0].mount_point, PathBuf::from("/"));
        assert_eq!(entries[0].fs_type, "ext4");
        assert_eq!(entries[0].source, "/dev/nvme0n1p2");
        assert_eq!(entries[2].source, "proc");
        assert_eq!(entries[5].root, "/home");
    }

    #[test]
    fn decodes_escaped_spaces() {
        let entries = parse_mountinfo(SAMPLE).unwrap();
        assert_eq!(entries[3].mount_point, PathBuf::from("/mnt/My Data"));
        assert_eq!(unescape("a\\011b\\134c"), "a\tb\\c");
        assert_eq!(unescape("trailing\\04"), "trailing\\04");
    }

    #[test]
    fn keeps_local_disks_once_and_drops_the_rest() {
        let chosen = local_filesystems(&parse_mountinfo(SAMPLE).unwrap());
        let points: Vec<_> = chosen
            .iter()
            .map(|e| e.mount_point.to_string_lossy().into_owned())
            .collect();
        // No /proc, no squashfs, no NFS, and the /srv/home bind mount folds into /.
        assert_eq!(points, ["/", "/boot/efi", "/mnt/My Data"]);
    }

    #[test]
    fn btrfs_subvolumes_fold_into_the_shortest_mount_point() {
        let text = "\
30 1 0:31 /@ / rw - btrfs /dev/sda2 rw
31 30 0:32 /@home /home rw - btrfs /dev/sda2 rw
";
        let chosen = local_filesystems(&parse_mountinfo(text).unwrap());
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].mount_point, PathBuf::from("/"));
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(parse_mountinfo("not a mount line\n").is_err());
        assert!(parse_mountinfo("").unwrap().is_empty());
    }
}
