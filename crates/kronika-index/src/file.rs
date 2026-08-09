//! The targeted `.idx` container: header, physical-layout TOC, and blocks.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};

use kronika_format::Crc32c;

use crate::summary::{SectionSummary, decode_section, encode_section};

/// Magic of the unreleased targeted index format.
pub const MAGIC: [u8; 8] = *b"KRNIDX3\0";
/// Bytes before the table: magic, source set, entry count, checksum.
pub const HEADER_LEN: usize = 20;
/// Bytes per TOC entry: kind, physical type id, body offset, body length.
pub const ENTRY_LEN: usize = 16;

const CHECKSUM_AT: usize = 16;
const KIND_SECTION: u32 = 1;
const MAX_INDEX_BYTES: u64 = 128 * 1024 * 1024;
const CHECKSUM_CHUNK: usize = 16 * 1024;

/// Why an index container or one selected block was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexError {
    /// A declared byte range is absent.
    Truncated,
    /// The file is from another format.
    BadMagic,
    /// The persisted checksum does not match the bytes.
    BadChecksum,
    /// TOC order, a block, or a bounded count is invalid.
    BadLayout,
    /// The file exceeds the fixed index read bound.
    TooLarge,
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "the index file is truncated"),
            Self::BadMagic => write!(f, "the index file does not start with its magic"),
            Self::BadChecksum => write!(f, "the index file does not match its checksum"),
            Self::BadLayout => write!(f, "the index table or selected block is invalid"),
            Self::TooLarge => write!(f, "the index file exceeds its read bound"),
        }
    }
}

impl std::error::Error for IndexError {}

/// All blocks built for one segment.
#[derive(Debug, Clone, PartialEq)]
pub struct Index {
    /// Source bitset explicitly configured for the web process.
    pub sources: u32,
    /// One independently encoded block per physical registry layout.
    pub sections: Vec<SectionSummary>,
}

/// Blocks selected from a validated index.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetedIndex {
    /// Canonical checksum suitable for a finished response `ETag`; active
    /// computed resources have no checksum.
    pub checksum: Option<u32>,
    /// Source bitset under which the file was built.
    pub sources: u32,
    /// Selected physical layout blocks, in `type_id` order.
    pub sections: Vec<SectionSummary>,
}

#[derive(Debug, Clone, Copy)]
struct TocEntry {
    kind: u32,
    type_id: u32,
    offset: u32,
    len: u32,
}

impl Index {
    /// Encode a canonical index file.
    ///
    /// # Errors
    ///
    /// Returns [`IndexError::BadLayout`] for duplicate/out-of-order layouts or
    /// a block that does not fit the container fields, or
    /// [`IndexError::TooLarge`] when the complete file crosses its read bound.
    pub fn encode(&self) -> Result<Vec<u8>, IndexError> {
        let count =
            u32::try_from(self.sections.len()).map_err(|_overflow| IndexError::BadLayout)?;
        let table_len = self
            .sections
            .len()
            .checked_mul(ENTRY_LEN)
            .ok_or(IndexError::TooLarge)?;
        let body_at = HEADER_LEN
            .checked_add(table_len)
            .ok_or(IndexError::TooLarge)?;
        if u64::try_from(body_at).map_err(|_overflow| IndexError::TooLarge)? > MAX_INDEX_BYTES {
            return Err(IndexError::TooLarge);
        }

        let mut bytes = Vec::with_capacity(body_at);
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&self.sources.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.resize(body_at, 0);

        let mut previous = None;
        let mut offset = 0_u32;
        for (at, section) in self.sections.iter().enumerate() {
            if previous.is_some_and(|before| before >= section.type_id) {
                return Err(IndexError::BadLayout);
            }
            previous = Some(section.type_id);
            let body = encode_section(section)?;
            let len = u32::try_from(body.len()).map_err(|_overflow| IndexError::BadLayout)?;
            if len == 0 {
                return Err(IndexError::BadLayout);
            }
            let table_at = HEADER_LEN
                .checked_add(at.checked_mul(ENTRY_LEN).ok_or(IndexError::TooLarge)?)
                .ok_or(IndexError::TooLarge)?;
            put_u32_at(&mut bytes, table_at, KIND_SECTION)?;
            put_u32_at(&mut bytes, table_at + 4, section.type_id)?;
            put_u32_at(&mut bytes, table_at + 8, offset)?;
            put_u32_at(&mut bytes, table_at + 12, len)?;
            offset = offset.checked_add(len).ok_or(IndexError::BadLayout)?;
            let total = bytes
                .len()
                .checked_add(body.len())
                .ok_or(IndexError::TooLarge)?;
            if u64::try_from(total).map_err(|_overflow| IndexError::TooLarge)? > MAX_INDEX_BYTES {
                return Err(IndexError::TooLarge);
            }
            bytes.extend_from_slice(&body);
        }

        let value = checksum(&bytes[..CHECKSUM_AT], &bytes[HEADER_LEN..]);
        bytes[CHECKSUM_AT..HEADER_LEN].copy_from_slice(&value.to_le_bytes());
        Ok(bytes)
    }

