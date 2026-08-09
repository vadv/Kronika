//! Building an index from a segment, and the file it lives in beside it.
//!
//! Nothing here decides when to rebuild. A caller reads, and on any answer
//! other than an index it builds one and writes it: an `.idx` is derived from
//! the segment next to it, so there is one answer to every way it can be
//! unusable.

use std::io;
use std::path::{Path, PathBuf};

use kronika_reader::{ReaderError, Resolved, Segment};
use kronika_registry::{DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID};

use crate::build::{INSTANCE_METADATA_TYPE_ID, OS_PSI_TYPE_ID, objects, points};
use crate::file::{Index, IndexError};

/// Extension of an index file.
pub const EXTENSION: &str = "idx";

/// Why an index file could not be read.
#[allow(
    variant_size_differences,
    reason = "an io::Error is wider than the codec's reason, and both are returned by the same call"
)]
#[derive(Debug)]
pub enum LoadError {
    /// The file could not be opened or read.
    Io(io::Error),
    /// The bytes are not an index this version reads.
    Bad(IndexError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Bad(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Extension of a finished segment.
const SEGMENT_EXTENSION: &str = "zms";

/// Where the index of `segment` lives: the same path with the extension
/// replaced, so the two are found together and deleted together.
///
/// `None` for the current segment. It grows with every snapshot, so a file
/// written for it would be rewritten by the next request and describe a
/// segment that no longer ends where it did. Its points are computed for the
/// answer instead.
#[must_use]
pub fn path_of(segment: &Path) -> Option<PathBuf> {
    if segment.extension()? != SEGMENT_EXTENSION {
        return None;
    }
    Some(segment.with_extension(EXTENSION))
}

/// Read an index file.
///
/// # Errors
///
/// Returns why the file could not be used. Every reason is answered the same
/// way: build it again.
pub fn read(path: &Path) -> Result<Index, LoadError> {
    let bytes = std::fs::read(path).map_err(LoadError::Io)?;
    Index::decode(&bytes).map_err(LoadError::Bad)
}

/// Write an index file, replacing whatever was there.
///
/// The bytes land in a temporary beside the target and are renamed onto it, so
/// a reader either sees the file that was there or the whole new one.
///
/// # Errors
///
/// Returns the write error. The index is derived data: a caller that cannot
/// write it has a slower next request, not a lost one.
pub fn write(path: &Path, index: &Index) -> io::Result<()> {
    let temporary = path.with_extension("idx.tmp");
    std::fs::write(&temporary, index.encode())?;
    std::fs::rename(&temporary, path)
}

/// Build the index of one segment.
///
/// # Errors
///
/// Returns the reader's error when a section or the dictionary cannot be
/// decoded.
pub fn build(segment: &Segment, sources: u32) -> Result<Index, ReaderError> {
    let dictionary = segment.dictionary()?;
    let resolve = |id: u64| {
        dictionary.resolve(id).map(|entry| match entry {
            Resolved::Str(bytes) => String::from_utf8_lossy(bytes).into_owned(),
            Resolved::Blob(blob) => String::from_utf8_lossy(blob.stored_bytes).into_owned(),
        })
    };
    let points = points(
        &segment.rows(INSTANCE_METADATA_TYPE_ID)?,
        &segment.rows(OS_PSI_TYPE_ID)?,
    );
    let type_ids: Vec<u32> = segment
        .type_ids()
        .filter(|id| !is_dictionary(*id))
        .collect();
    let mut sections = Vec::new();
    for type_id in type_ids {
        if let Some(section) = objects(&segment.rows(type_id)?, resolve) {
            sections.push(section);
        }
    }
    Ok(Index {
        sources,
        points,
        objects: sections,
    })
}

/// Whether a section is one of the segment's dictionaries.
///
/// A dictionary is how the other sections store their strings, not a section
/// of rows, and no registry contract decodes one.
const fn is_dictionary(type_id: u32) -> bool {
    matches!(type_id, DICT_STRINGS_TYPE_ID | DICT_BLOBS_TYPE_ID)
}

#[cfg(test)]
mod tests;
