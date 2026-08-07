//! Per-type row buffers before a journal part is written.

use std::any::Any;
use std::collections::BTreeMap;

use kronika_format::{PartMeta, SectionInput, build_part};
use kronika_registry::{CodecError, MAX_SECTION_ROWS, Section};

/// Buffered rows for one section type.
trait TypeBuffer: Any {
    fn section_type_id(&self) -> u32;
    fn is_empty(&self) -> bool;
    /// Encode the buffered rows to a section body, its row count, and ts range.
    fn encode(&self) -> Result<EncodedRows, CodecError>;
    fn clear(&mut self);
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// One encoded section: the Parquet body plus the catalog fields derived from it.
struct EncodedRows {
    body: Vec<u8>,
    rows: u32,
    ts_range: Option<(i64, i64)>,
    list_i32_child_value_count: usize,
}

/// Encoded bytes and row count for one section in a flushed journal part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionFlushSummary {
    /// Section `type_id`.
    pub type_id: u32,
    /// Rows encoded into this section.
    pub rows: u32,
    /// Encoded section body bytes.
    pub body_bytes: usize,
    /// Child values across all `ListI32` columns in this section.
    pub list_i32_child_value_count: usize,
}

/// Accounting for one flushed collection window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlushSummary {
    /// One entry per section written into the part.
    pub sections: Vec<SectionFlushSummary>,
    /// Total ZMS part bytes appended to the journal frame body.
    pub part_bytes: usize,
}

/// A flushed journal part plus its section-level accounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlushedPart {
    /// ZMS part body ready for `Journal::append`.
    pub body: Vec<u8>,
    /// Section and byte counts for logs.
    pub summary: FlushSummary,
}

struct RowBuffer<T: Section> {
    rows: Vec<T>,
}

