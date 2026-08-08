//! One finished or current logical segment, opened.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use kronika_format::{Catalog, Entry, ReadAt as _, crc32c};
use kronika_registry::{
    Bytes, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, Row, VerifiedSection, decode_rows,
};
use kronika_store::{ActiveSnapshot, LocalDir, read_catalog};

use crate::dictionary::Dictionary;
use crate::error::ReaderError;
use crate::{SegmentRef, SegmentSource};

#[derive(Debug)]
enum Source {
    Finished { file: File, catalog: Catalog },
    Active(ActiveSnapshot),
}

/// An open finished `.zms` or captured `active.wal` logical segment.
///
/// Section bodies are read on demand. A finished segment holds one file
/// descriptor; a current segment holds one descriptor for its captured prefix.
#[derive(Debug)]
pub struct Segment {
    path: PathBuf,
    source: Source,
    min_ts: i64,
    max_ts: i64,
    window_count: u32,
    section_rows: BTreeMap<u32, u64>,
}

impl Segment {
    pub(crate) fn open(
        dir: &LocalDir,
        root: &Path,
        unit: &SegmentRef,
    ) -> Result<Self, ReaderError> {
        match &unit.source {
            SegmentSource::Finished(finished) => {
                let file = dir.open_finished(finished)?;
                let catalog = read_catalog(&file)?;
                dir.validate_finished_file(&file, finished)?;
                let section_rows = rows_by_type(std::iter::once(&catalog));
                Ok(Self {
                    path: finished_path(root, finished),
                    source: Source::Finished { file, catalog },
                    min_ts: unit.min_ts,
                    max_ts: unit.max_ts,
                    window_count: finished.summary.window_count,
                    section_rows,
                })
            }
            SegmentSource::Active(snapshot) => {
                let window_count = u32::try_from(snapshot.parts().len()).map_err(|_overflow| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "active segment window count does not fit u32",
                    )
                })?;
                let section_rows = rows_by_type(snapshot.parts().iter().map(|part| &part.catalog));
                Ok(Self {
                    path: root.join("active.wal"),
                    source: Source::Active(snapshot.clone()),
                    min_ts: unit.min_ts,
                    max_ts: unit.max_ts,
                    window_count,
                    section_rows,
                })
            }
        }
    }

    /// Where the segment was read from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Earliest timestamp the segment carries, unix microseconds.
    #[must_use]
    pub const fn min_ts(&self) -> i64 {
        self.min_ts
    }

    /// Latest timestamp the segment carries, unix microseconds.
    #[must_use]
    pub const fn max_ts(&self) -> i64 {
        self.max_ts
    }

    /// Collection windows coalesced into the logical segment.
    #[must_use]
    pub const fn window_count(&self) -> u32 {
        self.window_count
    }

    /// Section types present in the segment, in numeric order.
    pub fn type_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.section_rows.keys().copied()
    }

    /// Rows recorded for `type_id`, or `None` when the section is absent.
    #[must_use]
    pub fn rows_of(&self, type_id: u32) -> Option<u64> {
        self.section_rows.get(&type_id).copied()
    }

    /// Decode every section of `type_id` into column-addressable rows.
    ///
    /// Current-segment sections are concatenated in journal order.
    ///
    /// # Errors
    ///
    /// Returns an error when a body fails its checksum or the codec rejects it.
    pub fn rows(&self, type_id: u32) -> Result<Vec<Row>, ReaderError> {
        let mut rows = Vec::new();
        match &self.source {
            Source::Finished { file, catalog } => {
                if let Some(entry) = entry(catalog, type_id) {
                    rows.extend(decode_section_rows(type_id, finished_body(file, entry)?)?);
                }
            }
            Source::Active(snapshot) => {
                for (part_index, part) in snapshot.parts().iter().enumerate() {
                    if let Some(entry) = entry(&part.catalog, type_id) {
                        rows.extend(decode_section_rows(
                            type_id,
                            active_body(snapshot, part_index, entry)?,
                        )?);
                    }
                }
            }
        }
        Ok(rows)
    }

    /// Decode the complete segment dictionary.
    ///
    /// Both `dict.strings` and `dict.blobs` are included. Current-segment
    /// dictionary deltas are applied in journal order.
    ///
    /// # Errors
    ///
    /// Returns an error when a dictionary body fails its checksum or codec.
    pub fn dictionary(&self) -> Result<Dictionary, ReaderError> {
        let mut dictionary = Dictionary::default();
        match &self.source {
            Source::Finished { file, catalog } => {
                decode_dictionary_catalog(&mut dictionary, catalog, |entry| {
                    finished_body(file, entry)
                })?;
            }
            Source::Active(snapshot) => {
                for (part_index, part) in snapshot.parts().iter().enumerate() {
                    decode_dictionary_catalog(&mut dictionary, &part.catalog, |entry| {
                        active_body(snapshot, part_index, entry)
                    })?;
                }
            }
        }
        Ok(dictionary)
    }
}

