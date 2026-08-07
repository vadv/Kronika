//! Walking a journal's frames, in memory or through a reader.

use super::{
    Error, FRAME_HEADER_LEN, FRAME_MAGIC, FrameHeader, MAX_PART_LEN, ReadAt, fmt, io, validate_part,
};

/// Limits used while scanning a journal buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalLimits {
    /// Frames claiming a part longer than this are rejected.
    pub max_part_len: u64,
}

impl Default for JournalLimits {
    fn default() -> Self {
        Self {
            max_part_len: MAX_PART_LEN,
        }
    }
}

/// Location of one valid part body inside the journal buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartRef {
    /// Offset of the part body (after the frame header).
    pub offset: usize,
    /// Length of the part body, bytes.
    pub len: usize,
}

/// One damaged region found by the scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageRegion {
    /// Offset where the damaged frame starts.
    pub from: usize,
    /// What the damage means for the journal.
    pub kind: DamageKind,
}

/// Classification of a damaged journal region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageKind {
    /// An incomplete final frame.
    ///
    /// This is diagnostic classification only. The production journal opens
    /// fail closed and does not truncate or repair damaged bytes.
    TornTail,
    /// A damaged frame with a valid frame after it.
    Middle {
        /// Offset of the next valid frame.
        resumed_at: usize,
    },
    /// Damage at the end of the journal with no later valid frame.
    ///
    /// This is diagnostic classification only; the production journal leaves
    /// these bytes unchanged and rejects the file.
    DamagedTail,
}

/// Result of scanning a journal buffer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanReport {
    /// Valid parts in journal order.
    pub parts: Vec<PartRef>,
    /// Damaged regions in journal order; empty for a clean journal.
    pub damages: Vec<DamageRegion>,
    /// Length of the journal prefix ending at the last valid frame.
    /// After an incomplete final frame this is the truncation point.
    pub valid_len: usize,
}

impl ScanReport {
    /// Return whether the buffer contains only valid frames.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.damages.is_empty()
    }
}

/// Failure while scanning a journal with an explicit part-count bound.
#[derive(Debug)]
pub enum JournalScanError {
    /// The byte source could not be read.
    Io(io::Error),
    /// Another valid frame would exceed the caller's part-count bound.
    PartLimitExceeded {
        /// Maximum number of returned parts.
        limit: usize,
    },
}

impl fmt::Display for JournalScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "journal scan I/O: {error}"),
            Self::PartLimitExceeded { limit } => {
                write!(f, "journal contains more than {limit} parts")
            }
        }
    }
}

impl Error for JournalScanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::PartLimitExceeded { .. } => None,
        }
    }
}

impl From<io::Error> for JournalScanError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Scan an in-memory journal buffer.
#[must_use]
pub fn scan_journal(bytes: &[u8], limits: JournalLimits) -> ScanReport {
    let mut report = ScanReport::default();
    let mut pos = 0_usize;

    while pos < bytes.len() {
        match frame_at(bytes, pos, limits) {
            FrameCheck::Valid { body_len } => {
                report.parts.push(PartRef {
                    offset: pos + FRAME_HEADER_LEN,
                    len: body_len,
                });
                pos += FRAME_HEADER_LEN + body_len;
                report.valid_len = pos;
            }
            FrameCheck::Torn => {
                report.damages.push(DamageRegion {
                    from: pos,
                    kind: DamageKind::TornTail,
                });
                return report;
            }
            FrameCheck::Damaged { implied_end } => {
                if let Some(next) = resync(bytes, pos, implied_end, limits) {
                    report.damages.push(DamageRegion {
                        from: pos,
                        kind: DamageKind::Middle { resumed_at: next },
                    });
                    pos = next;
                    continue;
                }
                // A complete-looking final frame with a sane header is treated
                // like an interrupted write; otherwise keep the damaged tail.
                let kind = if implied_end == Some(bytes.len()) {
                    DamageKind::TornTail
                } else {
                    DamageKind::DamagedTail
                };
                report.damages.push(DamageRegion { from: pos, kind });
                return report;
            }
        }
    }

    report
}

