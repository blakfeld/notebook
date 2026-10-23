//! The folder output, for native clients.
//!
//! Files are written into a staging folder next to the destination, which is
//! renamed into place only once everything is on disk, so the user sees a
//! complete export folder or none. The staging folder is visible and named
//! as unfinished, so one left by a crash is noticed rather than hidden, and on
//! Unix it is private to the user (the rename keeps its mode).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{ExportError, ExportPlan};
use crate::note::Timestamp;

/// Writes `plan`'s export folder inside `parent` and returns its path. An
/// existing folder with the export's name is never touched; the export takes
/// the smallest free `-N` suffix (N ≥ 2) instead. On error nothing is left
/// behind.
pub fn write_dir(plan: &ExportPlan, parent: &Path) -> Result<PathBuf, ExportError> {
    let staging = create_staging(parent)?;
    match fill(&staging, plan).and_then(|()| publish(&staging, parent, &plan.folder_name)) {
        Ok(path) => Ok(path),
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            Err(e.into())
        }
    }
}

fn create_staging(parent: &Path) -> io::Result<PathBuf> {
    loop {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let path = parent.join(format!(
            "notebook-export-incomplete-{:016x}",
            u64::from_le_bytes(random)
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

fn fill(staging: &Path, plan: &ExportPlan) -> io::Result<()> {
    for file in &plan.files {
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staging.join(&file.name))?;
        out.write_all(file.contents.as_bytes())?;
        // Times are a convenience; some filesystems can't set them.
        if let Some(time) = system_time(file.modified) {
            let _ = out.set_modified(time);
        }
        out.sync_all()?;
    }
    sync_dir(staging);
    Ok(())
}

/// Renames `staging` to the first free export folder name in `parent`.
fn publish(staging: &Path, parent: &Path, folder_name: &str) -> io::Result<PathBuf> {
    for n in 1u32.. {
        let name = match n {
            1 => folder_name.to_owned(),
            n => format!("{folder_name}-{n}"),
        };
        let target = parent.join(name);
        match fs::symlink_metadata(&target) {
            Ok(_) => continue,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        match fs::rename(staging, &target) {
            Ok(()) => {
                // The export is in place now; nothing after this may fail.
                sync_dir(parent);
                return Ok(target);
            }
            // Taken between the check and the rename.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::AlreadyExists | io::ErrorKind::DirectoryNotEmpty
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("ran out of export folder suffixes")
}

/// Flushes a directory's entries to disk, best effort: some filesystems
/// can't sync a directory, and a parent may be writable but not readable.
/// The files themselves are already synced.
fn sync_dir(path: &Path) {
    if cfg!(unix) {
        let _ = File::open(path).and_then(|dir| dir.sync_all());
    }
}

fn system_time(ts: Timestamp) -> Option<SystemTime> {
    let ms = ts.as_millis();
    let offset = Duration::from_millis(ms.unsigned_abs());
    if ms >= 0 {
        SystemTime::UNIX_EPOCH.checked_add(offset)
    } else {
        SystemTime::UNIX_EPOCH.checked_sub(offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::tests::note;
    use crate::export::{ExportOptions, plan};

    const NOW: Timestamp = Timestamp(1_792_591_385_000);

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn sample() -> ExportPlan {
        plan(
            &[note(1, "Grocery list"), note(2, "Ideas"), note(3, "Ideas")],
            ExportOptions::default(),
            NOW,
        )
    }

    #[test]
    fn export_into_a_chosen_folder() {
        let parent = tempfile::tempdir().unwrap();
        let plan = sample();
        let path = write_dir(&plan, parent.path()).unwrap();
        assert_eq!(path, parent.path().join("notebook-export-20261021-140305"));
        assert_eq!(entries(parent.path()), ["notebook-export-20261021-140305"]);
        assert_eq!(
            entries(&path),
            ["Grocery list.md", "Ideas 2.md", "Ideas.md"]
        );
        for file in &plan.files {
            assert_eq!(
                fs::read_to_string(path.join(&file.name)).unwrap(),
                file.contents
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn export_folder_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir().unwrap();
        let path = write_dir(&sample(), parent.path()).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn empty_export_writes_an_empty_folder() {
        let parent = tempfile::tempdir().unwrap();
        let plan = plan(&[], ExportOptions::default(), NOW);
        let path = write_dir(&plan, parent.path()).unwrap();
        assert!(entries(&path).is_empty());
    }

    #[test]
    fn existing_folder_is_not_overwritten() {
        let parent = tempfile::tempdir().unwrap();
        let existing = parent.path().join("notebook-export-20261021-140305");
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("keep.txt"), "mine").unwrap();
        // An empty folder with the next name is taken too.
        fs::create_dir(parent.path().join("notebook-export-20261021-140305-2")).unwrap();

        let path = write_dir(&sample(), parent.path()).unwrap();
        assert_eq!(
            path,
            parent.path().join("notebook-export-20261021-140305-3")
        );
        assert_eq!(entries(&existing), ["keep.txt"]);
        assert_eq!(
            fs::read_to_string(existing.join("keep.txt")).unwrap(),
            "mine"
        );
        assert!(entries(&parent.path().join("notebook-export-20261021-140305-2")).is_empty());
        assert_eq!(entries(&path).len(), 3);
    }

    #[test]
    fn file_times_match_notes() {
        let parent = tempfile::tempdir().unwrap();
        let mut n = note(1, "x");
        // 2024-10-22T14:03:05Z.
        n.set_body("y", Timestamp(1_729_605_785_000));
        let plan = plan(&[n], ExportOptions::default(), NOW);
        let path = write_dir(&plan, parent.path()).unwrap();
        let modified = fs::metadata(path.join("y.md")).unwrap().modified().unwrap();
        assert_eq!(
            modified,
            SystemTime::UNIX_EPOCH + Duration::from_millis(1_729_605_785_000)
        );
    }

    #[test]
    fn write_failure_leaves_nothing_behind() {
        let parent = tempfile::tempdir().unwrap();
        let mut plan = sample();
        // A second file with the same name fails `create_new` after the
        // first file has been written.
        let dup = plan.files[0].clone();
        plan.files.push(dup);
        let err = write_dir(&plan, parent.path()).unwrap_err();
        assert!(matches!(err, ExportError::Io(e) if e.kind() == io::ErrorKind::AlreadyExists));
        assert!(entries(parent.path()).is_empty());
    }

    #[test]
    fn missing_parent_is_an_error() {
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("nope");
        assert!(write_dir(&sample(), &missing).is_err());
        assert!(entries(parent.path()).is_empty());
    }

    #[test]
    fn negative_times_convert() {
        assert_eq!(
            system_time(Timestamp(-1000)),
            SystemTime::UNIX_EPOCH.checked_sub(Duration::from_secs(1))
        );
    }
}
