//! The `.idx` file: health points, and the checksum a browser revalidates on.

use kronika_format::crc32c;

/// Magic of an index file.
pub const MAGIC: [u8; 8] = *b"KRNIDX1\0";

/// Format version. A file written by another version is deleted and rebuilt,
/// never migrated.
pub const FORMAT_VERSION: u32 = 1;

/// Bytes before the first point: magic, version, sources, checksum.
///
/// The point count is not stored. It follows from the body length, and a
/// second statement of the same number could only ever disagree with the first.
pub const HEADER_LEN: usize = 20;

/// Bytes per point: the timestamp and its health.
pub const POINT_LEN: usize = 9;

/// Health of a point that could not be computed.
const NO_HEALTH: u8 = 0xFF;

/// Health at one snapshot, `None` where the interval before it gave nothing to
/// divide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    /// Snapshot timestamp, unix microseconds.
    pub ts: i64,
    /// `0` to `100`.
    pub health: Option<u8>,
}

/// A decoded index file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    /// Sources enabled when the file was built. A different set is a different
    /// file, and web rebuilds it.
    pub sources: u32,
    /// Health over the segment, oldest first.
    pub points: Vec<Point>,
}

/// Why an index file was rejected.
///
/// Every one of these means the same thing to a caller: delete the file and
/// build it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexError {
    /// Shorter than a header, or the points do not fill the rest.
    Truncated,
    /// The first eight bytes are not [`MAGIC`].
    BadMagic,
    /// Written by a version this build does not read.
    UnsupportedVersion(u32),
    /// The points do not match the checksum in the header.
    BadChecksum,
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "the index file is truncated"),
            Self::BadMagic => write!(f, "the index file does not start with its magic"),
            Self::UnsupportedVersion(version) => {
                write!(
                    f,
                    "the index file is version {version}, not {FORMAT_VERSION}"
                )
            }
            Self::BadChecksum => write!(f, "the index file does not match its checksum"),
        }
    }
}

impl std::error::Error for IndexError {}

impl Index {
    /// Encode the file.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut points = Vec::with_capacity(self.points.len() * POINT_LEN);
        for point in &self.points {
            points.extend_from_slice(&point.ts.to_le_bytes());
            points.push(point.health.unwrap_or(NO_HEALTH));
        }
        let mut bytes = Vec::with_capacity(HEADER_LEN + points.len());
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&self.sources.to_le_bytes());
        bytes.extend_from_slice(&crc32c(&points).to_le_bytes());
        bytes.extend_from_slice(&points);
        bytes
    }

    /// Decode a file.
    ///
    /// # Errors
    ///
    /// Returns the reason the file is unusable. Every reason is answered the
    /// same way: build it again.
    pub fn decode(bytes: &[u8]) -> Result<Self, IndexError> {
        let header = bytes.get(..HEADER_LEN).ok_or(IndexError::Truncated)?;
        if header[..8] != MAGIC {
            return Err(IndexError::BadMagic);
        }
        let version = u32_at(header, 8);
        if version != FORMAT_VERSION {
            return Err(IndexError::UnsupportedVersion(version));
        }
        let sources = u32_at(header, 12);
        let expected_crc = u32_at(header, 16);
        let body = bytes.get(HEADER_LEN..).ok_or(IndexError::Truncated)?;
        if body.len() % POINT_LEN != 0 {
            return Err(IndexError::Truncated);
        }
        if crc32c(body) != expected_crc {
            return Err(IndexError::BadChecksum);
        }
        let points = body
            .chunks_exact(POINT_LEN)
            .map(|chunk| Point {
                ts: i64::from_le_bytes(
                    chunk[..8]
                        .try_into()
                        .unwrap_or_else(|_never| [0; 8].to_owned()),
                ),
                health: Some(chunk[8]).filter(|health| *health != NO_HEALTH),
            })
            .collect();
        Ok(Self { sources, points })
    }

    /// The checksum the header carries, without decoding the points.
    ///
    /// This is what a browser revalidates against, so it has to be readable
    /// from the first bytes of the file.
    ///
    /// # Errors
    ///
    /// Returns the reason the header is unusable.
    pub fn checksum_of(bytes: &[u8]) -> Result<u32, IndexError> {
        let header = bytes.get(..HEADER_LEN).ok_or(IndexError::Truncated)?;
        if header[..8] != MAGIC {
            return Err(IndexError::BadMagic);
        }
        let version = u32_at(header, 8);
        if version != FORMAT_VERSION {
            return Err(IndexError::UnsupportedVersion(version));
        }
        Ok(u32_at(header, 16))
    }
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    let mut raw = [0_u8; 4];
    raw.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(raw)
}

#[cfg(test)]
mod tests;
