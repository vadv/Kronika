//! One finished segment, opened.

use std::fs::File;
use std::path::{Path, PathBuf};

use kronika_format::{Catalog, ReadAt as _, crc32c};
use kronika_registry::{Bytes, DICT_STRINGS_TYPE_ID, Row, VerifiedSection, decode_rows};
use kronika_store::{FinalUnit, LocalDir, read_catalog};

use crate::error::ReaderError;
use crate::strings::Strings;

/// An open `.zms`, holding its file descriptor and decoded catalog.
///
/// Section bodies are read on demand and dropped with the caller's value, so
/// an idle holder costs one descriptor and the catalog.
#[derive(Debug)]
pub struct Segment {
    path: PathBuf,
    file: File,
    catalog: Catalog,
}

impl Segment {
    /// Open the file behind `unit` and decode its catalog.
    ///
    /// # Errors
    ///
    /// Returns the store error of opening the file or rejecting its catalog.
    pub(crate) fn open(dir: &LocalDir, root: &Path, unit: &FinalUnit) -> Result<Self, ReaderError> {
        let file = dir.open_finished(unit)?;
        let catalog = read_catalog(&file)?;
        // The catalog was read positionally; checking identity after it means a
        // file rewritten mid-read is an error rather than a decoded mixture.
        dir.validate_finished_file(&file, unit)?;
        Ok(Self {
            path: segment_path(root, unit),
            file,
            catalog,
        })
    }

    /// Where the segment was read from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Earliest timestamp the segment carries, unix microseconds.
    #[must_use]
    pub const fn min_ts(&self) -> i64 {
        self.catalog.min_ts
    }

    /// Latest timestamp the segment carries, unix microseconds.
    #[must_use]
    pub const fn max_ts(&self) -> i64 {
        self.catalog.max_ts
    }

    /// Collection windows coalesced into the segment; `0` when unknown.
    #[must_use]
    pub const fn window_count(&self) -> u32 {
        self.catalog.window_count
    }

    /// Section types present in the segment, in catalog order.
    pub fn type_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.catalog.entries.iter().map(|entry| entry.type_id)
    }

    /// Rows recorded for `type_id`, or `None` when the section is absent.
    ///
    /// Absent and empty are different answers: a section the collector wrote
    /// with nothing in it reports `Some(0)`.
    #[must_use]
    pub fn rows_of(&self, type_id: u32) -> Option<u32> {
        self.catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == type_id)
            .map(|entry| entry.rows)
    }

    /// Decode a section into column-addressable rows.
    ///
    /// An absent section decodes to no rows, so a caller sweeping many types
    /// does not branch on presence.
    ///
    /// # Errors
    ///
    /// Returns an error when the body fails its checksum or the codec rejects
    /// it.
    pub fn rows(&self, type_id: u32) -> Result<Vec<Row>, ReaderError> {
        let Some((body, crc)) = self.body(type_id)? else {
            return Ok(Vec::new());
        };
        let verified = VerifiedSection::verify(body, crc, crc32c)
            .map_err(|source| ReaderError::Section { type_id, source })?;
        decode_rows(type_id, verified).map_err(|source| ReaderError::Section { type_id, source })
    }

    /// The segment's string dictionary, empty when it interned nothing.
    ///
    /// # Errors
    ///
    /// Returns an error when the dictionary section cannot be read.
    pub fn strings(&self) -> Result<Strings, ReaderError> {
        let Some((body, crc)) = self.body(DICT_STRINGS_TYPE_ID)? else {
            return Ok(Strings::default());
        };
        let verified =
            VerifiedSection::verify(body, crc, crc32c).map_err(|source| ReaderError::Section {
                type_id: DICT_STRINGS_TYPE_ID,
                source,
            })?;
        Strings::decode(verified.into_bytes())
    }

    /// Read one section body and its recorded checksum, or `None` when the
    /// catalog has no such section.
    fn body(&self, type_id: u32) -> Result<Option<(Bytes, u32)>, ReaderError> {
        let Some(entry) = self
            .catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == type_id)
        else {
            return Ok(None);
        };
        let len = usize::try_from(entry.len).map_err(|_e| {
            ReaderError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("section {type_id} declares {} bytes", entry.len),
            ))
        })?;
        let mut buf = vec![0_u8; len];
        self.file.read_exact_at(&mut buf, entry.offset)?;
        Ok(Some((Bytes::from(buf), entry.crc32c)))
    }
}

/// `YYYY/MM/DD/N.zms` under the root, the path the collector wrote to.
fn segment_path(root: &Path, unit: &FinalUnit) -> PathBuf {
    let day = unit.address.day;
    root.join(day.year_component())
        .join(day.month_component())
        .join(day.day_component())
        .join(unit.address.zms_name())
}
