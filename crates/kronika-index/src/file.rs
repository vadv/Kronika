//! The `.idx` file: health points, and the checksum a browser revalidates on.

use kronika_format::Crc32c;

/// Magic of an index file.
pub const MAGIC: [u8; 8] = *b"KRNIDX1\0";

/// Bytes before the first point: magic, sources, checksum.
///
/// The point count is not stored. It follows from the body length, and a
/// second statement of the same number could only ever disagree with the first.
pub const HEADER_LEN: usize = 16;

/// Bytes per point: the timestamp and its health.
pub const POINT_LEN: usize = 9;

/// Health of a point that could not be computed.
const NO_HEALTH: u8 = 0xFF;

/// The checksum starts here and is the only file field it does not cover.
const CHECKSUM_AT: usize = 12;

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
    /// The magic, sources, or points do not match the checksum in the header.
    BadChecksum,
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "the index file is truncated"),
            Self::BadMagic => write!(f, "the index file does not start with its magic"),
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
        bytes.extend_from_slice(&self.sources.to_le_bytes());
        let checksum = semantic_checksum(&bytes, &points);
        bytes.extend_from_slice(&checksum.to_le_bytes());
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
        let header = valid_header(bytes)?;
        let sources = u32_at(header, 8);
        let expected_crc = u32_at(header, CHECKSUM_AT);
        let body = &bytes[HEADER_LEN..];
        if !body.len().is_multiple_of(POINT_LEN) {
            return Err(IndexError::Truncated);
        }
        if semantic_checksum(&header[..CHECKSUM_AT], body) != expected_crc {
            return Err(IndexError::BadChecksum);
        }
        let points = body
            .chunks_exact(POINT_LEN)
            .map(|chunk| Point {
                ts: i64::from_le_bytes([
                    chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
                ]),
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
        Ok(u32_at(valid_header(bytes)?, CHECKSUM_AT))
    }
}

fn valid_header(bytes: &[u8]) -> Result<&[u8], IndexError> {
    let header = bytes.get(..HEADER_LEN).ok_or(IndexError::Truncated)?;
    if header[..MAGIC.len()] != MAGIC {
        return Err(IndexError::BadMagic);
    }
    Ok(header)
}

const fn semantic_checksum(prefix: &[u8], points: &[u8]) -> u32 {
    let mut checksum = Crc32c::new();
    checksum.update(prefix);
    checksum.update(points);
    checksum.finalize()
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    let mut raw = [0_u8; 4];
    raw.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(raw)
}

#[cfg(test)]
mod tests;