#[allow(
    single_use_lifetimes,
    reason = "the named lifetime is required in this impl-Trait associated item on Rust 1.96"
)]
fn rows_by_type<'a>(catalogs: impl IntoIterator<Item = &'a Catalog>) -> BTreeMap<u32, u64> {
    let mut rows = BTreeMap::new();
    for catalog in catalogs {
        for entry in &catalog.entries {
            *rows.entry(entry.type_id).or_default() += u64::from(entry.rows);
        }
    }
    rows
}

fn decode_section_rows(type_id: u32, section: VerifiedSection) -> Result<Vec<Row>, ReaderError> {
    decode_rows(type_id, section).map_err(|source| ReaderError::Section { type_id, source })
}

fn entry(catalog: &Catalog, type_id: u32) -> Option<&Entry> {
    catalog
        .entries
        .iter()
        .find(|entry| entry.type_id == type_id)
}

fn decode_dictionary_catalog(
    dictionary: &mut Dictionary,
    catalog: &Catalog,
    mut body: impl FnMut(&Entry) -> Result<VerifiedSection, ReaderError>,
) -> Result<(), ReaderError> {
    for entry in &catalog.entries {
        if matches!(entry.type_id, DICT_STRINGS_TYPE_ID | DICT_BLOBS_TYPE_ID) {
            let type_id = entry.type_id;
            dictionary
                .decode(type_id, body(entry)?)
                .map_err(|source| ReaderError::Section { type_id, source })?;
        }
    }
    Ok(())
}

fn finished_body(file: &File, entry: &Entry) -> Result<VerifiedSection, ReaderError> {
    let len = usize::try_from(entry.len).map_err(|_overflow| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("section {} declares {} bytes", entry.type_id, entry.len),
        )
    })?;
    let mut buffer = vec![0_u8; len];
    file.read_exact_at(&mut buffer, entry.offset)?;
    verified_body(entry, buffer)
}

fn active_body(
    snapshot: &ActiveSnapshot,
    part_index: usize,
    entry: &Entry,
) -> Result<VerifiedSection, ReaderError> {
    let buffer = snapshot.read_part_range(part_index, entry.offset, entry.len)?;
    verified_body(entry, buffer)
}

fn verified_body(entry: &Entry, body: Vec<u8>) -> Result<VerifiedSection, ReaderError> {
    VerifiedSection::verify(Bytes::from(body), entry.crc32c, crc32c).map_err(|source| {
        ReaderError::Section {
            type_id: entry.type_id,
            source,
        }
    })
}

fn finished_path(root: &Path, unit: &kronika_store::FinalUnit) -> PathBuf {
    let day = unit.address.day;
    root.join(day.year_component())
        .join(day.month_component())
        .join(day.day_component())
        .join(unit.address.zms_name())
}