    /// Decode every block in a byte slice.
    ///
    /// # Errors
    ///
    /// Returns the first container or block validation failure.
    pub fn decode(bytes: &[u8]) -> Result<Self, IndexError> {
        let mut cursor = std::io::Cursor::new(bytes);
        read_all(&mut cursor)
    }

    /// Decode only selected indexed-series layouts.
    ///
    /// The checksum and complete TOC are validated, but unrelated blocks are
    /// never decoded.
    ///
    /// # Errors
    ///
    /// Returns the first container or selected-block validation failure.
    pub fn decode_target(bytes: &[u8], type_ids: &[u32]) -> Result<TargetedIndex, IndexError> {
        let mut cursor = std::io::Cursor::new(bytes);
        read_target(&mut cursor, type_ids)
    }
}

/// Read selected blocks from a seekable index without allocating unrelated
/// block bodies.
pub(crate) fn read_target(
    reader: &mut (impl Read + Seek),
    type_ids: &[u32],
) -> Result<TargetedIndex, IndexError> {
    let (sources, expected_checksum, table, body_at, file_len) = metadata(reader)?;
    validate_checksum(reader, expected_checksum, file_len)?;
    let wanted: HashSet<u32> = type_ids.iter().copied().collect();
    let mut sections = Vec::new();
    for entry in table {
        let selected = entry.kind == KIND_SECTION && wanted.contains(&entry.type_id);
        if !selected {
            continue;
        }
        let absolute = body_at
            .checked_add(u64::from(entry.offset))
            .ok_or(IndexError::BadLayout)?;
        reader
            .seek(SeekFrom::Start(absolute))
            .map_err(|_error| IndexError::Truncated)?;
        let len = usize::try_from(entry.len).map_err(|_overflow| IndexError::TooLarge)?;
        let mut block = vec![0_u8; len];
        reader
            .read_exact(&mut block)
            .map_err(|_error| IndexError::Truncated)?;
        sections.push(decode_section(&block, entry.type_id)?);
    }
    Ok(TargetedIndex {
        checksum: Some(expected_checksum),
        sources,
        sections,
    })
}

pub(crate) fn read_all(reader: &mut (impl Read + Seek)) -> Result<Index, IndexError> {
    let (sources, expected_checksum, table, body_at, file_len) = metadata(reader)?;
    validate_checksum(reader, expected_checksum, file_len)?;
    let mut sections = Vec::new();
    for entry in table {
        let absolute = body_at
            .checked_add(u64::from(entry.offset))
            .ok_or(IndexError::BadLayout)?;
        reader
            .seek(SeekFrom::Start(absolute))
            .map_err(|_error| IndexError::Truncated)?;
        let len = usize::try_from(entry.len).map_err(|_overflow| IndexError::TooLarge)?;
        let mut block = vec![0_u8; len];
        reader
            .read_exact(&mut block)
            .map_err(|_error| IndexError::Truncated)?;
        sections.push(decode_section(&block, entry.type_id)?);
    }
    Ok(Index { sources, sections })
}