/// Scans a journal source sequentially from `start_at`.
///
/// `start_at` must be a frame boundary. Returned part and damage offsets, and
/// [`ScanReport::valid_len`], remain absolute from the start of the source. If
/// no bytes follow `start_at`, the report is empty and `valid_len` equals
/// `start_at`.
///
/// Scanning stops at the first damaged frame and never searches the damaged
/// bytes for candidate frame magic. Peak memory is one part body and at most
/// `max_parts` references. The part limit is checked before adding each
/// [`PartRef`] to the report.
///
/// # Errors
///
/// Returns [`JournalScanError::PartLimitExceeded`] before returning more than
/// `max_parts` valid frames. A `start_at` beyond the source and failures from
/// the source are returned as [`JournalScanError::Io`].
pub fn scan_journal_streaming_strict_from<R: ReadAt>(
    reader: &R,
    start_at: u64,
    limits: JournalLimits,
    max_parts: usize,
) -> Result<ScanReport, JournalScanError> {
    let total_len = usize::try_from(reader.byte_len()?).map_err(|_overflow| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "source does not fit the address space",
        )
    })?;
    let mut pos = usize::try_from(start_at).map_err(|_overflow| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "start_at does not fit the address space",
        )
    })?;
    if pos > total_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "start_at is beyond the journal source",
        )
        .into());
    }

    let mut report = ScanReport {
        valid_len: pos,
        ..ScanReport::default()
    };
    let mut part_buf = Vec::new();

    while pos < total_len {
        match streaming_frame_at(reader, total_len, pos, limits, &mut part_buf)? {
            StreamingFrame::Valid { body_len } => {
                if report.parts.len() >= max_parts {
                    return Err(JournalScanError::PartLimitExceeded { limit: max_parts });
                }
                report.parts.push(PartRef {
                    offset: pos + FRAME_HEADER_LEN,
                    len: body_len,
                });
                pos += FRAME_HEADER_LEN + body_len;
                report.valid_len = pos;
            }
            StreamingFrame::Damaged {
                reason: FrameDamage::TornFrameHeader | FrameDamage::TornFrameBody,
                ..
            } => {
                report.damages.push(DamageRegion {
                    from: pos,
                    kind: DamageKind::TornTail,
                });
                return Ok(report);
            }
            StreamingFrame::Damaged { implied_end, .. } => {
                let kind = if implied_end == Some(total_len) {
                    DamageKind::TornTail
                } else {
                    DamageKind::DamagedTail
                };
                report.damages.push(DamageRegion { from: pos, kind });
                return Ok(report);
            }
        }
    }

    Ok(report)
}

/// Why one frame position could not be read as a complete part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameDamage {
    /// Fewer than [`FRAME_HEADER_LEN`] bytes remain.
    TornFrameHeader,
    /// The header declares a body beyond the end of the file.
    TornFrameBody,
    /// Frame magic or the header CRC is invalid.
    InvalidFrameHeader,
    /// The declared body is above the admission limit.
    PartTooLarge,
    /// The complete body is not a valid canonical part.
    InvalidPart,
}

/// One frame position as the streaming scanner found it.
enum StreamingFrame {
    /// A complete, fully validated frame body of this length.
    Valid { body_len: usize },
    /// The position holds no usable frame. `implied_end` is the end the
    /// header claimed, when it claimed a sane one.
    Damaged {
        reason: FrameDamage,
        implied_end: Option<usize>,
    },
}

