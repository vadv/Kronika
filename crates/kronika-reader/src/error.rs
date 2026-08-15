//! What can go wrong between a directory on disk and a row.

use std::fmt;

use kronika_registry::CodecError;
use kronika_store::StoreError;

/// Why a read failed.
#[derive(Debug)]
pub enum ReaderError {
    /// The directory or a file in it could not be read.
    Io(std::io::Error),
    /// The segment's container framing was rejected.
    Store(StoreError),
    /// A section body failed its checksum or could not be decoded.
    Section {
        /// Section type that failed.
        type_id: u32,
        /// What the codec said.
        source: CodecError,
    },
}

impl ReaderError {
    /// Whether reopening the data root may select a source that replaced this one.
    #[must_use]
    pub fn source_changed_during_read(&self) -> bool {
        match self {
            Self::Io(error) => error.kind() == std::io::ErrorKind::Interrupted,
            Self::Store(StoreError::Io(error)) => matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::UnexpectedEof
            ),
            Self::Store(_) | Self::Section { .. } => false,
        }
    }
}

impl fmt::Display for ReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "read the data directory: {error}"),
            Self::Store(error) => write!(f, "open the segment: {error}"),
            Self::Section { type_id, source } => write!(f, "decode section {type_id}: {source}"),
        }
    }
}

impl std::error::Error for ReaderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::Section { source, .. } => Some(source),
        }
    }
}

impl From<std::io::Error> for ReaderError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<StoreError> for ReaderError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}
