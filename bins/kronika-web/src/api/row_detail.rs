//! One exact physical row and one bounded dictionary-value chunk.

use std::collections::HashSet;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use kronika_reader::{Cell, Dictionary, Resolved, Row, Segment, SegmentKind, SegmentRef};
use kronika_registry::{
    Column, ColumnClass, ColumnType, TypeContract, contract, logical_section_name,
};
use serde_json::{Value, json};

use super::render::{cell, hex, projected_layout};
use super::{ApiError, explicit_segment};

const CURSOR_VERSION: &str = "row-detail-v1";
const MAX_CHUNK_BYTES: usize = 32 * 1_024;

/// Exact physical coordinate and bounded projection for one recorded row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowDetailRequest {
    pub(crate) segment_id: i64,
    pub(crate) type_id: u32,
    pub(crate) row_ordinal: u64,
    pub(crate) timestamp_us: i64,
    pub(crate) fields: Vec<String>,
    pub(crate) text_field: Option<String>,
    /// Explicit caller offset. A continuation cursor supplies it when absent.
    pub(crate) byte_offset: Option<u64>,
    pub(crate) byte_limit: usize,
    pub(crate) cursor: Option<String>,
}

/// One exact projected row plus an optional lossless dictionary-value chunk.
#[derive(Debug, PartialEq)]
pub(crate) struct RowDetail {
    pub(crate) row: Value,
    pub(crate) layout: Value,
    pub(crate) text_chunk: Option<Value>,
    pub(crate) next_cursor: Option<String>,
    pub(crate) source_truncated: bool,
    pub(crate) active_position: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cursor {
    segment_id: i64,
    active_position: u64,
    byte_offset: u64,
    binding: u64,
}

struct Selection<'request> {
    layout: &'static TypeContract,
    logical_name: &'static str,
    timestamp: &'static Column,
    fields: Vec<(&'request str, &'static Column)>,
    text_column: Option<&'static Column>,
    projection: Vec<&'static str>,
}

/// Read one row only when its complete physical locator still matches.
///
/// Text is resolved only for the requested `StrId` and sliced after the
/// locator check. A continuation cursor pins the active WAL prefix and binds
/// the locator, row projection, text field, and chunk size.
pub(crate) fn read_row_detail(
    root: &Path,
    request: &RowDetailRequest,
) -> Result<RowDetail, ApiError> {
    let (binding, cursor) = validate_request(request)?;
    let (reader, current) = explicit_segment(root, request.segment_id)?;
    let reference = pin(current, cursor)?;
    let segment = reader.open_segment(&reference)?;
    let active_position = segment.active_position();
    if cursor.is_some_and(|cursor| cursor.active_position != active_position.unwrap_or(0)) {
        return Err(ApiError::BadCursor);
    }
    let selection = select_columns(request)?;
    let row = locate_row(&segment, request, &selection)?;
    let (row_value, layout_value) = render_row(request, &row, &selection)?;
    let Some(column) = selection.text_column else {
        return Ok(RowDetail {
            row: row_value,
            layout: layout_value,
            text_chunk: None,
            next_cursor: None,
            source_truncated: false,
            active_position,
        });
    };
    let offset = match cursor {
        Some(cursor) => {
            if request
                .byte_offset
                .is_some_and(|offset| offset != cursor.byte_offset)
            {
                return Err(ApiError::BadCursor);
            }
            cursor.byte_offset
        }
        None => request.byte_offset.unwrap_or(0),
    };
    let chunk = resolve_chunk(&segment, &row, column, offset, request.byte_limit)?;
    let next_cursor = chunk.more_stored_bytes.then(|| {
        Cursor {
            segment_id: request.segment_id,
            active_position: active_position.unwrap_or(0),
            byte_offset: chunk.next_offset,
            binding,
        }
        .encode()
    });
    Ok(RowDetail {
        row: row_value,
        layout: layout_value,
        text_chunk: Some(chunk.value),
        next_cursor,
        source_truncated: chunk.source_truncated,
        active_position,
    })
}

fn validate_request(request: &RowDetailRequest) -> Result<(u64, Option<Cursor>), ApiError> {
    if request.byte_limit == 0 || request.byte_limit > MAX_CHUNK_BYTES {
        return Err(ApiError::BadFilter("byte_limit".to_owned()));
    }
    if request
        .byte_offset
        .is_some_and(|offset| offset > u64::from(u32::MAX))
    {
        return Err(ApiError::BadFilter("byte_offset".to_owned()));
    }
    if request.text_field.is_none() && request.byte_offset.is_some() {
        return Err(ApiError::BadFilter("byte_offset".to_owned()));
    }
    let binding = binding(request);
    let cursor = request
        .cursor
        .as_deref()
        .map(Cursor::parse)
        .transpose()?
        .filter(|cursor| cursor.segment_id == request.segment_id && cursor.binding == binding);
    if request.cursor.is_some() && cursor.is_none() {
        return Err(ApiError::BadCursor);
    }
    if cursor.is_some() && request.text_field.is_none() {
        return Err(ApiError::BadCursor);
    }
    Ok((binding, cursor))
}

