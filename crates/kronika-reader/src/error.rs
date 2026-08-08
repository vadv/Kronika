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
    /// The string dictionary section could not be read as Parquet.
    Dictionary(parquet::errors::ParquetError),
    /// The string dictionary decoded, but without the columns a dictionary has.
    DictionaryShape(&'static str),
}

impl fmt::Display for ReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "read the data directory: {error}"),
            Self::Store(error) => write!(f, "open the segment: {error}"),
            Self::Section { type_id, source } => write!(f, "decode section {type_id}: {source}"),
            Self::Dictionary(error) => write!(f, "read the string dictionary: {error}"),
            Self::DictionaryShape(what) => write!(f, "the string dictionary has {what}"),
        }
    }
}

impl std::error::Error for ReaderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::Section { source, .. } => Some(source),
            Self::Dictionary(error) => Some(error),
            Self::DictionaryShape(_what) => None,
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
