//! Shared paging, cursor, source-pin, and result-budget behavior.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use kronika_reader::{Reader, SegmentKind, SegmentRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Default number of whole rows in a dense product page.
pub(crate) const DEFAULT_PAGE_SIZE: u16 = 200;
/// Largest caller-selected number of whole rows in a dense product page.
pub(crate) const MAX_PAGE_SIZE: u16 = 5_000;
/// One private byte ceiling shared by every dense product page.
pub(crate) const SHARED_RESULT_MAX_BYTES: usize = 1024 * 1024;

const CURSOR_PREFIX: &str = "pc1_";
const CURSOR_DOMAIN: &[u8] = b"kronika product page cursor v1\0";
const CURSOR_SIGNATURE_BYTES: usize = 32;

/// Dense product surfaces which use the concrete shared pager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PageSurface {
    /// PostgreSQL Activity.
    Activity,
    /// PostgreSQL Statements.
    Statements,
    /// PostgreSQL Tables.
    Tables,
    /// PostgreSQL Indexes.
    Indexes,
}

/// Public semantic ordering direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Direction {
    /// Ascending non-null values, with nulls last.
    Asc,
    /// Descending non-null values, with nulls last.
    Desc,
}

/// Normalized common page inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageRequest {
    /// Maximum number of whole rows to return.
    pub(crate) page_size: u16,
    /// Opaque continuation, if this is not the first page.
    pub(crate) cursor: Option<String>,
}

/// One typed page shared by the four dense products.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Page<Row> {
    /// Whole rows in deterministic product order.
    pub(crate) rows: Vec<Row>,
    /// Cursor for the first unreturned row, or `None` at exhaustion.
    pub(crate) next_cursor: Option<String>,
}

/// Key used to authenticate opaque page cursors.
#[derive(Clone)]
pub(crate) struct PageKey([u8; CURSOR_SIGNATURE_BYTES]);

impl fmt::Debug for PageKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PageKey([redacted])")
    }
}

impl PageKey {
    /// Derive a domain-separated cursor key from the authenticated deployment.
    pub(crate) fn derive(authentication_material: &[u8]) -> Self {
        let mut digest = Sha256::new();
        digest.update(CURSOR_DOMAIN);
        digest.update(authentication_material);
        Self(digest.finalize().into())
    }
}

/// Exact source reference retained by a continuation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourcePin {
    #[serde(rename = "i")]
    id: i64,
    #[serde(rename = "k")]
    kind: SourceKind,
    #[serde(rename = "p", skip_serializing_if = "Option::is_none")]
    active_position: Option<u64>,
    #[serde(rename = "f")]
    fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SourceKind {
    #[serde(rename = "f")]
    Finished,
    #[serde(rename = "a")]
    Active,
}

impl SourcePin {
    /// Capture stable catalog shape and the committed active prefix.
    pub(crate) fn capture(source: &SegmentRef) -> Self {
        Self {
            id: source.id(),
            kind: match source.kind() {
                SegmentKind::Finished => SourceKind::Finished,
                SegmentKind::Active => SourceKind::Active,
            },
            active_position: source.active_position(),
            fingerprint: source_fingerprint(source),
        }
    }

    pub(crate) const fn segment_id(&self) -> i64 {
        self.id
    }

    #[cfg(test)]
    pub(crate) fn fixture(id: i64, active_position: Option<u64>) -> Self {
        Self {
            id,
            kind: if active_position.is_some() {
                SourceKind::Active
            } else {
                SourceKind::Finished
            },
            active_position,
            fingerprint: format!("fixture-{id}-{active_position:?}"),
        }
    }
}

/// Query and source state bound into an opaque cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CursorBinding {
    /// Concrete product surface.
    pub(crate) surface: PageSurface,
    /// Digest of every normalized surface query input other than page size.
    pub(crate) query_binding: String,
    /// Selected observation, when the surface selects one.
    pub(crate) selected_at: Option<i64>,
    /// Complete source view captured by the first call.
    pub(crate) source_pins: Vec<SourcePin>,
    /// Normalized maximum row count.
    pub(crate) page_size: u16,
}