fn select_columns(request: &RowDetailRequest) -> Result<Selection<'_>, ApiError> {
    let layout = contract(request.type_id).ok_or(ApiError::NoSuchSection)?;
    let logical_name = logical_section_name(request.type_id).ok_or(ApiError::NoSuchSection)?;
    let timestamp = layout
        .columns
        .iter()
        .find(|column| column.class == ColumnClass::Timestamp)
        .ok_or(ApiError::BadCursor)?;
    let mut names = HashSet::new();
    let fields = request
        .fields
        .iter()
        .map(|name| {
            if !names.insert(name.as_str()) {
                return Err(ApiError::BadFilter("fields".to_owned()));
            }
            let column = layout
                .column(name)
                .ok_or_else(|| ApiError::NoSuchColumn(name.clone()))?;
            if column.ty == ColumnType::StrId {
                return Err(ApiError::BadFilter("fields".to_owned()));
            }
            Ok((name.as_str(), column))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let text_column = request
        .text_field
        .as_deref()
        .map(|name| {
            let column = layout
                .column(name)
                .ok_or_else(|| ApiError::NoSuchColumn(name.to_owned()))?;
            if column.ty != ColumnType::StrId {
                return Err(ApiError::BadFilter("text_field".to_owned()));
            }
            Ok(column)
        })
        .transpose()?;
    let mut projection = Vec::with_capacity(fields.len().saturating_add(2));
    projection.push(timestamp.name);
    for (_name, column) in &fields {
        if !projection.contains(&column.name) {
            projection.push(column.name);
        }
    }
    if let Some(column) = text_column
        && !projection.contains(&column.name)
    {
        projection.push(column.name);
    }
    Ok(Selection {
        layout,
        logical_name,
        timestamp,
        fields,
        text_column,
        projection,
    })
}

fn locate_row(
    segment: &Segment,
    request: &RowDetailRequest,
    selection: &Selection<'_>,
) -> Result<Row, ApiError> {
    let rows = segment
        .rows_of(request.type_id)
        .ok_or(ApiError::NoSuchSection)?;
    if request.row_ordinal >= rows {
        return Err(ApiError::BadCursor);
    }
    let mut located = None;
    segment.visit_rows(
        request.type_id,
        &selection.projection,
        request.row_ordinal,
        1,
        |ordinal, row| {
            if ordinal == request.row_ordinal
                && row_timestamp(&row, selection.timestamp.name) == Some(request.timestamp_us)
            {
                located = Some(row);
            }
            false
        },
    )?;
    located.ok_or(ApiError::BadCursor)
}

fn render_row(
    request: &RowDetailRequest,
    row: &Row,
    selection: &Selection<'_>,
) -> Result<(Value, Value), ApiError> {
    let dictionary = Dictionary::default();
    let values = selection
        .fields
        .iter()
        .map(|(_name, column)| {
            row.get(column.name)
                .map_or(Ok(Value::Null), |stored| cell(stored, &dictionary))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let row_value = json!({
        "record": "row",
        "type_id": request.type_id.to_string(),
        "ordinal": request.row_ordinal.to_string(),
        "segment_id": request.segment_id.to_string(),
        "timestamp": request.timestamp_us.to_string(),
        "values": values,
    });
    let projected = selection
        .fields
        .iter()
        .map(|(name, column)| (*name, Some(*column)))
        .collect::<Vec<_>>();
    Ok((
        row_value,
        projected_layout(selection.logical_name, selection.layout, &projected),
    ))
}

fn resolve_chunk(
    segment: &Segment,
    row: &Row,
    column: &Column,
    offset: u64,
    limit: usize,
) -> Result<Chunk, ApiError> {
    let stored = row.get(column.name).ok_or_else(|| {
        ApiError::Unreadable(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("projected row lacks dictionary field {:?}", column.name),
        )))
    })?;
    match stored {
        Cell::Null => Chunk::null(column.name, offset),
        Cell::StrId(id) => {
            let dictionary = segment.dictionary_for(&HashSet::from([*id]))?;
            let resolved = dictionary.resolve(*id).ok_or_else(|| {
                ApiError::Unreadable(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unresolved dictionary id {id}"),
                )))
            })?;
            Chunk::from_resolved(column.name, *id, offset, limit, resolved)
        }
        _other => Err(ApiError::Unreadable(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "dictionary field {:?} decoded with another type",
                column.name
            ),
        )))),
    }
}

struct Chunk {
    value: Value,
    next_offset: u64,
    more_stored_bytes: bool,
    source_truncated: bool,
}

impl Chunk {
    fn null(field: &str, offset: u64) -> Result<Self, ApiError> {
        if offset != 0 {
            return Err(ApiError::BadFilter("byte_offset".to_owned()));
        }
        Ok(Self {
            value: json!({
                "field": field,
                "str_id": Value::Null,
                "representation": Value::Null,
                "utf8": Value::Null,
                "base64": Value::Null,
                "byte_offset": "0",
                "byte_len": "0",
                "stored_len": Value::Null,
                "source_full_len": Value::Null,
                "chunk_truncated": false,
                "source_truncated": false,
                "source_sha256": Value::Null,
                "is_null": true,
            }),
            next_offset: 0,
            more_stored_bytes: false,
            source_truncated: false,
        })
    }