fn metadata(
    reader: &mut (impl Read + Seek),
) -> Result<(u32, u32, Vec<TocEntry>, u64, u64), IndexError> {
    let file_len = reader
        .seek(SeekFrom::End(0))
        .map_err(|_error| IndexError::Truncated)?;
    if file_len > MAX_INDEX_BYTES {
        return Err(IndexError::TooLarge);
    }
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_error| IndexError::Truncated)?;
    let mut header = [0_u8; HEADER_LEN];
    reader
        .read_exact(&mut header)
        .map_err(|_error| IndexError::Truncated)?;
    if header[..MAGIC.len()] != MAGIC {
        return Err(IndexError::BadMagic);
    }
    let sources = u32_at(&header, 8)?;
    let count = usize::try_from(u32_at(&header, 12)?).map_err(|_overflow| IndexError::TooLarge)?;
    let expected_checksum = u32_at(&header, CHECKSUM_AT)?;
    let table_len = count.checked_mul(ENTRY_LEN).ok_or(IndexError::TooLarge)?;
    let body_at = u64::try_from(HEADER_LEN)
        .ok()
        .and_then(|head| {
            u64::try_from(table_len)
                .ok()
                .and_then(|table| head.checked_add(table))
        })
        .ok_or(IndexError::TooLarge)?;
    if body_at > file_len {
        return Err(IndexError::Truncated);
    }
    let mut raw_table = vec![0_u8; table_len];
    reader
        .read_exact(&mut raw_table)
        .map_err(|_error| IndexError::Truncated)?;
    let mut table = Vec::with_capacity(count);
    let mut expected_offset = 0_u32;
    let mut previous_type = None;
    for raw in raw_table.chunks_exact(ENTRY_LEN) {
        let entry = TocEntry {
            kind: u32_at(raw, 0)?,
            type_id: u32_at(raw, 4)?,
            offset: u32_at(raw, 8)?,
            len: u32_at(raw, 12)?,
        };
        if entry.offset != expected_offset || entry.len == 0 {
            return Err(IndexError::BadLayout);
        }
        expected_offset = expected_offset
            .checked_add(entry.len)
            .ok_or(IndexError::BadLayout)?;
        match entry.kind {
            KIND_SECTION if previous_type.is_none_or(|before| before < entry.type_id) => {
                previous_type = Some(entry.type_id);
            }
            _ => return Err(IndexError::BadLayout),
        }
        table.push(entry);
    }
    let declared_end = body_at
        .checked_add(u64::from(expected_offset))
        .ok_or(IndexError::BadLayout)?;
    if declared_end != file_len {
        return Err(IndexError::BadLayout);
    }
    Ok((sources, expected_checksum, table, body_at, file_len))
}

fn validate_checksum(
    reader: &mut (impl Read + Seek),
    expected: u32,
    file_len: u64,
) -> Result<(), IndexError> {
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_error| IndexError::Truncated)?;
    let mut prefix = [0_u8; CHECKSUM_AT];
    reader
        .read_exact(&mut prefix)
        .map_err(|_error| IndexError::Truncated)?;
    reader
        .seek(SeekFrom::Start(HEADER_LEN as u64))
        .map_err(|_error| IndexError::Truncated)?;
    let mut checksum = Crc32c::new();
    checksum.update(&prefix);
    let mut remaining = file_len.saturating_sub(HEADER_LEN as u64);
    let mut buffer = [0_u8; CHECKSUM_CHUNK];
    while remaining != 0 {
        let take = usize::try_from(remaining.min(CHECKSUM_CHUNK as u64))
            .map_err(|_overflow| IndexError::TooLarge)?;
        reader
            .read_exact(&mut buffer[..take])
            .map_err(|_error| IndexError::Truncated)?;
        checksum.update(&buffer[..take]);
        remaining -= take as u64;
    }
    if checksum.finalize() == expected {
        Ok(())
    } else {
        Err(IndexError::BadChecksum)
    }
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, IndexError> {
    let end = at.checked_add(4).ok_or(IndexError::Truncated)?;
    let raw: [u8; 4] = bytes
        .get(at..end)
        .ok_or(IndexError::Truncated)?
        .try_into()
        .map_err(|_error| IndexError::Truncated)?;
    Ok(u32::from_le_bytes(raw))
}

fn put_u32_at(bytes: &mut [u8], at: usize, value: u32) -> Result<(), IndexError> {
    let end = at.checked_add(4).ok_or(IndexError::BadLayout)?;
    bytes
        .get_mut(at..end)
        .ok_or(IndexError::BadLayout)?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

const fn checksum(head: &[u8], rest: &[u8]) -> u32 {
    let mut checksum = Crc32c::new();
    checksum.update(head);
    checksum.update(rest);
    checksum.finalize()
}

#[cfg(test)]
mod tests;