/// Verified continuation payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedCursor {
    /// Cursor-bound query and source state.
    pub(crate) binding: CursorBinding,
    /// Index of the first unreturned row in the pinned ordered stream.
    pub(crate) first_unreturned: usize,
    /// Physical first-unreturned row used to resume a bounded snapshot scan.
    pub(crate) snapshot_position: Option<SnapshotPosition>,
}

/// Stable physical position of one first-unreturned snapshot row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SnapshotPosition {
    /// Deterministic page-context index within the pinned source view.
    pub(crate) context_index: usize,
    /// Physical ordinal within that context's section.
    pub(crate) ordinal: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotPositionWire {
    #[serde(rename = "c")]
    context_index: u64,
    #[serde(rename = "r")]
    ordinal: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct CursorWire {
    #[serde(rename = "s")]
    surface: PageSurface,
    #[serde(rename = "a")]
    query_binding: String,
    #[serde(rename = "o", skip_serializing_if = "Option::is_none")]
    selected_at: Option<i64>,
    #[serde(rename = "p")]
    source_pins: Vec<SourcePin>,
    #[serde(rename = "n")]
    page_size: u16,
    #[serde(rename = "u")]
    first_unreturned: u64,
    #[serde(rename = "r", skip_serializing_if = "Option::is_none")]
    snapshot_position: Option<SnapshotPositionWire>,
}

/// Failure from common page machinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageError {
    /// Cursor syntax, signature, binding, or pinned source is invalid.
    InvalidCursor,
    /// The next whole row cannot fit by itself.
    ResultTooLarge,
    /// The typed result could not be measured.
    ResultEncoding,
}

impl fmt::Display for PageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidCursor => "invalid page cursor",
            Self::ResultTooLarge => "the next whole row does not fit in a minimum page",
            Self::ResultEncoding => "the typed page could not be encoded",
        })
    }
}

impl std::error::Error for PageError {}

/// Domain-separated canonical digest for a normalized product query.
#[derive(Debug)]
pub(crate) struct QueryBinding {
    digest: Sha256,
}

impl QueryBinding {
    /// Start one surface-specific binding.
    pub(crate) fn new(surface: PageSurface) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"kronika normalized product query v1\0");
        digest.update(match surface {
            PageSurface::Activity => b"activity".as_slice(),
            PageSurface::Statements => b"statements".as_slice(),
            PageSurface::Tables => b"tables".as_slice(),
            PageSurface::Indexes => b"indexes".as_slice(),
        });
        Self { digest }
    }

    /// Add one unambiguous tagged byte value.
    pub(crate) fn part(&mut self, tag: &str, value: &[u8]) {
        self.digest.update(tag.len().to_le_bytes());
        self.digest.update(tag.as_bytes());
        self.digest.update(value.len().to_le_bytes());
        self.digest.update(value);
    }

    /// Finish as a compact URL-safe digest.
    pub(crate) fn finish(self) -> String {
        URL_SAFE_NO_PAD.encode(self.digest.finalize())
    }
}

/// Encode a first-unreturned-row cursor for one normalized binding.
pub(crate) fn encode_cursor(
    binding: &CursorBinding,
    first_unreturned: usize,
    key: &PageKey,
) -> Result<String, PageError> {
    encode_cursor_position(binding, first_unreturned, None, key)
}

/// Encode a first-unreturned physical snapshot row for a bounded resume scan.
pub(crate) fn encode_snapshot_cursor(
    binding: &CursorBinding,
    first_unreturned: usize,
    snapshot_position: SnapshotPosition,
    key: &PageKey,
) -> Result<String, PageError> {
    encode_cursor_position(binding, first_unreturned, Some(snapshot_position), key)
}

