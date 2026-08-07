//! How much of a journal reads before the first frame that does not.

use kronika_format::{
    JOURNAL_HEADER_LEN, JournalHeader, JournalLimits, JournalState, MAX_JOURNAL_PARTS, PartRef,
    ReadAt, scan_journal_streaming_strict_from,
};
use std::io;

/// What a journal held up to its first unreadable byte.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ReadablePrefix {
    /// Segment identity from the header, absent when the header does not read
    /// or names no segment.
    pub(super) segment_id: Option<i64>,
    /// Complete frames, in the order they were appended.
    pub(super) parts: Vec<PartRef>,
}

/// Read frames from the front of a journal until one does not parse.
///
/// Everything before that point is complete and validated; everything from it
/// on is the caller's to set aside. A header that does not read leaves nothing
/// to salvage.
///
/// # Errors
///
/// Returns an [`io::Error`] only when the source itself cannot be read. Bad
/// bytes are not errors here; they are where the prefix ends.
pub(super) fn readable_prefix<R: ReadAt>(source: &R) -> io::Result<ReadablePrefix> {
    let len = source.byte_len()?;
    if len < JOURNAL_HEADER_LEN as u64 {
        return Ok(ReadablePrefix::default());
    }
    let mut encoded = [0_u8; JOURNAL_HEADER_LEN];
    source.read_exact_at(&mut encoded, 0)?;
    let Ok(header) = JournalHeader::decode(encoded) else {
        return Ok(ReadablePrefix::default());
    };
    let JournalState::Active { segment_id } = header.state else {
        return Ok(ReadablePrefix::default());
    };
    let report = scan_journal_streaming_strict_from(
        source,
        JOURNAL_HEADER_LEN as u64,
        JournalLimits::default(),
        MAX_JOURNAL_PARTS,
    )
    .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(ReadablePrefix {
        segment_id: Some(segment_id),
        parts: report.parts,
    })
}

#[cfg(test)]
mod tests;