    fn from_resolved(
        field: &str,
        str_id: u64,
        offset: u64,
        limit: usize,
        resolved: Resolved<'_>,
    ) -> Result<Self, ApiError> {
        let (storage, stored, source_full_len, source_truncated, source_sha256) = match resolved {
            Resolved::Str(bytes) => (
                "string",
                bytes,
                u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                false,
                None,
            ),
            Resolved::Blob(blob) => (
                "blob",
                blob.stored_bytes,
                blob.full_len,
                blob.truncated,
                blob.full_sha256,
            ),
        };
        let offset_usize = usize::try_from(offset)
            .ok()
            .filter(|offset| *offset <= stored.len())
            .ok_or_else(|| ApiError::BadFilter("byte_offset".to_owned()))?;
        let end = offset_usize.saturating_add(limit).min(stored.len());
        let bytes = &stored[offset_usize..end];
        let (representation, utf8, base64) = match std::str::from_utf8(bytes) {
            Ok(text) => ("utf8", Value::String(text.to_owned()), Value::Null),
            Err(_invalid) => ("base64", Value::Null, Value::String(STANDARD.encode(bytes))),
        };
        let stored_len = u64::try_from(stored.len()).unwrap_or(u64::MAX);
        let next_offset = u64::try_from(end).unwrap_or(u64::MAX);
        let more_stored_bytes = end < stored.len();
        Ok(Self {
            value: json!({
                "field": field,
                "str_id": str_id.to_string(),
                "storage": storage,
                "representation": representation,
                "utf8": utf8,
                "base64": base64,
                "byte_offset": offset.to_string(),
                "byte_len": bytes.len().to_string(),
                "stored_len": stored_len.to_string(),
                "source_full_len": source_full_len.to_string(),
                "chunk_truncated": more_stored_bytes,
                "source_truncated": source_truncated,
                "source_sha256": source_sha256.map(|hash| hex(&hash)),
                "is_null": false,
            }),
            next_offset,
            more_stored_bytes,
            source_truncated,
        })
    }
}

impl Cursor {
    fn parse(raw: &str) -> Result<Self, ApiError> {
        let fields = raw.split(',').collect::<Vec<_>>();
        if fields.len() != 5 || fields[0] != CURSOR_VERSION {
            return Err(ApiError::BadCursor);
        }
        Ok(Self {
            segment_id: fields[1].parse().map_err(|_error| ApiError::BadCursor)?,
            active_position: fields[2].parse().map_err(|_error| ApiError::BadCursor)?,
            byte_offset: fields[3].parse().map_err(|_error| ApiError::BadCursor)?,
            binding: fields[4].parse().map_err(|_error| ApiError::BadCursor)?,
        })
    }

    fn encode(self) -> String {
        format!(
            "{CURSOR_VERSION},{},{},{},{}",
            self.segment_id, self.active_position, self.byte_offset, self.binding
        )
    }
}

fn pin(current: SegmentRef, cursor: Option<Cursor>) -> Result<SegmentRef, ApiError> {
    let Some(cursor) = cursor else {
        return Ok(current);
    };
    match current.kind() {
        SegmentKind::Finished if cursor.active_position == 0 => Ok(current),
        SegmentKind::Active => current
            .at_active_position(cursor.active_position)
            .map_err(|_error| ApiError::BadCursor),
        SegmentKind::Finished => Err(ApiError::BadCursor),
    }
}

fn binding(request: &RowDetailRequest) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    hash_part(&mut hash, b"segment", &request.segment_id.to_le_bytes());
    hash_part(&mut hash, b"type", &request.type_id.to_le_bytes());
    hash_part(&mut hash, b"ordinal", &request.row_ordinal.to_le_bytes());
    hash_part(&mut hash, b"timestamp", &request.timestamp_us.to_le_bytes());
    for field in &request.fields {
        hash_part(&mut hash, b"field", field.as_bytes());
    }
    if let Some(text_field) = request.text_field.as_deref() {
        hash_part(&mut hash, b"text-field", text_field.as_bytes());
    }
    let byte_limit = u64::try_from(request.byte_limit).unwrap_or(u64::MAX);
    hash_part(&mut hash, b"byte-limit", &byte_limit.to_le_bytes());
    hash
}

fn hash_part(hash: &mut u64, tag: &[u8], bytes: &[u8]) {
    hash_bytes(hash, tag);
    hash_bytes(hash, bytes);
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes.len().to_le_bytes().iter().chain(bytes) {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn row_timestamp(row: &Row, column: &str) -> Option<i64> {
    match row.get(column) {
        Some(Cell::Ts(stored)) => Some(*stored),
        _other => None,
    }
}

#[cfg(test)]
mod tests;
