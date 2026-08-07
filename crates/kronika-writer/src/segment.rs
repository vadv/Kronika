//! Segment completion: merge the journal's parts into one immutable segment.
//!
//! Coalesces collection-window sections by type into a temporary file and
//! writes the end catalog last.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::FileExt as _;

use arrow_array::{
    Array, BinaryArray, BooleanArray, FixedSizeBinaryArray, RecordBatch, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use kronika_format::{
    Catalog, ENTRY_LEN, Entry, EntrySnapshot, FORMAT_VERSION, HotMark, MAGIC, META_LEN, PartError,
    Placement, StrId, TAIL_INDEX_LEN, TailIndex, crc32c, validate_part_catalog,
};
use kronika_layout::{FileIdentity, LayoutError, SegmentAddress, SegmentId, WriterOwner, ZmsTemp};
use kronika_registry::{
    Bytes, CodecError, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, MAX_DECODED_SECTION_BYTES,
    MAX_ROW_GROUPS, MAX_SECTION_BYTES, MAX_SECTION_ROWS, VerifiedSection, decode_any,
    encode_final_batches, final_data_body_bound, validate_plain_parquet_decode_work,
};
use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};

use crate::{Journal, JournalError, JournalPartRef};

mod compare;
mod dictionary;
mod error;

#[cfg(test)]
use compare::arm_after_first_comparison_chunk;
use compare::{files_equal, validate_segment};
use dictionary::normalize_dictionary;
pub use error::WriteError;

const MAX_CATALOG_BYTES: usize = 64 * 1024 * 1024;
const COMPARE_BUFFER_BYTES: usize = 64 * 1024;
const MAX_CATALOG_ENTRIES: usize = (MAX_CATALOG_BYTES - META_LEN) / ENTRY_LEN;

#[cfg(test)]
std::thread_local! {
    static AFTER_FIRST_COMPARISON_CHUNK:
        std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

/// Runs a test-only hook at one point of the byte comparison.
#[macro_export]
macro_rules! write_test_hook {
    (AfterFirstComparisonChunk) => {
        #[cfg(test)]
        run_after_first_comparison_chunk();
    };
}

/// What a completed segment contains, for the caller's metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteSummary {
    /// Number of catalog entries (sections) written.
    pub sections: usize,
    /// Total segment length, bytes.
    pub bytes: u64,
    /// Minimal timestamp across the segment, unix microseconds.
    pub min_ts: i64,
    /// Maximal timestamp across the segment, unix microseconds.
    pub max_ts: i64,
}

/// Write journal parts into the immutable segment at `address`.
///
/// The final ZMS is never overwritten. Call `Journal::reset` only after `Ok`.
///
/// # Errors
///
/// Returns [`WriteError`] when the journal is empty, a part is invalid, I/O
/// fails, or an existing final segment cannot be proven byte-identical.
pub fn write_segment(
    journal: &Journal,
    owner: &WriterOwner,
    address: SegmentAddress,
) -> Result<WriteSummary, WriteError> {
    if journal.parts().is_empty() {
        return Err(WriteError::Empty);
    }
    if let Some(segment_id) = journal.segment_id()
        && segment_id != address.id
    {
        return Err(WriteError::SegmentIdMismatch {
            journal: segment_id,
            destination: address.id,
        });
    }
    let mut temporary = owner.create_zms_temp(address)?;
    let summary = write_tmp(journal, &mut temporary)?;
    let generated = temporary.try_clone_file()?;
    if !validate_segment(&generated, summary)? {
        return Err(WriteError::GeneratedSegmentInvalid);
    }
    match temporary.publish() {
        Ok(()) => Ok(summary),
        Err(LayoutError::SegmentAlreadyExists { .. }) => {
            let existing = owner.root().open_zms(address)?;
            let existing_identity = FileIdentity::from_file(&existing)?;
            if !validate_segment(&existing, summary)? {
                return Err(WriteError::ExistingSegmentInvalid);
            }
            if !files_equal(&generated, &existing)? {
                return Err(WriteError::ExistingSegmentMismatch);
            }
            if FileIdentity::from_file(&existing)? != existing_identity {
                return Err(WriteError::ExistingSegmentMismatch);
            }
            let named_existing = owner.root().open_zms(address)?;
            if FileIdentity::from_file(&named_existing)? != existing_identity {
                return Err(WriteError::ExistingSegmentMismatch);
            }
            temporary.discard()?;
            Ok(summary)
        }
        Err(error) => Err(WriteError::Layout(error)),
    }
}

