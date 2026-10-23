//! Backup file names: `notebook-backup-YYYYMMDD-HHMMSS.nbbackup`, with a
//! ` 2`, ` 3`, … suffix when the name is taken.

use crate::note::Timestamp;
use crate::time::stamp;

const PREFIX: &str = "notebook-backup-";
/// The extension of every backup file, without the dot.
pub const BACKUP_EXTENSION: &str = "nbbackup";

/// The name of the `copy`th backup created at `created_at` (UTC): copy 1
/// has no suffix, later ones get ` 2`, ` 3`, and so on.
pub fn file_name(created_at: Timestamp, copy: u32) -> String {
    let stamp = stamp(created_at);
    if copy <= 1 {
        format!("{PREFIX}{stamp}.{BACKUP_EXTENSION}")
    } else {
        format!("{PREFIX}{stamp} {copy}.{BACKUP_EXTENSION}")
    }
}

/// A file name that matches the backup pattern
/// `^notebook-backup-\d{8}-\d{6}( \d+)?\.nbbackup$`. Ordering is by time
/// stamp, then suffix, so oldest sorts first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BackupFileName<'a> {
    /// `YYYYMMDD-HHMMSS`; fixed width, so it orders as text.
    pub stamp: &'a str,
    /// 1 when there is no suffix.
    pub copy: u32,
}

impl<'a> BackupFileName<'a> {
    /// Matches `name` against the backup pattern. Renamed backups, conflicted
    /// copies, and suffixes too large for a `u32` don't match.
    pub fn parse(name: &'a str) -> Option<Self> {
        let rest = name.strip_prefix(PREFIX)?;
        let rest = rest.strip_suffix(BACKUP_EXTENSION)?.strip_suffix('.')?;
        let (stamp, suffix) = rest.split_at_checked(15)?;
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let (date, time) = (stamp.get(..8)?, stamp.get(9..)?);
        if !digits(date) || stamp.as_bytes()[8] != b'-' || !digits(time) {
            return None;
        }
        let copy = match suffix.strip_prefix(' ') {
            None if suffix.is_empty() => 1,
            Some(n) if digits(n) => n.parse().ok()?,
            _ => return None,
        };
        Some(Self { stamp, copy })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_from_time() {
        // 2026-10-22T14:03:09Z.
        let t = Timestamp(1_792_677_789_000);
        assert_eq!(file_name(t, 1), "notebook-backup-20261022-140309.nbbackup");
        assert_eq!(
            file_name(t, 2),
            "notebook-backup-20261022-140309 2.nbbackup"
        );
        assert_eq!(
            file_name(t, 13),
            "notebook-backup-20261022-140309 13.nbbackup"
        );
    }

    #[test]
    fn written_names_parse_back() {
        let t = Timestamp(1_792_677_789_000);
        for copy in [1, 2, 10, u32::MAX] {
            let name = file_name(t, copy);
            assert_eq!(
                BackupFileName::parse(&name),
                Some(BackupFileName {
                    stamp: "20261022-140309",
                    copy
                }),
                "{name}"
            );
        }
    }

    #[test]
    fn other_names_do_not_match() {
        for name in [
            "",
            "notebook-backup-.nbbackup",
            "notebook-backup-20261022-140309",
            "notebook-backup-20261022-140309.nbbackup.partial",
            ".notebook-backup-20261022-140309.nbbackup",
            "notebook-backup-20261022-14030.nbbackup",
            "notebook-backup-20261022-1403099.nbbackup",
            "notebook-backup-2026102a-140309.nbbackup",
            "notebook-backup-20261022_140309.nbbackup",
            "notebook-backup-20261022-140309 .nbbackup",
            "notebook-backup-20261022-140309  2.nbbackup",
            "notebook-backup-20261022-140309 2a.nbbackup",
            "notebook-backup-20261022-140309 -2.nbbackup",
            "notebook-backup-20261022-140309 99999999999.nbbackup",
            "notebook-backup-20261022-140309.NBBACKUP",
            "notebook-backup-20261022-140309.txt",
            // Dropbox conflicted copy.
            "notebook-backup-20261022-140309 (Laptop's conflicted copy 2026-10-23).nbbackup",
            // Renamed by the user.
            "my notebook-backup-20261022-140309.nbbackup",
            "notebook-backup-20261022-140309 (1).nbbackup",
            "notes.md",
            // Multi-byte text where the stamp would be.
            "notebook-backup-２０２６1022-140309.nbbackup",
        ] {
            assert_eq!(BackupFileName::parse(name), None, "{name:?}");
        }
    }

    #[test]
    fn orders_by_stamp_then_copy() {
        let mut names = [
            "notebook-backup-20261022-140309 10.nbbackup",
            "notebook-backup-20261023-000000.nbbackup",
            "notebook-backup-20261022-140309 2.nbbackup",
            "notebook-backup-20251231-235959.nbbackup",
            "notebook-backup-20261022-140309.nbbackup",
        ]
        .map(|n| BackupFileName::parse(n).unwrap());
        names.sort();
        let order: Vec<_> = names.iter().map(|n| (n.stamp, n.copy)).collect();
        assert_eq!(
            order,
            [
                ("20251231-235959", 1),
                ("20261022-140309", 1),
                ("20261022-140309", 2),
                ("20261022-140309", 10),
                ("20261023-000000", 1),
            ]
        );
    }
}
