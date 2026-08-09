//! One finished or current logical segment, opened.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kronika_format::{Catalog, Entry, ReadAt as _, crc32c};
use kronika_registry::{
    Bytes, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, Row, VerifiedSection, contract,
    logical_section_name, visit_rows,
};
use kronika_store::{ActiveSnapshot, LocalDir};

use crate::dictionary::Dictionary;
use crate::error::ReaderError;
use crate::{SegmentKind, SegmentRef, SegmentSource};

#[derive(Debug)]
enum Source {
    Finished { file: File, catalog: Arc<Catalog> },
    Active(ActiveSnapshot),
}

/// An open finished `.zms` or captured `active.wal` logical segment.
///
/// Section bodies are read on demand. A finished segment holds one file
/// descriptor; a current segment holds one descriptor for its captured prefix.
#[derive(Debug)]
pub struct Segment {
    id: i64,
    kind: SegmentKind,
    path: PathBuf,
    source: Source,
    min_ts: i64,
    max_ts: i64,
    captured_bytes: u64,
    window_count: u32,
    section_rows: BTreeMap<u32, Section>,
}

/// What one section type occupies in a segment.
///
/// A current segment coalesces several parts, so both figures are the sum over
/// the parts that carry the type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Section {
    /// Rows the catalogs recorded.
    pub rows: u64,
    /// Encoded body bytes, before the container's own framing.
    pub bytes: u64,
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
                let catalog = Arc::new(kronika_store::read_catalog(&file)?);
                dir.validate_finished_file(&file, finished)?;
                let section_rows = rows_by_type(std::iter::once(catalog.as_ref()));
                Ok(Self {
                    id: unit.segment_id,
                    kind: SegmentKind::Finished,
                    path: finished_path(root, finished),
                    source: Source::Finished { file, catalog },
                    min_ts: unit.min_ts,
                    max_ts: unit.max_ts,
                    captured_bytes: unit.captured_bytes,
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
                    id: unit.segment_id,
                    kind: SegmentKind::Active,
                    path: root.join("active.wal"),
                    source: Source::Active(snapshot.clone()),
                    min_ts: unit.min_ts,
                    max_ts: unit.max_ts,
                    captured_bytes: unit.captured_bytes,
                    window_count,
                    section_rows,
                })
            }
        }
    }

    /// Stable segment id.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Whether this segment is immutable or a captured journal prefix.
    #[must_use]
    pub const fn kind(&self) -> SegmentKind {
        self.kind
    }

    /// Exact committed active-journal position, when current.
    #[must_use]
    pub const fn active_position(&self) -> Option<u64> {
        match self.kind {
            SegmentKind::Finished => None,
            SegmentKind::Active => Some(self.captured_bytes),
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

    /// Bytes in the finished file or captured current-journal prefix.
    #[must_use]
    pub const fn captured_bytes(&self) -> u64 {
        self.captured_bytes
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
        self.section_rows.get(&type_id).map(|section| section.rows)
    }

    /// Every section the segment carries, in numeric order, with what it cost.
    ///
    /// This is what a size report is built from: the bodies are the segment
    /// minus its catalog and framing.
    pub fn sections(&self) -> impl Iterator<Item = (u32, Section)> + '_ {
        self.section_rows
            .iter()
            .map(|(type_id, section)| (*type_id, *section))
    }

    /// Physical layouts present under one stable public section name.
    pub fn layouts<'a>(
        &'a self,
        logical_name: &'a str,
    ) -> impl Iterator<Item = (u32, Section)> + 'a {
        self.sections().filter(move |(type_id, _section)| {
            contract(*type_id).is_some()
                && logical_section_name(*type_id).is_some_and(|name| name == logical_name)
        })
    }

    /// Decode every section of `type_id` into column-addressable rows.
    ///
    /// Current-segment sections are concatenated in journal order.
    ///
    /// # Errors
    ///
    /// Returns an error when a body fails its checksum or the codec rejects it.
    pub fn rows(&self, type_id: u32) -> Result<Vec<Row>, ReaderError> {
        let Some(contract) = contract(type_id) else {
            return Err(ReaderError::Section {
                type_id,
                source: kronika_registry::CodecError::UnknownType { type_id },
            });
        };
        let columns: Vec<&str> = contract.columns.iter().map(|column| column.name).collect();
        let mut rows = Vec::new();
        self.visit_rows(type_id, &columns, 0, usize::MAX, |_ordinal, row| {
            rows.push(row);
            true
        })?;
        Ok(rows)
    }

    /// Visit a projected physical row range in stable ascending order.
    ///
    /// Current-segment ordinals span all journal parts carrying `type_id`.
    /// Catalog row counts skip earlier parts without opening their bodies, and
    /// returning `false` from `visitor` stops decoding immediately.
    ///
    /// # Errors
    ///
    /// Returns an error when a selected body fails its checksum or projection
    /// decode.
    pub fn visit_rows(
        &self,
        type_id: u32,
        columns: &[&str],
        offset: u64,
        limit: usize,
        mut visitor: impl FnMut(u64, Row) -> bool,
    ) -> Result<usize, ReaderError> {
        let mut visited = 0_usize;
        let mut global_base = 0_u64;
        let mut remaining_offset = offset;
        match &self.source {
            Source::Finished { file, catalog } => {
                if let Some(entry) = entry(catalog, type_id) {
                    let rows = u64::from(entry.rows);
                    if remaining_offset < rows {
                        let local_offset =
                            usize::try_from(remaining_offset).map_err(|_overflow| {
                                std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    "row offset does not fit usize",
                                )
                            })?;
                        let count = visit_body(
                            type_id,
                            finished_body(file, entry)?,
                            columns,
                            BodyVisit {
                                expected_rows: u64::from(entry.rows),
                                offset: local_offset,
                                limit,
                                base: 0,
                            },
                            &mut visitor,
                        )?;
                        visited = visited.saturating_add(count);
                    }
                }
            }
            Source::Active(snapshot) => {
                for (part_index, part) in snapshot.parts().iter().enumerate() {
                    let Some(entry) = entry(&part.catalog, type_id) else {
                        continue;
                    };
                    if visited >= limit {
                        break;
                    }
                    if remaining_offset >= u64::from(entry.rows) {
                        remaining_offset -= u64::from(entry.rows);
                        global_base = global_base.saturating_add(u64::from(entry.rows));
                        continue;
                    }
                    let local_offset = usize::try_from(remaining_offset).map_err(|_overflow| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "row offset does not fit usize",
                        )
                    })?;
                    let mut keep_going = true;
                    let count = visit_body(
                        type_id,
                        active_body(snapshot, part_index, entry)?,
                        columns,
                        BodyVisit {
                            expected_rows: u64::from(entry.rows),
                            offset: local_offset,
                            limit: limit.saturating_sub(visited),
                            base: global_base,
                        },
                        &mut |ordinal, row| {
                            keep_going = visitor(ordinal, row);
                            keep_going
                        },
                    )?;
                    visited = visited.saturating_add(count);
                    remaining_offset = 0;
                    global_base = global_base.saturating_add(u64::from(entry.rows));
                    if !keep_going {
                        break;
                    }
                }
            }
        }
        Ok(visited)
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

    /// Decode only dictionary values named by `ids`.
    ///
    /// The dictionary id column is scanned first and large value columns are
    /// projected only for matching rows. An empty set opens no dictionary
    /// section.
    ///
    /// # Errors
    ///
    /// Returns an error when a dictionary body fails its checksum or codec.
    pub fn dictionary_for(&self, ids: &HashSet<u64>) -> Result<Dictionary, ReaderError> {
        let mut dictionary = Dictionary::default();
        if ids.is_empty() {
            return Ok(dictionary);
        }
        match &self.source {
            Source::Finished { file, catalog } => {
                decode_selected_dictionary_catalog(&mut dictionary, catalog, ids, |entry| {
                    finished_body(file, entry)
                })?;
            }
            Source::Active(snapshot) => {
                for (part_index, part) in snapshot.parts().iter().enumerate() {
                    decode_selected_dictionary_catalog(
                        &mut dictionary,
                        &part.catalog,
                        ids,
                        |entry| active_body(snapshot, part_index, entry),
                    )?;
                }
            }
        }
        Ok(dictionary)
    }
}