fn encode_cursor_position(
    binding: &CursorBinding,
    first_unreturned: usize,
    snapshot_position: Option<SnapshotPosition>,
    key: &PageKey,
) -> Result<String, PageError> {
    let wire = CursorWire {
        surface: binding.surface,
        query_binding: binding.query_binding.clone(),
        selected_at: binding.selected_at,
        source_pins: binding.source_pins.clone(),
        page_size: binding.page_size,
        first_unreturned: u64::try_from(first_unreturned)
            .map_err(|_overflow| PageError::ResultEncoding)?,
        snapshot_position: snapshot_position
            .map(|position| {
                Ok(SnapshotPositionWire {
                    context_index: u64::try_from(position.context_index)
                        .map_err(|_overflow| PageError::ResultEncoding)?,
                    ordinal: position.ordinal,
                })
            })
            .transpose()?,
    };
    let payload = serde_json::to_vec(&wire).map_err(|_error| PageError::ResultEncoding)?;
    let signature = hmac_sha256(&key.0, &payload);
    let encoded = format!(
        "{CURSOR_PREFIX}{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        URL_SAFE_NO_PAD.encode(signature)
    );
    if encoded.len() > 4_096 {
        return Err(PageError::ResultEncoding);
    }
    Ok(encoded)
}

/// Decode and authenticate one opaque page cursor.
pub(crate) fn decode_cursor(raw: &str, key: &PageKey) -> Result<DecodedCursor, PageError> {
    if raw.is_empty() || raw.len() > 4_096 {
        return Err(PageError::InvalidCursor);
    }
    let encoded = raw
        .strip_prefix(CURSOR_PREFIX)
        .ok_or(PageError::InvalidCursor)?;
    let (payload_text, signature_text) = encoded.split_once('.').ok_or(PageError::InvalidCursor)?;
    if payload_text.is_empty() || signature_text.is_empty() || signature_text.contains('.') {
        return Err(PageError::InvalidCursor);
    }
    let payload = URL_SAFE_NO_PAD
        .decode(payload_text)
        .map_err(|_error| PageError::InvalidCursor)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature_text)
        .map_err(|_error| PageError::InvalidCursor)?;
    let offered = <[u8; CURSOR_SIGNATURE_BYTES]>::try_from(signature.as_slice())
        .map_err(|_error| PageError::InvalidCursor)?;
    if URL_SAFE_NO_PAD.encode(&payload) != payload_text
        || URL_SAFE_NO_PAD.encode(offered) != signature_text
        || !same(&offered, &hmac_sha256(&key.0, &payload))
    {
        return Err(PageError::InvalidCursor);
    }
    let wire: CursorWire =
        serde_json::from_slice(&payload).map_err(|_error| PageError::InvalidCursor)?;
    let first_unreturned =
        usize::try_from(wire.first_unreturned).map_err(|_overflow| PageError::InvalidCursor)?;
    let snapshot_position = wire
        .snapshot_position
        .map(|position| {
            Ok(SnapshotPosition {
                context_index: usize::try_from(position.context_index)
                    .map_err(|_overflow| PageError::InvalidCursor)?,
                ordinal: position.ordinal,
            })
        })
        .transpose()?;
    Ok(DecodedCursor {
        binding: CursorBinding {
            surface: wire.surface,
            query_binding: wire.query_binding,
            selected_at: wire.selected_at,
            source_pins: wire.source_pins,
            page_size: wire.page_size,
        },
        first_unreturned,
        snapshot_position,
    })
}

/// Reopen exactly the source view retained by a verified cursor.
pub(crate) fn reopen_sources(
    reader: &Reader,
    pins: &[SourcePin],
) -> Result<Vec<SegmentRef>, PageError> {
    let mut sources = Vec::with_capacity(pins.len());
    for pin in pins {
        let listing = reader
            .catalog_segment(pin.id)
            .map_err(|_error| PageError::InvalidCursor)?;
        let mut listed = listing.segments.into_iter();
        let current = listed.next().ok_or(PageError::InvalidCursor)?;
        if listed.next().is_some() {
            return Err(PageError::InvalidCursor);
        }
        let source = match (pin.kind, current.kind(), pin.active_position) {
            (SourceKind::Finished, SegmentKind::Finished, None) => current,
            (SourceKind::Active, SegmentKind::Active, Some(position)) => current
                .at_active_position(position)
                .map_err(|_error| PageError::InvalidCursor)?,
            (SourceKind::Finished, SegmentKind::Finished, Some(_))
            | (SourceKind::Finished, SegmentKind::Active, _)
            | (SourceKind::Active, SegmentKind::Finished, _)
            | (SourceKind::Active, SegmentKind::Active, None) => {
                return Err(PageError::InvalidCursor);
            }
        };
        if SourcePin::capture(&source) != *pin {
            return Err(PageError::InvalidCursor);
        }
        sources.push(source);
    }
    Ok(sources)
}

