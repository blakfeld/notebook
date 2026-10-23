//! Writing backups into a folder and pruning old ones. Native only.
//!
//! A backup is written to a hidden `.notebook-backup-<random>.partial` file
//! in the backup folder, flushed to disk, and then hard-linked to a free
//! [`file_name`] (renamed where hard links aren't supported). Both are
//! atomic within one directory, so anything under a backup name is
//! complete, and the link never replaces an existing file. The folder
//! must already exist: it is never created, so an unmounted cloud drive
//! can't make a backup land in a stray local directory.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use super::{BackupFileName, MAX_HEADER_LEN, file_name, read_header};
use crate::crypto::{VaultId, fill_random};
use crate::note::Timestamp;
use crate::time::stamp;

const PARTIAL_PREFIX: &str = ".notebook-backup-";
const PARTIAL_SUFFIX: &str = ".partial";
const MS_PER_DAY: i64 = 86_400_000;

/// Why a backup couldn't be written.
#[derive(Debug)]
pub enum FolderError {
    /// The folder doesn't exist or isn't a directory, such as a drive that
    /// isn't mounted.
    Unavailable,
    /// The folder exists but files can't be created in it.
    NotWritable,
    /// Any other I/O failure.
    Io(io::Error),
}

impl fmt::Display for FolderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("the backup folder is unavailable"),
            Self::NotWritable => f.write_str("the backup folder is not writable"),
            Self::Io(err) => write!(f, "writing the backup failed: {err}"),
        }
    }
}

impl std::error::Error for FolderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for FolderError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Writes `bytes` as a new backup in `folder`, named from `created_at`, and
/// returns its path. An existing file is never overwritten; a taken name gets
/// a ` 2`, ` 3`, … suffix. On failure no backup-named file is left behind.
pub fn write(folder: &Path, created_at: Timestamp, bytes: &[u8]) -> Result<PathBuf, FolderError> {
    write_with(folder, created_at, |file| file.write_all(bytes))
}

/// [`write`] with the file contents produced by `fill`, so tests can fail
/// partway through.
fn write_with(
    folder: &Path,
    created_at: Timestamp,
    fill: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<PathBuf, FolderError> {
    match fs::metadata(folder) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(FolderError::Unavailable),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Err(FolderError::Unavailable),
        Err(err) => return Err(err.into()),
    }
    remove_stale_partials(folder);

    let partial = folder.join(partial_name()?);
    let mut file = File::create_new(&partial).map_err(|err| match err.kind() {
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
            FolderError::NotWritable
        }
        io::ErrorKind::NotFound => FolderError::Unavailable,
        _ => FolderError::Io(err),
    })?;
    let result = fill(&mut file)
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            drop(file);
            publish(&partial, folder, created_at)
        });
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    Ok(result?)
}

fn partial_name() -> Result<String, FolderError> {
    let mut random = [0u8; 8];
    fill_random(&mut random).map_err(|err| FolderError::Io(io::Error::other(err)))?;
    let hex: String = random.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{PARTIAL_PREFIX}{hex}{PARTIAL_SUFFIX}"))
}

/// Removes temporary files left by a backup that stopped partway. Best
/// effort: a file that can't be removed is tried again next time.
fn remove_stale_partials(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(PARTIAL_PREFIX)
            && name.ends_with(PARTIAL_SUFFIX)
            && entry.file_type().is_ok_and(|t| t.is_file())
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Moves the finished `partial` to the first backup name for `created_at`
/// that nothing in `folder` uses.
///
/// A hard link fails instead of replacing an existing file, so a name taken
/// by a sync client in the meantime moves on to the next suffix. Where hard
/// links aren't supported (FAT and exFAT drives), it falls back to checking
/// the name and renaming, which a file appearing in between could replace.
fn publish(partial: &Path, folder: &Path, created_at: Timestamp) -> io::Result<PathBuf> {
    for copy in 1.. {
        let target = folder.join(file_name(created_at, copy));
        match fs::hard_link(partial, &target) {
            Ok(()) => {
                // The backup is complete; a leftover partial goes next time.
                let _ = fs::remove_file(partial);
                return Ok(target);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => {}
        }
        // No hard links here: rename into a name that is free right now.
        match fs::symlink_metadata(&target) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                fs::rename(partial, &target)?;
                return Ok(target);
            }
            Err(err) => return Err(err),
            Ok(_) => {}
        }
    }
    Err(io::Error::other("no free backup name"))
}