fn checked_catalog_entries(current: usize, additional: usize) -> Result<usize, WriteError> {
    let attempted_entries = current
        .checked_add(additional)
        .ok_or(WriteError::CatalogTooLarge {
            attempted_entries: usize::MAX,
            max_entries: MAX_CATALOG_ENTRIES,
        })?;
    if attempted_entries > MAX_CATALOG_ENTRIES {
        return Err(WriteError::CatalogTooLarge {
            attempted_entries,
            max_entries: MAX_CATALOG_ENTRIES,
        });
    }
    Ok(attempted_entries)
}

#[derive(Debug, Clone, Copy)]
struct SectionDescriptor {
    part: JournalPartRef,
    entry: Entry,
}

#[derive(Debug)]
struct SegmentPlan {
    by_type: BTreeMap<u32, Vec<SectionDescriptor>>,
    min_ts: i64,
    max_ts: i64,
    window_count: u32,
}

/// Write the merged segment to `tmp` and flush the encoder.
///
/// Publication synchronizes the file and its parent directories.
fn write_tmp(journal: &Journal, temporary: &mut ZmsTemp<'_>) -> Result<WriteSummary, WriteError> {
    let mut plan = plan_segment(journal)?;
    let strings = plan
        .by_type
        .remove(&DICT_STRINGS_TYPE_ID)
        .unwrap_or_default();
    let blobs = plan.by_type.remove(&DICT_BLOBS_TYPE_ID).unwrap_or_default();
    let dictionary = normalize_dictionary(journal, &strings, &blobs)?;

    let mut out = BufWriter::new(temporary.file_mut());

    out.write_all(&MAGIC)?;
    let mut offset = MAGIC.len() as u64;
    let mut entries: Vec<Entry> = Vec::new();

    for (type_id, descriptors) in plan.by_type {
        let declared_rows = aggregate_rows(type_id, &descriptors)?;
        let mut batches = Vec::<RecordBatch>::new();
        let mut decoded_rows = 0_usize;
        let mut list_i32_child_values = 0_usize;
        for descriptor in descriptors {
            let decoded = decode_any(type_id, read_verified_body(journal, descriptor)?)?;
            if decoded.stats.rows != descriptor.entry.rows as usize {
                return Err(WriteError::RowCountMismatch {
                    type_id,
                    declared: descriptor.entry.rows,
                    decoded: decoded.stats.rows,
                });
            }
            let projected_rows = decoded_rows.checked_add(decoded.stats.rows).ok_or(
                WriteError::ArithmeticOverflow {
                    what: "decoded row count",
                },
            )?;
            let projected_list_values = list_i32_child_values
                .checked_add(decoded.stats.list_i32_child_values)
                .ok_or(WriteError::ArithmeticOverflow {
                    what: "decoded ListI32 child count",
                })?;
            final_data_body_bound(type_id, projected_rows, projected_list_values)?;
            decoded_rows = projected_rows;
            list_i32_child_values = projected_list_values;
            batches.extend(decoded.batches);
        }
        if decoded_rows != declared_rows {
            return Err(WriteError::RowCountMismatch {
                type_id,
                declared: u32::try_from(declared_rows).unwrap_or(u32::MAX),
                decoded: decoded_rows,
            });
        }
        let body = encode_final_batches(type_id, batches)?;
        write_section(
            &mut out,
            &mut entries,
            &mut offset,
            type_id,
            u32::try_from(declared_rows).map_err(|_error| WriteError::ArithmeticOverflow {
                what: "section row count",
            })?,
            &body,
        )?;
    }

    for section in dictionary.sections()? {
        write_section(
            &mut out,
            &mut entries,
            &mut offset,
            section.type_id,
            section.rows,
            &section.body,
        )?;
    }

    let sections = entries.len();
    let catalog = Catalog {
        entries,
        min_ts: plan.min_ts,
        max_ts: plan.max_ts,
        format_version: FORMAT_VERSION,
        window_count: plan.window_count,
    };
    out.write_all(&catalog.encode())?;

    let file = out.into_inner().map_err(io::IntoInnerError::into_error)?;
    let bytes = file.metadata()?.len();
    file.sync_all()?;
    Ok(WriteSummary {
        sections,
        bytes,
        min_ts: plan.min_ts,
        max_ts: plan.max_ts,
    })
}

