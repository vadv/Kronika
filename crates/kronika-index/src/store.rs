//! Validated targeted reads and atomic publication beside immutable segments.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use crate::build::{BuildError, build, build_selected};
use crate::file::{Index, IndexError, TargetedIndex, read_all, read_target};
use crate::series::{SeriesKey, SeriesKind, pg_activity_layout, pg_database_layout};
use kronika_layout::{DataRoot, LayoutError, LayoutLimits, OwnerKind, SegmentAddress, SegmentId};
use kronika_reader::{Reader, ReaderError, SegmentKind, SegmentRef};

/// Extension of an index file.
pub const EXTENSION: &str = "idx";
const SEGMENT_EXTENSION: &str = "zms";

/// A validated resource and whether it came from an immutable sidecar.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceIndex {
    /// Selected blocks.
    pub index: TargetedIndex,
    /// `true` only for an immutable persisted IDX.
    pub persisted: bool,
}

/// Why a targeted index could not be read or rebuilt.
#[derive(Debug)]
pub enum LoadError {
    /// Ordinary file I/O failed.
    Io(io::Error),
    /// The index container or selected block was invalid.
    Bad(IndexError),
    /// Typed data-root access or publication failed.
    Layout(LayoutError),
    /// The production segment reader failed.
    Reader(ReaderError),
    /// Exact summary construction failed.
    Build(BuildError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Bad(error) => error.fmt(f),
            Self::Layout(error) => error.fmt(f),
            Self::Reader(error) => error.fmt(f),
            Self::Build(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Bad(error) => Some(error),
            Self::Layout(error) => Some(error),
            Self::Reader(error) => Some(error),
            Self::Build(error) => Some(error),
        }
    }
}

impl From<io::Error> for LoadError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<LayoutError> for LoadError {
    fn from(error: LayoutError) -> Self {
        Self::Layout(error)
    }
}

impl From<ReaderError> for LoadError {
    fn from(error: ReaderError) -> Self {
        Self::Reader(error)
    }
}

impl From<BuildError> for LoadError {
    fn from(error: BuildError) -> Self {
        Self::Build(error)
    }
}

/// Where the index of a finished segment lives, for diagnostics and tests.
#[must_use]
pub fn path_of(segment: &Path) -> Option<PathBuf> {
    if segment.extension()? != SEGMENT_EXTENSION {
        return None;
    }
    Some(segment.with_extension(EXTENSION))
}

/// Decode a complete index file, primarily for diagnostics and tests.
///
/// # Errors
///
/// Returns I/O or container validation failures.
pub fn read(path: &Path) -> Result<Index, LoadError> {
    let mut file = std::fs::File::open(path).map_err(LoadError::Io)?;
    read_all(&mut file).map_err(LoadError::Bad)
}