/// Fit the longest leading whole-row page under the shared result ceiling.
pub(crate) fn fit_page<Row: Clone>(
    ordered: &[Row],
    first_unreturned: usize,
    page_size: u16,
    mut cursor_at: impl FnMut(usize) -> Result<String, PageError>,
    mut measure: impl FnMut(&[Row], Option<&str>) -> Result<usize, PageError>,
) -> Result<Page<Row>, PageError> {
    if first_unreturned > ordered.len() || !(1..=MAX_PAGE_SIZE).contains(&page_size) {
        return Err(PageError::InvalidCursor);
    }
    if first_unreturned == ordered.len() {
        return Ok(Page {
            rows: Vec::new(),
            next_cursor: None,
        });
    }
    let remaining = ordered.len() - first_unreturned;
    let maximum = remaining.min(usize::from(page_size));
    let maximum_end = first_unreturned + maximum;
    if maximum_end == ordered.len()
        && measure(&ordered[first_unreturned..maximum_end], None)? <= SHARED_RESULT_MAX_BYTES
    {
        return Ok(Page {
            rows: ordered[first_unreturned..maximum_end].to_vec(),
            next_cursor: None,
        });
    }
    let fits = |count: usize,
                cursor_at: &mut dyn FnMut(usize) -> Result<String, PageError>,
                measure: &mut dyn FnMut(&[Row], Option<&str>) -> Result<usize, PageError>|
     -> Result<Option<String>, PageError> {
        let end = first_unreturned + count;
        let cursor = match cursor_at(end) {
            Ok(cursor) => cursor,
            Err(PageError::ResultTooLarge) => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(
            (measure(&ordered[first_unreturned..end], Some(&cursor))? <= SHARED_RESULT_MAX_BYTES)
                .then_some(cursor),
        )
    };
    let Some(first_cursor) = fits(1, &mut cursor_at, &mut measure)? else {
        return Err(PageError::ResultTooLarge);
    };
    let mut low = 1;
    let mut low_cursor = first_cursor;
    let mut high = maximum.saturating_add(1);
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if let Some(cursor) = fits(middle, &mut cursor_at, &mut measure)? {
            low = middle;
            low_cursor = cursor;
        } else {
            high = middle;
        }
    }
    Ok(Page {
        rows: ordered[first_unreturned..first_unreturned + low].to_vec(),
        next_cursor: Some(low_cursor),
    })
}