fn streaming_frame_at<R: ReadAt>(
    reader: &R,
    total_len: usize,
    pos: usize,
    limits: JournalLimits,
    part_buf: &mut Vec<u8>,
) -> io::Result<StreamingFrame> {
    let rem = total_len - pos;
    if rem < FRAME_HEADER_LEN {
        return Ok(StreamingFrame::Damaged {
            reason: FrameDamage::TornFrameHeader,
            implied_end: None,
        });
    }
    let mut header_bytes = [0_u8; FRAME_HEADER_LEN];
    reader.read_exact_at(&mut header_bytes, pos as u64)?;
    let Ok(header) = FrameHeader::decode(header_bytes) else {
        return Ok(StreamingFrame::Damaged {
            reason: FrameDamage::InvalidFrameHeader,
            implied_end: None,
        });
    };
    if header.part_len > limits.max_part_len {
        let implied_end = usize::try_from(header.part_len)
            .ok()
            .and_then(|body_len| {
                pos.checked_add(FRAME_HEADER_LEN)
                    .and_then(|body_at| body_at.checked_add(body_len))
            })
            .filter(|end| *end <= total_len);
        return Ok(StreamingFrame::Damaged {
            reason: FrameDamage::PartTooLarge,
            implied_end,
        });
    }
    let Ok(body_len) = usize::try_from(header.part_len) else {
        return Ok(StreamingFrame::Damaged {
            reason: FrameDamage::PartTooLarge,
            implied_end: None,
        });
    };
    if rem - FRAME_HEADER_LEN < body_len {
        return Ok(StreamingFrame::Damaged {
            reason: FrameDamage::TornFrameBody,
            implied_end: None,
        });
    }
    part_buf.resize(body_len, 0);
    reader.read_exact_at(&mut part_buf[..body_len], (pos + FRAME_HEADER_LEN) as u64)?;
    let Ok(catalog) = validate_part(&part_buf[..body_len]) else {
        return Ok(StreamingFrame::Damaged {
            reason: FrameDamage::InvalidPart,
            implied_end: Some(pos + FRAME_HEADER_LEN + body_len),
        });
    };
    if catalog
        .entries
        .iter()
        .try_fold(0_u64, |rows, entry| rows.checked_add(u64::from(entry.rows)))
        .is_none()
    {
        return Ok(StreamingFrame::Damaged {
            reason: FrameDamage::InvalidPart,
            implied_end: Some(pos + FRAME_HEADER_LEN + body_len),
        });
    }
    Ok(StreamingFrame::Valid { body_len })
}

/// Outcome of checking one frame position.
enum FrameCheck {
    /// A valid frame with a validated part of this length.
    Valid { body_len: usize },
    /// The frame is cut off by the end of the buffer: header and length
    /// are plausible (or the header itself is incomplete), nothing
    /// follows. This is an incomplete write, not media damage.
    Torn,
    /// Damaged frame. `implied_end` is set only if the header gave a sane end.
    Damaged { implied_end: Option<usize> },
}

fn frame_at(bytes: &[u8], pos: usize, limits: JournalLimits) -> FrameCheck {
    let rem = bytes.len() - pos;
    if rem < FRAME_HEADER_LEN {
        return FrameCheck::Torn;
    }
    let mut header_bytes = [0_u8; FRAME_HEADER_LEN];
    header_bytes.copy_from_slice(&bytes[pos..pos + FRAME_HEADER_LEN]);
    let Ok(header) = FrameHeader::decode(header_bytes) else {
        return FrameCheck::Damaged { implied_end: None };
    };
    if header.part_len > limits.max_part_len {
        return FrameCheck::Damaged { implied_end: None };
    }
    let Ok(body_len) = usize::try_from(header.part_len) else {
        return FrameCheck::Damaged { implied_end: None };
    };
    if rem - FRAME_HEADER_LEN < body_len {
        // The header CRC is valid and the length is sane, but the body
        // extends past the end: the write was cut mid-frame.
        return FrameCheck::Torn;
    }
    let body = &bytes[pos + FRAME_HEADER_LEN..pos + FRAME_HEADER_LEN + body_len];
    if validate_part(body).is_err() {
        return FrameCheck::Damaged {
            implied_end: Some(pos + FRAME_HEADER_LEN + body_len),
        };
    }
    FrameCheck::Valid { body_len }
}

/// Find the next valid frame after damage.
fn resync(
    bytes: &[u8],
    damaged_at: usize,
    implied_end: Option<usize>,
    limits: JournalLimits,
) -> Option<usize> {
    if let Some(boundary) = implied_end
        && boundary < bytes.len()
        && matches!(frame_at(bytes, boundary, limits), FrameCheck::Valid { .. })
    {
        return Some(boundary);
    }
    let mut cand = damaged_at + 1;
    while cand + FRAME_HEADER_LEN <= bytes.len() {
        match find_magic(&bytes[cand..]) {
            Some(found) => {
                let at = cand + found;
                if let FrameCheck::Valid { .. } = frame_at(bytes, at, limits) {
                    return Some(at);
                }
                cand = at + 1;
            }
            None => return None,
        }
    }
    None
}

/// Position of the first `FRAME_MAGIC` occurrence in `haystack`.
fn find_magic(haystack: &[u8]) -> Option<usize> {
    haystack
        .windows(FRAME_MAGIC.len())
        .position(|window| window == FRAME_MAGIC)
}