/// Deletes the oldest backups of `vault` in `folder` beyond the newest
/// `keep` (at least 1), and returns the deleted file names. `latest` is the
/// backup just written at `now`; it counts toward `keep` and is never
/// deleted.
///
/// Only regular files named like backups whose header names `vault` count.
/// Neither is authenticated, so a file stamped more than a day after `now`
/// (planted, or written under a wrong clock) is not counted either: it would
/// otherwise sort as newest and push out every real backup. Anything that
/// doesn't count, including a file whose header can't be read, is never
/// touched.
pub fn prune(
    folder: &Path,
    vault: VaultId,
    keep: usize,
    latest: &Path,
    now: Timestamp,
) -> io::Result<Vec<String>> {
    let horizon = stamp(Timestamp(now.as_millis().saturating_add(MS_PER_DAY)));
    let latest = latest.file_name();
    let mut older = Vec::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() || Some(entry.file_name().as_os_str()) == latest {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let current = BackupFileName::parse(&name).is_some_and(|n| n.stamp <= horizon.as_str());
        if current && header_vault(&entry.path()) == Some(vault) {
            older.push(name);
        }
    }
    older.sort_by(|a, b| BackupFileName::parse(a).cmp(&BackupFileName::parse(b)));

    let excess = older.len().saturating_sub(keep.max(1) - 1);
    let mut deleted = Vec::with_capacity(excess);
    for name in older.into_iter().take(excess) {
        fs::remove_file(folder.join(&name))?;
        deleted.push(name);
    }
    Ok(deleted)
}