fn source_fingerprint(source: &SegmentRef) -> String {
    let mut digest = Sha256::new();
    digest.update(b"kronika product source pin v1\0");
    digest.update(source.id().to_le_bytes());
    digest.update(source.min_ts().to_le_bytes());
    digest.update(source.max_ts().to_le_bytes());
    digest.update(source.active_position().unwrap_or(0).to_le_bytes());
    digest.update(source.sections().len().to_le_bytes());
    for section in source.sections() {
        digest.update(section.type_id.to_le_bytes());
        digest.update(section.rows.to_le_bytes());
        digest.update(section.bytes.to_le_bytes());
    }
    URL_SAFE_NO_PAD.encode(digest.finalize())
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; CURSOR_SIGNATURE_BYTES] {
    const BLOCK_BYTES: usize = 64;
    let mut block = [0_u8; BLOCK_BYTES];
    if key.len() > BLOCK_BYTES {
        block[..CURSOR_SIGNATURE_BYTES].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; BLOCK_BYTES];
    let mut outer_pad = [0x5c_u8; BLOCK_BYTES];
    for ((inner, outer), byte) in inner_pad.iter_mut().zip(&mut outer_pad).zip(block) {
        *inner ^= byte;
        *outer ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn same(left: &[u8; CURSOR_SIGNATURE_BYTES], right: &[u8; CURSOR_SIGNATURE_BYTES]) -> bool {
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::{
        CursorBinding, PageError, PageKey, PageSurface, SnapshotPosition, decode_cursor,
        encode_cursor, encode_snapshot_cursor, fit_page,
    };

    fn binding() -> CursorBinding {
        CursorBinding {
            surface: PageSurface::Activity,
            query_binding: "query".to_owned(),
            selected_at: Some(9),
            source_pins: Vec::new(),
            page_size: 2,
        }
    }

    #[test]
    fn opaque_cursor_roundtrips_wide_bounds_and_rejects_tampering() {
        let key = PageKey::derive(b"one account");
        let encoded = encode_cursor(&binding(), 3, &key).expect("cursor encodes");
        let decoded = decode_cursor(&encoded, &key).expect("cursor decodes");
        assert_eq!(decoded.binding, binding());
        assert_eq!(decoded.first_unreturned, 3);
        assert_eq!(decoded.snapshot_position, None);

        let position = SnapshotPosition {
            context_index: 17,
            ordinal: u64::MAX,
        };
        let snapshot =
            encode_snapshot_cursor(&binding(), 4, position, &key).expect("snapshot cursor encodes");
        let decoded = decode_cursor(&snapshot, &key).expect("snapshot cursor decodes");
        assert_eq!(decoded.first_unreturned, 4);
        assert_eq!(decoded.snapshot_position, Some(position));

        let mut tampered = encoded.into_bytes();
        let index = tampered.len() / 2;
        tampered[index] = if tampered[index] == b'a' { b'b' } else { b'a' };
        let tampered = String::from_utf8(tampered).expect("ASCII cursor");
        assert_eq!(
            decode_cursor(&tampered, &key),
            Err(PageError::InvalidCursor)
        );
        let other_key = PageKey::derive(b"another account");
        assert_eq!(
            decode_cursor(
                &encode_cursor(&binding(), 3, &key).expect("cursor encodes"),
                &other_key
            ),
            Err(PageError::InvalidCursor)
        );

        let mut oversized = binding();
        oversized.query_binding = "q".repeat(4_096);
        assert_eq!(
            encode_cursor(&oversized, 3, &key),
            Err(PageError::ResultEncoding)
        );
    }

    #[test]
    fn pager_returns_first_unreturned_cursor_without_adapting_page_size() {
        let rows = [1_u8, 2, 3, 4];
        let page = fit_page(
            &rows,
            0,
            3,
            |offset| Ok(format!("cursor-{offset}")),
            |selected, cursor| Ok(selected.len() * 10 + cursor.map_or(0, str::len)),
        )
        .expect("page fits");
        assert_eq!(page.rows, [1, 2, 3]);
        assert_eq!(page.next_cursor.as_deref(), Some("cursor-3"));
    }

    #[test]
    fn pager_reports_only_a_single_whole_row_that_cannot_fit() {
        let rows = [1_u8, 2];
        let result = fit_page(
            &rows,
            0,
            2,
            |offset| Ok(format!("cursor-{offset}")),
            |_selected, _cursor| Ok(super::SHARED_RESULT_MAX_BYTES + 1),
        );
        assert_eq!(result, Err(PageError::ResultTooLarge));
    }

    #[test]
    fn pager_measures_large_pages_in_logarithmic_calls() {
        let rows = vec![0_u8; 5_000];
        let mut calls = 0;
        let page = fit_page(
            &rows,
            0,
            5_000,
            |offset| Ok(format!("cursor-{offset}")),
            |selected, _cursor| {
                calls += 1;
                Ok(selected.len() * 300)
            },
        )
        .expect("bounded page fits partially");
        assert!(!page.rows.is_empty());
        assert!(calls <= 16, "binary fitting made {calls} measurements");
    }

    #[test]
    fn pager_uses_an_encodable_earlier_cursor_when_a_later_one_is_too_large() {
        let rows = [0_u8; 21];
        let page = fit_page(
            &rows,
            0,
            20,
            |offset| {
                if offset >= 10 {
                    Err(PageError::ResultTooLarge)
                } else {
                    Ok(format!("cursor-{offset}"))
                }
            },
            |_selected, _cursor| Ok(1),
        )
        .expect("an earlier continuation is publishable");
        assert_eq!(page.rows.len(), 9);
        assert_eq!(page.next_cursor.as_deref(), Some("cursor-9"));
    }
}
