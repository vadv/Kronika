//! Sparse finding locators and the small upward-spike calculation.

use crate::file::IndexError;

/// Maximum sparse locators stored for one physical section.
pub const MAX_FINDINGS_PER_BLOCK: usize = 4_096;

const FIFTEEN_MINUTES_US: i64 = 15 * 60 * 1_000_000;
const HEADER_LEN: usize = 9;
const FINDING_LEN: usize = 15;

/// The two independent visual marks Kronika records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum FindingKind {
    /// An explicit known-bad boundary was crossed.
    KnownBad = 1,
    /// The current transformed value exceeded its upper Tukey fence.
    Spike = 2,
}

impl FindingKind {
    const fn from_raw(raw: u8) -> Result<Self, IndexError> {
        match raw {
            1 => Ok(Self::KnownBad),
            2 => Ok(Self::Spike),
            _ => Err(IndexError::BadLayout),
        }
    }
}

/// One locator into a physical ZMS section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finding {
    /// Independent visual mark.
    pub kind: FindingKind,
    /// Zero-based field position in the physical registry contract.
    pub field_ordinal: u16,
    /// Physical row position returned by the production reader.
    pub row_ordinal: u32,
    /// Current snapshot timestamp in unix microseconds.
    pub timestamp: i64,
}

impl Finding {
    const fn order_key(self) -> (i64, u32, u16, FindingKind) {
        (
            self.timestamp,
            self.row_ordinal,
            self.field_ordinal,
            self.kind,
        )
    }
}

/// Sparse findings for one physical section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingBlock {
    /// Physical source layout.
    pub type_id: u32,
    /// Qualifying findings before the fixed storage cap was applied.
    pub total_hits: u32,
    /// Whether qualifying locators were omitted by the fixed cap.
    pub truncated: bool,
    /// Stored locators in timestamp and locator order.
    pub findings: Vec<Finding>,
}

impl FindingBlock {
    pub(crate) fn encode(&self) -> Result<Vec<u8>, IndexError> {
        validate(self)?;
        let count = u32::try_from(self.findings.len()).map_err(|_overflow| IndexError::TooLarge)?;
        let capacity = HEADER_LEN
            .checked_add(
                self.findings
                    .len()
                    .checked_mul(FINDING_LEN)
                    .ok_or(IndexError::TooLarge)?,
            )
            .ok_or(IndexError::TooLarge)?;
        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(&self.total_hits.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.push(u8::from(self.truncated));
        for finding in &self.findings {
            out.push(finding.kind as u8);
            out.extend_from_slice(&finding.field_ordinal.to_le_bytes());
            out.extend_from_slice(&finding.row_ordinal.to_le_bytes());
            out.extend_from_slice(&finding.timestamp.to_le_bytes());
        }
        Ok(out)
    }

    pub(crate) fn decode(type_id: u32, bytes: &[u8]) -> Result<Self, IndexError> {
        let total_hits = u32_at(bytes, 0)?;
        let count = usize::try_from(u32_at(bytes, 4)?).map_err(|_overflow| IndexError::TooLarge)?;
        let truncated = match bytes.get(8) {
            Some(0) => false,
            Some(1) => true,
            _ => return Err(IndexError::BadLayout),
        };
        let expected = HEADER_LEN
            .checked_add(count.checked_mul(FINDING_LEN).ok_or(IndexError::TooLarge)?)
            .ok_or(IndexError::TooLarge)?;
        if bytes.len() != expected || count > MAX_FINDINGS_PER_BLOCK {
            return Err(IndexError::BadLayout);
        }
        let mut findings = Vec::with_capacity(count);
        for raw in bytes[HEADER_LEN..].chunks_exact(FINDING_LEN) {
            findings.push(Finding {
                kind: FindingKind::from_raw(raw[0])?,
                field_ordinal: u16_at(raw, 1)?,
                row_ordinal: u32_at(raw, 3)?,
                timestamp: i64_at(raw, 7)?,
            });
        }
        let block = Self {
            type_id,
            total_hits,
            truncated,
            findings,
        };
        validate(&block)?;
        Ok(block)
    }
}

fn validate(block: &FindingBlock) -> Result<(), IndexError> {
    if block.findings.len() > MAX_FINDINGS_PER_BLOCK {
        return Err(IndexError::BadLayout);
    }
    let stored = u32::try_from(block.findings.len()).map_err(|_overflow| IndexError::TooLarge)?;
    if block.total_hits < stored || block.truncated != (block.total_hits > stored) {
        return Err(IndexError::BadLayout);
    }
    let mut previous = None;
    for finding in &block.findings {
        if previous.is_some_and(|before| before >= finding.order_key()) {
            return Err(IndexError::BadLayout);
        }
        previous = Some(finding.order_key());
    }
    Ok(())
}

/// One valid prior transformed value, ordered by timestamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PriorValue {
    /// Snapshot timestamp in unix microseconds.
    pub timestamp: i64,
    /// Value used by the spike calculation.
    pub value: f64,
}