#[derive(Debug, Clone, Copy)]
struct BodyVisit {
    expected_rows: u64,
    offset: usize,
    limit: usize,
    base: u64,
}

fn visit_body(
    type_id: u32,
    body: VerifiedSection,
    columns: &[&str],
    request: BodyVisit,
    visitor: &mut impl FnMut(u64, Row) -> bool,
) -> Result<usize, ReaderError> {
    visit_rows(
        type_id,
        body,
        columns,
        Some(request.expected_rows),
        request.offset,
        request.limit,
        |local_ordinal, row| visitor(request.base.saturating_add(local_ordinal), row),
    )
    .map_err(|source| ReaderError::Section { type_id, source })
}

#[allow(
    single_use_lifetimes,
    reason = "the named lifetime is required in this impl-Trait associated item on Rust 1.96"
)]
fn rows_by_type<'a>(catalogs: impl IntoIterator<Item = &'a Catalog>) -> BTreeMap<u32, Section> {
    let mut sections: BTreeMap<u32, Section> = BTreeMap::new();
    for catalog in catalogs {
        for entry in &catalog.entries {
            let section = sections.entry(entry.type_id).or_default();
            section.rows += u64::from(entry.rows);
            section.bytes += entry.len;
        }
    }
    sections
}

fn entry(catalog: &Catalog, type_id: u32) -> Option<&Entry> {
    catalog
        .entries
        .iter()
        .find(|entry| entry.type_id == type_id)
}

fn decode_selected_dictionary_catalog(
    dictionary: &mut Dictionary,
    catalog: &Catalog,
    ids: &HashSet<u64>,
    mut body: impl FnMut(&Entry) -> Result<VerifiedSection, ReaderError>,
) -> Result<(), ReaderError> {
    for entry in &catalog.entries {
        if matches!(entry.type_id, DICT_STRINGS_TYPE_ID | DICT_BLOBS_TYPE_ID) {
            let type_id = entry.type_id;
            dictionary
                .decode_selected(type_id, body(entry)?, ids, u64::from(entry.rows))
                .map_err(|source| ReaderError::Section { type_id, source })?;
        }
    }
    Ok(())
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
                .decode(type_id, body(entry)?, u64::from(entry.rows))
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