fn plan_segment(journal: &Journal) -> Result<SegmentPlan, WriteError> {
    let mut by_type = BTreeMap::<u32, Vec<SectionDescriptor>>::new();
    let mut section_count = 0_usize;
    let mut min_ts = i64::MAX;
    let mut max_ts = i64::MIN;
    let window_count =
        u32::try_from(journal.parts().len()).map_err(|_error| WriteError::ArithmeticOverflow {
            what: "window count",
        })?;
    for &part_ref in journal.parts() {
        let part = journal.read_part(part_ref)?;
        // Recheck bodies immediately before publication. The journal may have
        // changed on disk after append even though its frame remained valid.
        let catalog = validate_part_catalog(&part).map_err(WriteError::Part)?;
        if catalog.format_version != FORMAT_VERSION {
            return Err(WriteError::UnsupportedFormat {
                version: catalog.format_version,
            });
        }
        min_ts = min_ts.min(catalog.min_ts);
        max_ts = max_ts.max(catalog.max_ts);
        for entry in catalog.entries {
            section_count = section_count
                .checked_add(1)
                .ok_or(WriteError::ArithmeticOverflow {
                    what: "section descriptor count",
                })?;
            if section_count > MAX_SECTION_ROWS {
                return Err(WriteError::TooManySections {
                    sections: section_count,
                    max: MAX_SECTION_ROWS,
                });
            }
            let descriptors = by_type.entry(entry.type_id).or_default();
            descriptors
                .try_reserve(1)
                .map_err(WriteError::CatalogAllocation)?;
            descriptors.push(SectionDescriptor {
                part: part_ref,
                entry,
            });
        }
    }
    if min_ts > max_ts {
        min_ts = 0;
        max_ts = 0;
    }
    Ok(SegmentPlan {
        by_type,
        min_ts,
        max_ts,
        window_count,
    })
}

fn aggregate_rows(type_id: u32, descriptors: &[SectionDescriptor]) -> Result<usize, WriteError> {
    let rows = descriptors.iter().try_fold(0_usize, |rows, descriptor| {
        rows.checked_add(descriptor.entry.rows as usize)
            .ok_or(WriteError::ArithmeticOverflow {
                what: "section row count",
            })
    })?;
    if rows > MAX_SECTION_ROWS {
        return Err(CodecError::TooManyRows {
            rows,
            max: MAX_SECTION_ROWS,
        }
        .into());
    }
    if type_id == 0 {
        return Err(CodecError::UnknownType { type_id }.into());
    }
    Ok(rows)
}

fn read_verified_body(
    journal: &Journal,
    descriptor: SectionDescriptor,
) -> Result<VerifiedSection, WriteError> {
    let start = usize::try_from(descriptor.entry.offset).map_err(|_error| {
        WriteError::ArithmeticOverflow {
            what: "section offset",
        }
    })?;
    let len =
        usize::try_from(descriptor.entry.len).map_err(|_error| WriteError::ArithmeticOverflow {
            what: "section length",
        })?;
    let body = journal.read_part_range(descriptor.part, start, len)?;
    VerifiedSection::verify(Bytes::from(body), descriptor.entry.crc32c, crc32c)
        .map_err(WriteError::Codec)
}

fn write_section(
    out: &mut impl Write,
    entries: &mut Vec<Entry>,
    offset: &mut u64,
    type_id: u32,
    rows: u32,
    body: &[u8],
) -> Result<(), WriteError> {
    if entries.last().is_some_and(|entry| entry.type_id >= type_id) {
        return Err(CodecError::SchemaMismatch.into());
    }
    checked_catalog_entries(entries.len(), 1)?;
    entries
        .try_reserve(1)
        .map_err(WriteError::CatalogAllocation)?;
    let len = u64::try_from(body.len()).map_err(|_error| WriteError::ArithmeticOverflow {
        what: "section length",
    })?;
    out.write_all(body)?;
    entries.push(Entry {
        type_id,
        flags: 0,
        offset: *offset,
        len,
        rows,
        crc32c: crc32c(body),
    });
    *offset = offset
        .checked_add(len)
        .ok_or(WriteError::ArithmeticOverflow {
            what: "segment offset",
        })?;
    Ok(())
}

#[cfg(test)]
mod tests;