impl<T: Section + 'static> TypeBuffer for RowBuffer<T> {
    fn section_type_id(&self) -> u32 {
        T::CONTRACT.type_id.get()
    }

    fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn encode(&self) -> Result<EncodedRows, CodecError> {
        let list_i32_child_value_count = T::list_i32_child_value_count(&self.rows);
        let body = T::encode(&self.rows)?;
        // `encode` already enforced the row cap; the catalog row field is `u32`.
        let rows = u32::try_from(self.rows.len()).unwrap_or(u32::MAX);
        Ok(EncodedRows {
            body,
            rows,
            ts_range: T::ts_range(&self.rows),
            list_i32_child_value_count,
        })
    }

    fn clear(&mut self) {
        self.rows.clear();
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// The writer's collection window: typed rows buffered per section type until a
/// flush turns them into one ZMS part.
#[derive(Default)]
pub struct SectionBuffers {
    by_type: BTreeMap<u32, Box<dyn TypeBuffer>>,
}

impl std::fmt::Debug for SectionBuffers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SectionBuffers")
            .field("type_ids", &self.by_type.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl SectionBuffers {
    /// An empty set of buffers.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            by_type: BTreeMap::new(),
        }
    }

    /// Buffer one row of section type `T`.
    ///
    /// # Errors
    ///
    /// Returns the input row when this type reached the row cap.
    ///
    /// # Panics
    ///
    /// Panics if two `Section` types use the same `type_id`.
    pub fn push<T: Section + 'static>(&mut self, row: T) -> Result<(), T> {
        let type_id = T::CONTRACT.type_id.get();
        let buffer = self
            .by_type
            .entry(type_id)
            .or_insert_with(|| Box::new(RowBuffer::<T> { rows: Vec::new() }));
        let rows = &mut buffer
            .as_any_mut()
            .downcast_mut::<RowBuffer<T>>()
            .expect("a type_id maps to exactly one Section type")
            .rows;
        if rows.len() >= MAX_SECTION_ROWS {
            return Err(row);
        }
        rows.push(row);
        Ok(())
    }

    /// Whether no rows are buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_type.values().all(|buffer| buffer.is_empty())
    }

    /// Encode buffered rows and dictionary sections into one ZMS part.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] when section encoding or part assembly fails.
    pub fn flush(
        &mut self,
        dict_sections: &[crate::dict::DictSection],
    ) -> Result<Option<Vec<u8>>, CodecError> {
        Ok(self
            .flush_with_summary(dict_sections)?
            .map(|flushed| flushed.body))
    }

    /// Encode buffered rows and dictionary sections into one ZMS part.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] when section encoding or part assembly fails.
    pub fn flush_with_summary(
        &mut self,
        dict_sections: &[crate::dict::DictSection],
    ) -> Result<Option<FlushedPart>, CodecError> {
        let encoded: Vec<(u32, EncodedRows)> = self
            .by_type
            .values()
            .filter(|buffer| !buffer.is_empty())
            .map(|buffer| Ok((buffer.section_type_id(), buffer.encode()?)))
            .collect::<Result<_, CodecError>>()?;
        if encoded.is_empty() && dict_sections.is_empty() {
            return Ok(None);
        }

        // Dictionary-only parts use an empty interval; `seal` ignores it while
        // folding the segment range.
        let lo = encoded
            .iter()
            .filter_map(|(_, section)| section.ts_range.map(|(lo, _)| lo))
            .min();
        let hi = encoded
            .iter()
            .filter_map(|(_, section)| section.ts_range.map(|(_, hi)| hi))
            .max();
        let (min_ts, max_ts) = match (lo, hi) {
            (Some(lo), Some(hi)) => (lo, hi),
            _ => (i64::MAX, i64::MIN),
        };

        let mut sections: Vec<SectionInput<'_>> = encoded
            .iter()
            .map(|(type_id, section)| SectionInput {
                type_id: *type_id,
                rows: section.rows,
                body: &section.body,
            })
            .collect();
        sections.extend(dict_sections.iter().map(|dict| SectionInput {
            type_id: dict.type_id,
            rows: dict.rows,
            body: &dict.body,
        }));
        let part = build_part(&sections, PartMeta { min_ts, max_ts });
        let mut summary_sections = Vec::with_capacity(sections.len());
        for (type_id, section) in &encoded {
            summary_sections.push(SectionFlushSummary {
                type_id: *type_id,
                rows: section.rows,
                body_bytes: section.body.len(),
                list_i32_child_value_count: section.list_i32_child_value_count,
            });
        }
        for section in dict_sections {
            summary_sections.push(SectionFlushSummary {
                type_id: section.type_id,
                rows: section.rows,
                body_bytes: section.body.len(),
                list_i32_child_value_count: 0,
            });
        }
        let summary = FlushSummary {
            sections: summary_sections,
            part_bytes: part.len(),
        };

        for buffer in self.by_type.values_mut() {
            buffer.clear();
        }
        Ok(Some(FlushedPart {
            body: part,
            summary,
        }))
    }
}

#[cfg(test)]
mod tests {
    use kronika_format::{crc32c, validate_part};
    use kronika_registry::instance_metadata::{Environment, InstanceMetadata};
    use kronika_registry::os_loadavg::OsLoadavg;
    use kronika_registry::{Bytes, MAX_SECTION_ROWS, StrId, Ts, VerifiedSection, decode_any};

    use super::SectionBuffers;

    fn loadavg(ts: i64) -> OsLoadavg {
        OsLoadavg {
            ts: Ts(ts),
            load1: 1.5,
            load5: 1.0,
            load15: 0.5,
            running: 2,
            total: 345,
            scope: 0,
        }
    }

    fn instance(ts: i64) -> InstanceMetadata {
        InstanceMetadata {
            ts: Ts(ts),
            hostname: StrId(1),
            kernel_version: StrId(3),
            environment: Environment::Machine.as_u8(),
            clock_ticks_per_sec: 100,
            page_size_bytes: 4096,
            boot_id: StrId(4),
            btime: Ts(ts - 1_000),
        }
    }

