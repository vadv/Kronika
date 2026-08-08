//! The `.idx` file: a header, a table of what it holds, and the blocks.
//!
//! A request for the health line does not decode the object rows, so the file
//! says where each block starts and how long it is. Offsets, lengths and the
//! checksum belong to the file; what a block holds belongs to the block.

use kronika_format::Crc32c;

use crate::objects::{self, SectionObjects};

/// Magic of an index file.
///
/// A file written under a different one is not read: it is deleted and built
/// again from the segment beside it.
pub const MAGIC: [u8; 8] = *b"KRNIDX2\0";

/// Bytes before the block table: magic, sources, block count, checksum.
pub const HEADER_LEN: usize = 20;

/// Bytes per block-table entry: kind, offset, length.
pub const ENTRY_LEN: usize = 12;

/// Bytes per health point: the timestamp and its health.
pub const POINT_LEN: usize = 9;

/// Health of a point that could not be computed.
const NO_HEALTH: u8 = 0xFF;

/// The checksum ends the header and is the only field it does not cover.
const CHECKSUM_AT: usize = 16;

/// Block kinds.
const KIND_HEALTH: u32 = 1;
const KIND_OBJECTS: u32 = 2;

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
#[derive(Debug, Clone, PartialEq)]
pub struct Index {
    /// Sources enabled when the file was built. A different set is a different
    /// file, and web rebuilds it.
    pub sources: u32,
    /// Health over the segment, oldest first.
    pub points: Vec<Point>,
    /// The objects each section saw over the segment.
    pub objects: Vec<SectionObjects>,
}

/// Why an index file was rejected.
///
/// Every one of these means the same thing to a caller: delete the file and
/// build it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexError {
    /// Shorter than it says it is, or a block runs past its end.
    Truncated,
    /// The first eight bytes are not [`MAGIC`].
    BadMagic,
    /// The bytes do not match the checksum in the header.
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
    ///
    /// A block with nothing in it is left out rather than written empty.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut blocks: Vec<(u32, Vec<u8>)> = Vec::new();
        if !self.points.is_empty() {
            let mut body = Vec::with_capacity(self.points.len() * POINT_LEN);
            for point in &self.points {
                body.extend_from_slice(&point.ts.to_le_bytes());
                body.push(point.health.unwrap_or(NO_HEALTH));
            }
            blocks.push((KIND_HEALTH, body));
        }
        if !self.objects.is_empty() {
            let mut body = Vec::new();
            objects::encode(&self.objects, &mut body);
            blocks.push((KIND_OBJECTS, body));
        }

        let mut table = Vec::with_capacity(blocks.len() * ENTRY_LEN);
        let mut offset = 0_u32;
        for (kind, body) in &blocks {
            let len = u32::try_from(body.len()).unwrap_or(u32::MAX);
            table.extend_from_slice(&kind.to_le_bytes());
            table.extend_from_slice(&offset.to_le_bytes());
            table.extend_from_slice(&len.to_le_bytes());
            offset = offset.saturating_add(len);
        }

        let mut head = Vec::with_capacity(CHECKSUM_AT);
        head.extend_from_slice(&MAGIC);
        head.extend_from_slice(&self.sources.to_le_bytes());
        head.extend_from_slice(
            &u32::try_from(blocks.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );

        let mut rest = table;
        for (_kind, body) in &blocks {
            rest.extend_from_slice(body);
        }

        let mut bytes = Vec::with_capacity(HEADER_LEN + rest.len());
        bytes.extend_from_slice(&head);
        bytes.extend_from_slice(&checksum(&head, &rest).to_le_bytes());
        bytes.extend_from_slice(&rest);
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
        let count = u32_at(header, 12) as usize;
        let rest = bytes.get(HEADER_LEN..).ok_or(IndexError::Truncated)?;
        if checksum(&header[..CHECKSUM_AT], rest) != u32_at(header, CHECKSUM_AT) {
            return Err(IndexError::BadChecksum);
        }

        let table_len = count.checked_mul(ENTRY_LEN).ok_or(IndexError::Truncated)?;
        let table = rest.get(..table_len).ok_or(IndexError::Truncated)?;
        let body = &rest[table_len..];

        let mut index = Self {
            sources,
            points: Vec::new(),
            objects: Vec::new(),
        };
        for entry in table.chunks_exact(ENTRY_LEN) {
            let kind = u32_at(entry, 0);
            let at = u32_at(entry, 4) as usize;
            let len = u32_at(entry, 8) as usize;
            let end = at.checked_add(len).ok_or(IndexError::Truncated)?;
            let block = body.get(at..end).ok_or(IndexError::Truncated)?;
            match kind {
                KIND_HEALTH => index.points = decode_points(block)?,
                KIND_OBJECTS => index.objects = objects::decode(block)?,
                _unknown => return Err(IndexError::Truncated),
            }
        }
        Ok(index)
    }

    /// The checksum the header carries, without decoding anything.
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

fn decode_points(block: &[u8]) -> Result<Vec<Point>, IndexError> {
    if !block.len().is_multiple_of(POINT_LEN) {
        return Err(IndexError::Truncated);
    }
    Ok(block
        .chunks_exact(POINT_LEN)
        .map(|chunk| Point {
            ts: i64::from_le_bytes([
                chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
            ]),
            health: Some(chunk[8]).filter(|health| *health != NO_HEALTH),
        })
        .collect())
}

fn valid_header(bytes: &[u8]) -> Result<&[u8], IndexError> {
    let header = bytes.get(..HEADER_LEN).ok_or(IndexError::Truncated)?;
    if header[..MAGIC.len()] != MAGIC {
        return Err(IndexError::BadMagic);
    }
    Ok(header)
}

const fn checksum(head: &[u8], rest: &[u8]) -> u32 {
    let mut checksum = Crc32c::new();
    checksum.update(head);
    checksum.update(rest);
    checksum.finalize()
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    let mut raw = [0_u8; 4];
    raw.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(raw)
}

#[cfg(test)]
mod tests;