/// Load one logical index resource, rebuilding derived data when necessary.
///
/// Finished resources use the atomically published sibling IDX. Active
/// resources are computed only for the captured committed prefix and are never
/// persisted.
///
/// # Errors
///
/// Returns reader, index, layout, build, or publication failures.
pub fn resource(
    root: &Path,
    reader: &Reader,
    segment_ref: &SegmentRef,
    logical_name: &str,
) -> Result<ResourceIndex, LoadError> {
    let keys = series_keys(segment_ref, logical_name);
    match segment_ref.kind() {
        SegmentKind::Active => {
            let segment = reader.open_segment(segment_ref)?;
            let index = build_selected(&segment, &keys)?;
            Ok(ResourceIndex {
                index: targeted(index, &keys, None),
                persisted: false,
            })
        }
        SegmentKind::Finished => {
            let data_root = DataRoot::open(root)?;
            let address = address_of(segment_ref.id())?;
            if let Some(mut file) = data_root.open_idx(address)?
                && let Ok(selected) = read_target(&mut file, &keys)
                && contains_targets(&selected, &keys)
            {
                return Ok(ResourceIndex {
                    index: selected,
                    persisted: true,
                });
            }

            match data_root.acquire_index(LayoutLimits::default()) {
                Ok(owner) => {
                    // Capture the immutable source identity before opening it
                    // through the reader. Publication revalidates this exact
                    // identity after the complete build.
                    let mut temporary = owner.create_idx_temp(address)?;
                    let segment = reader.open_segment(segment_ref)?;
                    let index = build(&segment)?;
                    let bytes = index.encode().map_err(LoadError::Bad)?;
                    let checksum = encoded_checksum(&bytes)?;
                    temporary.file_mut().write_all(&bytes)?;
                    drop(temporary.try_clone_file()?);
                    temporary.publish()?;
                    Ok(ResourceIndex {
                        index: targeted(index, &keys, Some(checksum)),
                        persisted: true,
                    })
                }
                Err(LayoutError::OwnerContended {
                    owner: OwnerKind::Index,
                }) => {
                    // Another cold request may be publishing the same
                    // canonical bytes. Serving a locally validated build keeps
                    // lock contention out of the HTTP failure surface; its
                    // full-file checksum is the same stable representation
                    // tag the winning publisher computes.
                    let segment = reader.open_segment(segment_ref)?;
                    let index = build(&segment)?;
                    let bytes = index.encode().map_err(LoadError::Bad)?;
                    let checksum = encoded_checksum(&bytes)?;
                    Ok(ResourceIndex {
                        index: targeted(index, &keys, Some(checksum)),
                        persisted: false,
                    })
                }
                Err(error) => Err(LoadError::Layout(error)),
            }
        }
    }
}

/// Return the allowlisted series exposed by one logical section.
#[must_use]
pub fn series_keys(segment: &SegmentRef, logical_name: &str) -> Vec<SeriesKey> {
    if logical_name == "health" {
        return vec![
            SeriesKey::OS_HEALTH,
            SeriesKey::OVERALL_HEALTH,
            SeriesKey::POSTGRES_HEALTH,
            SeriesKey::PGBOUNCER_HEALTH,
        ];
    }
    segment
        .sections()
        .iter()
        .filter_map(|section| match logical_name {
            "pg_stat_database" if pg_database_layout(section.type_id) => Some(SeriesKey {
                kind: SeriesKind::PgTransactionsPerSecond,
                type_id: section.type_id,
            }),
            "pg_stat_activity" if pg_activity_layout(section.type_id) => Some(SeriesKey {
                kind: SeriesKind::PgActiveBackends,
                type_id: section.type_id,
            }),
            _ => None,
        })
        .collect()
}

fn contains_targets(index: &TargetedIndex, keys: &[SeriesKey]) -> bool {
    keys.iter()
        .filter(|key| {
            !matches!(
                key.kind,
                SeriesKind::PostgresHealth | SeriesKind::PgbouncerHealth
            )
        })
        .all(|key| index.blocks.iter().any(|block| block.key() == *key))
}

fn address_of(raw_id: i64) -> Result<SegmentAddress, LayoutError> {
    SegmentAddress::new(SegmentId::new(raw_id)?)
}

fn targeted(index: Index, keys: &[SeriesKey], checksum: Option<u32>) -> TargetedIndex {
    let wanted: std::collections::HashSet<SeriesKey> = keys.iter().copied().collect();
    TargetedIndex {
        checksum,
        blocks: index
            .blocks
            .into_iter()
            .filter(|block| wanted.contains(&block.key()))
            .collect(),
    }
}

fn encoded_checksum(bytes: &[u8]) -> Result<u32, LoadError> {
    let raw: [u8; 4] = bytes
        .get(12..16)
        .ok_or(LoadError::Bad(IndexError::Truncated))?
        .try_into()
        .map_err(|_error| LoadError::Bad(IndexError::Truncated))?;
    Ok(u32::from_le_bytes(raw))
}

#[cfg(test)]
mod tests;