/// The vault named in the backup header at `path`, or `None` if it can't be
/// read as one.
fn header_vault(path: &Path) -> Option<VaultId> {
    let mut prefix = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_HEADER_LEN as u64)
        .read_to_end(&mut prefix)
        .ok()?;
    read_header(&prefix).ok().map(|header| header.vault)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::build;
    use crate::crypto::{RecoveryEntry, RecoveryKey, VaultKey};

    // 2026-10-22T14:03:09Z.
    const T: Timestamp = Timestamp(1_792_677_789_000);
    const DAY: i64 = 86_400_000;

    fn vault(n: u128) -> VaultId {
        VaultId::from_parts(1_000, n)
    }

    /// An empty backup of `vault`.
    fn backup_of(vault: VaultId) -> Vec<u8> {
        let key = VaultKey::generate().unwrap();
        let entry = RecoveryEntry::create(vault, &key, &RecoveryKey::generate().unwrap()).unwrap();
        build(&entry, &key, T, &[]).unwrap()
    }

    fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn folder_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = backup_of(vault(1));
        let path = write(dir.path(), T, &bytes).unwrap();
        assert_eq!(
            path,
            dir.path().join("notebook-backup-20261022-140309.nbbackup")
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(
            names(dir.path()),
            ["notebook-backup-20261022-140309.nbbackup"]
        );
    }

    #[test]
    fn taken_name_gets_a_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("notebook-backup-20261022-140309.nbbackup");
        fs::write(&first, b"existing").unwrap();
        let second = write(dir.path(), T, b"new").unwrap();
        assert_eq!(
            second,
            dir.path()
                .join("notebook-backup-20261022-140309 2.nbbackup")
        );
        let third = write(dir.path(), T, b"newer").unwrap();
        assert_eq!(
            third,
            dir.path()
                .join("notebook-backup-20261022-140309 3.nbbackup")
        );
        assert_eq!(fs::read(&first).unwrap(), b"existing");
        assert_eq!(fs::read(&second).unwrap(), b"new");
    }

    #[test]
    fn folder_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("Dropbox").join("Notebook");
        assert!(matches!(
            write(&missing, T, b"x"),
            Err(FolderError::Unavailable)
        ));
        assert!(!missing.exists());
        assert!(!dir.path().join("Dropbox").exists());

        let file = dir.path().join("not-a-folder");
        fs::write(&file, b"").unwrap();
        assert!(matches!(
            write(&file, T, b"x"),
            Err(FolderError::Unavailable)
        ));
        assert_eq!(names(dir.path()), ["not-a-folder"]);
    }

    #[cfg(unix)]
    #[test]
    fn folder_not_writable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("ro");
        fs::create_dir(&folder).unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o500)).unwrap();
        // Root ignores permissions; nothing to check then.
        if File::create(folder.join("probe")).is_ok() {
            return;
        }
        let result = write(&folder, T, b"x");
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(result, Err(FolderError::NotWritable)));
        assert!(names(&folder).is_empty());
    }

    #[test]
    fn failure_mid_write() {
        let dir = tempfile::tempdir().unwrap();
        let result = write_with(dir.path(), T, |file| {
            file.write_all(b"half a backup")?;
            Err(io::Error::other("disk full"))
        });
        assert!(matches!(result, Err(FolderError::Io(_))));
        assert!(names(dir.path()).is_empty());
    }

    #[test]
    fn stale_partials_are_removed_at_the_next_backup() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(".notebook-backup-0123456789abcdef.partial");
        fs::write(&stale, b"interrupted").unwrap();
        let unrelated = dir.path().join(".other.partial");
        fs::write(&unrelated, b"").unwrap();
        write(dir.path(), T, b"x").unwrap();
        assert_eq!(
            names(dir.path()),
            [".other.partial", "notebook-backup-20261022-140309.nbbackup"]
        );
    }

    #[test]
    fn old_backups_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = backup_of(vault(1));
        let mut written = Vec::new();
        for day in 0..4 {
            written.push(write(dir.path(), Timestamp(T.0 + day * DAY), &bytes).unwrap());
        }
        // A same-second copy sorts after its unsuffixed original.
        let now = Timestamp(T.0 + 3 * DAY);
        let latest = write(dir.path(), now, &bytes).unwrap();

        let deleted = prune(dir.path(), vault(1), 3, &latest, now).unwrap();
        assert_eq!(
            deleted,
            [
                "notebook-backup-20261022-140309.nbbackup",
                "notebook-backup-20261023-140309.nbbackup",
            ]
        );
        assert_eq!(
            names(dir.path()),
            [
                "notebook-backup-20261024-140309.nbbackup",
                "notebook-backup-20261025-140309 2.nbbackup",
                "notebook-backup-20261025-140309.nbbackup",
            ]
        );

        // Keep counts below 1 still keep the newest.
        prune(dir.path(), vault(1), 0, &latest, now).unwrap();
        assert_eq!(
            names(dir.path()),
            ["notebook-backup-20261025-140309 2.nbbackup"]
        );
    }

    #[test]
    fn future_dated_backups_do_not_push_out_real_ones() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = backup_of(vault(1));
        let header = &bytes[..118];
        // Header-only files anyone with folder access could plant, and a
        // backup from a clock running years ahead.
        for name in [
            "notebook-backup-99991230-000000.nbbackup",
            "notebook-backup-99991231-000000.nbbackup",
            "notebook-backup-20301022-140309.nbbackup",
        ] {
            fs::write(dir.path().join(name), header).unwrap();
        }
        let older = write(dir.path(), Timestamp(T.0 - DAY), &bytes).unwrap();
        // Within a day ahead still counts: a clock slightly off.
        let slightly_ahead = write(dir.path(), Timestamp(T.0 + DAY - 1000), &bytes).unwrap();
        let latest = write(dir.path(), T, &bytes).unwrap();

        let deleted = prune(dir.path(), vault(1), 2, &latest, T).unwrap();
        assert_eq!(deleted, ["notebook-backup-20261021-140309.nbbackup"]);
        assert!(!older.exists());
        assert!(latest.exists() && slightly_ahead.exists());

        // Even at the minimum keep count, the backup just written survives.
        prune(dir.path(), vault(1), 1, &latest, T).unwrap();
        assert!(latest.exists());
        assert!(!slightly_ahead.exists());
        assert_eq!(
            names(dir.path()),
            [
                "notebook-backup-20261022-140309.nbbackup",
                "notebook-backup-20301022-140309.nbbackup",
                "notebook-backup-99991230-000000.nbbackup",
                "notebook-backup-99991231-000000.nbbackup",
            ]
        );
    }

    #[test]
    fn other_files_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let ours = backup_of(vault(1));
        let theirs = backup_of(vault(2));
        let put = |name: &str, bytes: &[u8]| fs::write(dir.path().join(name), bytes).unwrap();
        put("notebook-backup-20260101-000000.nbbackup", &theirs);
        put("notebook-backup-20260102-000000.nbbackup", &theirs);
        put("Old notebook-backup-20260103-000000.nbbackup", &ours);
        put(
            "notebook-backup-20260104-000000 (Laptop's conflicted copy).nbbackup",
            &ours,
        );
        put("notebook-backup-20260105-000000.nbbackup", b"not a backup");
        put("notebook-backup-20260106-000000.nbbackup", &ours[..20]);
        put("notes.md", b"# notes");
        fs::create_dir(dir.path().join("notebook-backup-20260107-000000.nbbackup")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let locked = dir.path().join("notebook-backup-20260108-000000.nbbackup");
            fs::write(&locked, &ours).unwrap();
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
            // Root can still read it, which makes it a real backup of ours.
            if File::open(&locked).is_ok() {
                fs::remove_file(&locked).unwrap();
            }
        }
        let before = names(dir.path());
        // Our only real backups are the two written below.
        write(dir.path(), T, &ours).unwrap();
        let now = Timestamp(T.0 + DAY);
        let latest = write(dir.path(), now, &ours).unwrap();

        let deleted = prune(dir.path(), vault(1), 1, &latest, now).unwrap();
        assert_eq!(deleted, ["notebook-backup-20261022-140309.nbbackup"]);
        let mut expected = before;
        expected.push("notebook-backup-20261023-140309.nbbackup".into());
        expected.sort();
        assert_eq!(names(dir.path()), expected);
    }
}