    #[test]
    fn buffers_many_types_and_flushes_one_part() {
        let mut buffers = SectionBuffers::new();
        assert!(buffers.is_empty());
        buffers.push(loadavg(1_000)).expect("buffer not full");
        buffers.push(loadavg(2_000)).expect("buffer not full");
        buffers.push(instance(1_500)).expect("buffer not full");
        assert!(!buffers.is_empty());

        let part = buffers
            .flush(&[])
            .expect("flush encodes the buffered rows")
            .expect("buffered rows produce a part");
        assert!(buffers.is_empty(), "flush clears the window");

        let catalog = validate_part(&part).expect("the part is a valid container");
        assert_eq!(catalog.entries.len(), 2, "one section per buffered type");
        assert_eq!(
            (catalog.min_ts, catalog.max_ts),
            (1_000, 2_000),
            "time range spans both loadavg rows"
        );

        // Decode through the registry with the production CRC function.
        let decode_rows = |type_id: u32| -> usize {
            let entry = *catalog
                .entries
                .iter()
                .find(|entry| entry.type_id == type_id)
                .expect("the type's entry is present");
            let start = usize::try_from(entry.offset).expect("offset fits usize");
            let len = usize::try_from(entry.len).expect("len fits usize");
            let body = Bytes::copy_from_slice(&part[start..start + len]);
            let verified =
                VerifiedSection::verify(body, entry.crc32c, crc32c).expect("catalog crc matches");
            decode_any(type_id, verified).expect("decode").stats.rows
        };
        assert_eq!(decode_rows(1_105_001), 2);
        assert_eq!(decode_rows(1_021_001), 1);
    }

    #[test]
    fn flush_with_summary_reports_section_rows_and_bytes() {
        let mut buffers = SectionBuffers::new();
        buffers.push(loadavg(1_000)).expect("buffer not full");
        buffers.push(loadavg(2_000)).expect("buffer not full");
        buffers.push(instance(1_500)).expect("buffer not full");

        let flushed = buffers
            .flush_with_summary(&[])
            .expect("flush encodes the buffered rows")
            .expect("buffered rows produce a part");
        assert_eq!(flushed.summary.part_bytes, flushed.body.len());
        assert_eq!(flushed.summary.sections.len(), 2);
        let loadavg = flushed
            .summary
            .sections
            .iter()
            .find(|section| section.type_id == 1_105_001)
            .expect("loadavg section summary");
        assert_eq!(loadavg.rows, 2);
        assert!(loadavg.body_bytes > 0);
        assert_eq!(loadavg.list_i32_child_value_count, 0);
        let instance = flushed
            .summary
            .sections
            .iter()
            .find(|section| section.type_id == 1_021_001)
            .expect("instance section summary");
        assert_eq!(instance.rows, 1);
        assert!(instance.body_bytes > 0);
        assert_eq!(instance.list_i32_child_value_count, 0);
        assert!(buffers.is_empty(), "flush clears the window");
    }

    #[test]
    fn flushing_an_empty_window_yields_no_part() {
        let mut buffers = SectionBuffers::new();
        assert!(buffers.flush(&[]).expect("flush ok").is_none());
    }

    #[test]
    fn flush_summary_includes_dictionary_sections() {
        let mut buffers = SectionBuffers::new();
        let dict_sections = [crate::dict::DictSection {
            type_id: kronika_registry::DICT_STRINGS_TYPE_ID,
            rows: 3,
            body: vec![1, 2, 3, 4],
        }];

        let flushed = buffers
            .flush_with_summary(&dict_sections)
            .expect("flush ok")
            .expect("dictionary-only part is still written");
        assert_eq!(flushed.summary.sections.len(), 1);
        assert_eq!(
            flushed.summary.sections[0],
            super::SectionFlushSummary {
                type_id: kronika_registry::DICT_STRINGS_TYPE_ID,
                rows: 3,
                body_bytes: 4,
                list_i32_child_value_count: 0,
            }
        );
        assert_eq!(flushed.summary.part_bytes, flushed.body.len());
    }

    #[test]
    fn push_bounces_a_row_when_the_type_buffer_is_full() {
        let mut buffers = SectionBuffers::new();
        for _ in 0..MAX_SECTION_ROWS {
            buffers.push(loadavg(0)).expect("under the cap");
        }
        // A full buffer holds one section's worth; the next row comes back for
        // the caller to flush and retry, so memory stays bounded before a flush.
        assert!(buffers.push(loadavg(0)).is_err());
    }
}
