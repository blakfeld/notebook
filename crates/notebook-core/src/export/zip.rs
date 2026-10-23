//! The zip output: the export folder as one in-memory archive.

use std::io::{Cursor, Write};

use ::zip::write::SimpleFileOptions;
use ::zip::{CompressionMethod, DateTime, ZipWriter};

use super::time::civil;
use super::{ExportError, ExportPlan};
use crate::note::Timestamp;

/// Fixed so the same plan always compresses to the same bytes.
const DEFLATE_LEVEL: i64 = 6;

/// Builds the zip for `plan`: the export folder's entry first, then each
/// file inside it in name order. Entry times are the notes' updated times
/// in UTC, clamped to the range zip can store (1980–2107). The zip crate
/// sets the UTF-8 flag on every non-ASCII name.
pub fn write_zip(plan: &ExportPlan) -> Result<Vec<u8>, ExportError> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = |ts| {
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(DEFLATE_LEVEL))
            .last_modified_time(dos_time(ts))
    };
    zip.add_directory(format!("{}/", plan.folder_name), options(plan.exported))?;
    for file in &plan.files {
        zip.start_file(
            format!("{}/{}", plan.folder_name, file.name),
            options(file.modified),
        )?;
        zip.write_all(file.contents.as_bytes())?;
    }
    Ok(zip.finish()?.into_inner())
}

fn dos_time(ts: Timestamp) -> DateTime {
    let c = civil(ts);
    let dt = match c.year {
        ..1980 => DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0),
        2108.. => DateTime::from_date_and_time(2107, 12, 31, 23, 59, 58),
        // In range, so every field fits its narrower type.
        year => DateTime::from_date_and_time(
            year as u16,
            c.month as u8,
            c.day as u8,
            c.hour as u8,
            c.minute as u8,
            c.second as u8,
        ),
    };
    dt.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use ::zip::ZipArchive;

    use super::*;
    use crate::export::tests::note;
    use crate::export::{ExportOptions, plan};

    fn sample() -> ExportPlan {
        let notes = [
            note(1, "Grocery list\n- milk"),
            note(2, "Café"),
            note(3, "Ideas"),
            note(4, "Ideas"),
            note(5, ""),
        ];
        plan(
            &notes,
            ExportOptions::default(),
            Timestamp(1_792_591_385_000),
        )
    }

    #[test]
    fn archive_holds_the_plan() {
        let plan = sample();
        let bytes = write_zip(&plan).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), plan.files.len() + 1);

        let folder = archive.by_index(0).unwrap();
        assert_eq!(folder.name().unwrap(), "notebook-export-20261021-140305/");
        assert!(folder.is_dir());
        drop(folder);

        for (i, file) in plan.files.iter().enumerate() {
            let mut entry = archive.by_index(i + 1).unwrap();
            assert_eq!(
                entry.name().unwrap(),
                format!("notebook-export-20261021-140305/{}", file.name)
            );
            assert_eq!(entry.compression(), CompressionMethod::Deflated);
            let mut contents = String::new();
            entry.read_to_string(&mut contents).unwrap();
            assert_eq!(contents, file.contents);
            let modified = entry.last_modified().unwrap();
            assert_eq!(
                (modified.year(), modified.month(), modified.day()),
                (2024, 10, 21)
            );
            assert_eq!(
                (modified.hour(), modified.minute(), modified.second()),
                (14, 3, 4)
            );
        }
    }

    #[test]
    fn empty_export_is_just_the_folder() {
        let plan = plan(&[], ExportOptions::default(), Timestamp(0));
        let bytes = write_zip(&plan).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), 1);
        assert!(archive.by_index(0).unwrap().is_dir());
    }

    #[test]
    fn deterministic() {
        let plan = sample();
        assert_eq!(write_zip(&plan).unwrap(), write_zip(&plan).unwrap());
    }

    /// The general-purpose flags of the central directory entry named
    /// `name`.
    fn central_flags(bytes: &[u8], name: &str) -> u16 {
        let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        (0..bytes.len() - 46)
            .find_map(|i| {
                if bytes[i..i + 4] != *b"PK\x01\x02" {
                    return None;
                }
                let len = usize::from(u16_at(i + 28));
                (bytes.get(i + 46..i + 46 + len)? == name.as_bytes()).then(|| u16_at(i + 8))
            })
            .expect("entry present")
    }

    #[test]
    fn non_ascii_name_is_flagged_utf8() {
        let plan = sample();
        let bytes = write_zip(&plan).unwrap();
        let name = "notebook-export-20261021-140305/Café.md";
        assert_ne!(central_flags(&bytes, name) & (1 << 11), 0);
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert!(archive.by_name(name).is_ok());
    }

    #[test]
    fn dos_time_is_clamped() {
        let early = dos_time(Timestamp(0));
        assert_eq!((early.year(), early.month(), early.day()), (1980, 1, 1));
        // Year 2200.
        let late = dos_time(Timestamp(7_258_118_400_000));
        assert_eq!((late.year(), late.month(), late.day()), (2107, 12, 31));
    }
}