/// Select every preceding value in fifteen minutes when there are at least
/// five, otherwise select exactly the nearest five older values.
#[must_use]
pub fn select_baseline(prior: &[PriorValue], current_ts: i64) -> Option<&[PriorValue]> {
    if prior
        .windows(2)
        .any(|pair| pair[0].timestamp > pair[1].timestamp)
        || prior.iter().any(|point| !point.value.is_finite())
    {
        return None;
    }
    let prior_end = prior.partition_point(|point| point.timestamp < current_ts);
    if prior_end < 5 {
        return None;
    }
    let window_start_ts = current_ts.saturating_sub(FIFTEEN_MINUTES_US);
    let window_start =
        prior[..prior_end].partition_point(|point| point.timestamp < window_start_ts);
    if prior_end - window_start >= 5 {
        Some(&prior[window_start..prior_end])
    } else {
        Some(&prior[prior_end - 5..prior_end])
    }
}

/// Return the upper Tukey fence from at least five finite prior values.
#[must_use]
pub fn upper_tukey_fence(values: &[f64]) -> Option<f64> {
    if values.len() < 5 || values.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let q1 = quartile(&sorted, 1)?;
    let q3 = quartile(&sorted, 3)?;
    let fence = (q3 - q1).mul_add(1.5, q3);
    fence.is_finite().then_some(fence)
}

/// Whether a finite current value is strictly above the prior upper fence.
#[must_use]
pub fn is_upward_spike(current: f64, baseline: &[f64]) -> bool {
    current.is_finite() && upper_tukey_fence(baseline).is_some_and(|fence| current > fence)
}

fn quartile(sorted: &[f64], numerator: usize) -> Option<f64> {
    let scaled = sorted.len().checked_sub(1)?.checked_mul(numerator)?;
    let lower = scaled / 4;
    let remainder = scaled % 4;
    let low = *sorted.get(lower)?;
    let high = *sorted.get(lower.checked_add(usize::from(remainder != 0))?)?;
    let fraction = [0.0, 0.25, 0.5, 0.75][remainder];
    Some((high - low).mul_add(fraction, low))
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, IndexError> {
    let raw: [u8; 2] = bytes
        .get(at..at.checked_add(2).ok_or(IndexError::Truncated)?)
        .ok_or(IndexError::Truncated)?
        .try_into()
        .map_err(|_error| IndexError::Truncated)?;
    Ok(u16::from_le_bytes(raw))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, IndexError> {
    let raw: [u8; 4] = bytes
        .get(at..at.checked_add(4).ok_or(IndexError::Truncated)?)
        .ok_or(IndexError::Truncated)?
        .try_into()
        .map_err(|_error| IndexError::Truncated)?;
    Ok(u32::from_le_bytes(raw))
}

fn i64_at(bytes: &[u8], at: usize) -> Result<i64, IndexError> {
    let raw: [u8; 8] = bytes
        .get(at..at.checked_add(8).ok_or(IndexError::Truncated)?)
        .ok_or(IndexError::Truncated)?
        .try_into()
        .map_err(|_error| IndexError::Truncated)?;
    Ok(i64::from_le_bytes(raw))
}

#[cfg(test)]
mod tests;
