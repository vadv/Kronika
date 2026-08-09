//! Printing a segment as a table or as JSON.

use std::io::Write;

use kronika_index::{DERIVED_HEALTH_TYPE_ID, IdentityValue, Number, Observation, Sample};
use kronika_reader::{Cell, Dictionary, Resolved, Segment, StoreWarning};
use kronika_registry::{DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, section_name};
use serde_json::{Map, Value, json};

use crate::DumpError;

/// A file the scan would not admit.
///
/// It goes to the same stream as everything else under `--json`: a caller that
/// only reads stdout still learns that something was left out.
pub(crate) fn warning(
    output: &mut impl Write,
    json_output: bool,
    warning: &StoreWarning,
) -> Result<(), DumpError> {
    if json_output {
        say(
            output,
            &json!({"kind": "warning", "detail": format!("{warning:?}")}),
        )?;
    } else {
        eprintln!("kronika-dump: set aside {warning:?}");
    }
    Ok(())
}

/// What each section of the segment costs.
pub(crate) fn sizes(
    output: &mut impl Write,
    json_output: bool,
    segment: &Segment,
) -> Result<(), DumpError> {
    let section_bytes: u64 = segment.sections().map(|(_id, section)| section.bytes).sum();
    let captured_bytes = segment.captured_bytes();
    let overhead_bytes = captured_bytes.checked_sub(section_bytes).ok_or_else(|| {
        kronika_reader::ReaderError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("section bodies use {section_bytes} bytes in a {captured_bytes}-byte segment"),
        ))
    })?;
    if json_output {
        let path = segment.path().display().to_string();
        say(
            output,
            &json!({
                "kind": "segment",
                "path": path,
                "min_ts": segment.min_ts(),
                "max_ts": segment.max_ts(),
                "windows": segment.window_count(),
                "captured_bytes": captured_bytes,
                "section_bytes": section_bytes,
                "overhead_bytes": overhead_bytes,
            }),
        )?;
        for (type_id, section) in segment.sections() {
            say(
                output,
                &json!({
                    "kind": "section",
                    "path": path,
                    "type_id": type_id,
                    "section": section_name(type_id).unwrap_or("unknown"),
                    "rows": section.rows,
                    "bytes": section.bytes,
                    "share_percent": percent(section.bytes, captured_bytes),
                }),
            )?;
        }
        say(
            output,
            &json!({
                "kind": "overhead",
                "path": path,
                "bytes": overhead_bytes,
                "share_percent": percent(overhead_bytes, captured_bytes),
            }),
        )?;
        return Ok(());
    }
    writeln!(
        output,
        "{}  ts={}..{}  windows={}  captured_bytes={captured_bytes}  section_bytes={section_bytes}  overhead_bytes={overhead_bytes}",
        segment.path().display(),
        segment.min_ts(),
        segment.max_ts(),
        segment.window_count()
    )?;
    for (type_id, section) in segment.sections() {
        writeln!(
            output,
            "  {type_id:<9} {:<22} rows={:<8} bytes={:<10} {}%",
            section_name(type_id).unwrap_or("unknown"),
            section.rows,
            section.bytes,
            percent(section.bytes, captured_bytes)
        )?;
    }
    writeln!(
        output,
        "  {:<9} {:<22} rows={:<8} bytes={:<10} {}%",
        "-",
        "physical overhead",
        "-",
        overhead_bytes,
        percent(overhead_bytes, captured_bytes)
    )?;
    Ok(())
}

/// The index this segment would get: exact identities and bounded numeric
/// observations for each physical layout, including derived health.
///
/// # Errors
///
/// Returns a build, reader, or index-encoding error when the segment cannot be
/// summarized exactly.
pub(crate) fn index(
    output: &mut impl Write,
    json_output: bool,
    segment: &Segment,
) -> Result<(), DumpError> {
    let built = kronika_index::build(segment, 0)?;
    let path = segment.path().display().to_string();
    if json_output {
        let mut write_error = None;
        kronika_index::visit_health_points(
            segment,
            || true,
            |point| match say(
                output,
                &json!({
                    "kind": "point",
                    "path": path,
                    "ts": point.timestamp.to_string(),
                    "health": point.value,
                }),
            ) {
                Ok(()) => true,
                Err(error) => {
                    write_error = Some(error);
                    false
                }
            },
        )?;
        if let Some(error) = write_error {
            return Err(error);
        }
        for section in &built.sections {
            for object in &section.objects {
                say(
                    output,
                    &json!({
                        "kind": "object",
                        "path": path,
                        "type_id": section.type_id,
                        "section": index_section_name(section.type_id),
                        "identity": object.identity.iter().map(index_identity).collect::<Vec<_>>(),
                        "observations": object
                            .observations
                            .iter()
                            .map(index_observation)
                            .collect::<Vec<_>>(),
                    }),
                )?;
            }
        }
        return Ok(());
    }
    let encoded_bytes = built.encode()?.len();
    let object_count = built
        .sections
        .iter()
        .map(|section| section.objects.len())
        .sum::<usize>();
    let series_count = built
        .sections
        .iter()
        .flat_map(|section| &section.objects)
        .map(|object| object.observations.len())
        .sum::<usize>();
    writeln!(
        output,
        "{path}  sections={}  objects={object_count}  series={series_count}  idx_bytes={encoded_bytes}",
        built.sections.len(),
    )?;
    for section in &built.sections {
        let observations = section
            .objects
            .iter()
            .map(|object| object.observations.len())
            .sum::<usize>();
        let samples = section
            .objects
            .iter()
            .flat_map(|object| &object.observations)
            .map(|observation| observation.count)
            .fold(0_u64, u64::saturating_add);
        writeln!(
            output,
            "  {:<9} {:<22} objects={:<8} series={:<8} samples={}",
            section.type_id,
            index_section_name(section.type_id),
            section.objects.len(),
            observations,
            samples,
        )?;
    }
    Ok(())
}

fn index_section_name(type_id: u32) -> &'static str {
    if type_id == DERIVED_HEALTH_TYPE_ID {
        "health"
    } else {
        section_name(type_id).unwrap_or("unknown")
    }
}

/// One exact index identity as JSON. Wide integers and timestamps use decimal
/// strings so JavaScript consumers do not silently round them.
fn index_identity(value: &IdentityValue) -> Value {
    match value {
        IdentityValue::Null => Value::Null,
        IdentityValue::I16(number) => json!(number),
        IdentityValue::I32(number) => json!(number),
        IdentityValue::I64(number) | IdentityValue::Ts(number) => Value::String(number.to_string()),
        IdentityValue::U32(number) => json!(number),
        IdentityValue::U64(number) => Value::String(number.to_string()),
        IdentityValue::F64(number) => index_float(*number),
        IdentityValue::Bool(value) => json!(value),
        IdentityValue::Text(bytes) => index_bytes(bytes),
        IdentityValue::Blob {
            stored_bytes,
            full_len,
            truncated,
            full_sha256,
        } => json!({
            "representation": "blob",
            "stored_bytes": index_bytes(stored_bytes),
            "full_len": full_len.to_string(),
            "truncated": truncated,
            "full_sha256": full_sha256.map(|hash| index_hex(&hash)),
        }),
        IdentityValue::ListI32(values) => json!(values),
    }
}

fn index_observation(observation: &Observation) -> Value {
    json!({
        "count": observation.count.to_string(),
        "first": observation.first.map(index_sample),
        "last": observation.last.map(index_sample),
        "nonnegative_delta": observation.nonnegative_delta.map(index_number),
        "observed_us": observation.observed_us.to_string(),
    })
}

fn index_sample(sample: Sample) -> Value {
    json!({
        "ts": sample.ts.to_string(),
        "value": index_number(sample.value),
    })
}

fn index_number(number: Number) -> Value {
    match number {
        Number::I16(number) => json!(number),
        Number::I32(number) => json!(number),
        Number::I64(number) => Value::String(number.to_string()),
        Number::U32(number) => json!(number),
        Number::U64(number) => Value::String(number.to_string()),
        Number::F64(number) => index_float(number),
    }
}

fn index_float(number: f64) -> Value {
    serde_json::Number::from_f64(number).map_or_else(
        || {
            json!({
                "representation": "nonfinite_f64",
                "bits": number.to_bits().to_string(),
            })
        },
        Value::Number,
    )
}

fn index_bytes(bytes: &[u8]) -> Value {
    std::str::from_utf8(bytes).map_or_else(
        |_invalid| {
            json!({
                "representation": "bytes",
                "bytes": bytes,
            })
        },
        |text| Value::String(text.to_owned()),
    )
}

fn index_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// The rows of one section, with dictionary ids resolved to what they hold.
///
/// # Errors
///
/// Returns the reader's error when the section or the dictionary cannot be
/// decoded.
pub(crate) fn section(
    output: &mut impl Write,
    json_output: bool,
    segment: &Segment,
    type_id: u32,
    limit: usize,
) -> Result<(), DumpError> {
    let dictionary = segment.dictionary()?;
    if matches!(type_id, DICT_STRINGS_TYPE_ID | DICT_BLOBS_TYPE_ID) {
        return dictionary_section(output, json_output, segment, type_id, limit, &dictionary);
    }

    let rows = segment.rows(type_id)?;
    if !json_output {
        writeln!(
            output,
            "{}  {} ({type_id})  rows={}",
            segment.path().display(),
            section_name(type_id).unwrap_or("unknown"),
            rows.len()
        )?;
    }
    for row in rows
        .iter()
        .take(if limit == 0 { rows.len() } else { limit })
    {
        if json_output {
            let object: Map<String, Value> = row
                .iter()
                .map(|(name, cell)| ((*name).to_owned(), json_cell(cell, &dictionary)))
                .collect();
            write_json_row(output, segment.path(), type_id, &Value::Object(object))?;
        } else {
            let body: Vec<String> = row
                .iter()
                .map(|(name, cell)| {
                    let value = show(cell, &dictionary);
                    let shown = value.as_deref().unwrap_or("null");
                    format!("{name}={shown}")
                })
                .collect();
            writeln!(output, "  {}", body.join(" "))?;
        }
    }
    if !json_output && limit != 0 && rows.len() > limit {
        writeln!(output, "  … {} more rows", rows.len() - limit)?;
    }
    Ok(())
}

fn dictionary_section(
    output: &mut impl Write,
    json_output: bool,
    segment: &Segment,
    type_id: u32,
    limit: usize,
    dictionary: &Dictionary,
) -> Result<(), DumpError> {
    let mut entries: Vec<_> = dictionary
        .entries()
        .filter(|(_id, resolved)| {
            matches!(
                (type_id, *resolved),
                (DICT_STRINGS_TYPE_ID, Resolved::Str(_)) | (DICT_BLOBS_TYPE_ID, Resolved::Blob(_))
            )
        })
        .collect();
    entries.sort_unstable_by_key(|(id, _resolved)| *id);
    if !json_output {
        writeln!(
            output,
            "{}  {} ({type_id})  rows={}",
            segment.path().display(),
            section_name(type_id).unwrap_or("unknown"),
            entries.len()
        )?;
    }
    let shown = if limit == 0 { entries.len() } else { limit };
    for (id, resolved) in entries.iter().take(shown) {
        if json_output {
            let row = dictionary_json(*id, *resolved);
            write_json_row(output, segment.path(), type_id, &row)?;
        } else {
            match resolved {
                Resolved::Str(bytes) => {
                    writeln!(output, "  str_id={id} bytes={bytes:?}")?;
                }
                Resolved::Blob(blob) => {
                    writeln!(
                        output,
                        "  str_id={id} stored_bytes={:?} full_len={} truncated={} full_sha256={:?}",
                        blob.stored_bytes, blob.full_len, blob.truncated, blob.full_sha256
                    )?;
                }
            }
        }
    }
    if !json_output && limit != 0 && entries.len() > limit {
        writeln!(output, "  … {} more rows", entries.len() - limit)?;
    }
    Ok(())
}

fn dictionary_json(id: u64, resolved: Resolved<'_>) -> Value {
    match resolved {
        Resolved::Str(bytes) => json!({
            "str_id": id,
            "bytes": bytes,
        }),
        Resolved::Blob(blob) => json!({
            "str_id": id,
            "stored_bytes": blob.stored_bytes,
            "full_len": blob.full_len,
            "truncated": blob.truncated,
            "full_sha256": blob.full_sha256,
        }),
    }
}

fn write_json_row(
    output: &mut impl Write,
    path: &std::path::Path,
    type_id: u32,
    row: &Value,
) -> Result<(), DumpError> {
    say(
        output,
        &json!({
            "kind": "row",
            "path": path.display().to_string(),
            "type_id": type_id,
            "row": row,
        }),
    )
}

/// One cell as text. A dictionary id becomes what the segment interned under
/// it; a blob that was stored cut says so rather than passing for whole.
fn show(cell: &Cell, dictionary: &Dictionary) -> Option<String> {
    match cell {
        Cell::Null => None,
        Cell::I16(v) => Some(v.to_string()),
        Cell::I32(v) => Some(v.to_string()),
        Cell::I64(v) | Cell::Ts(v) => Some(v.to_string()),
        Cell::U32(v) => Some(v.to_string()),
        Cell::U64(v) => Some(v.to_string()),
        Cell::F64(v) => Some(v.to_string()),
        Cell::Bool(v) => Some(v.to_string()),
        Cell::ListI32(v) => Some(format!("{v:?}")),
        Cell::StrId(id) => Some(match dictionary.resolve(*id) {
            Some(Resolved::Str(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
            Some(Resolved::Blob(blob)) => {
                let text = String::from_utf8_lossy(blob.stored_bytes);
                if blob.truncated {
                    format!("{text}… (cut, {} bytes in full)", blob.full_len)
                } else {
                    text.into_owned()
                }
            }
            None => format!("<str {id}>"),
        }),
    }
}

/// `part` as a whole percent of `whole`.
///
/// The arithmetic widens rather than saturates: a section is small next to a
/// segment, but a figure that quietly collapsed to 1% would be worse than one
/// that costs a `u128` multiply.
fn percent(part: u64, whole: u64) -> u8 {
    if whole == 0 {
        return 0;
    }
    let part = part.min(whole);
    let scaled = u128::from(part) * 100 + u128::from(whole) / 2;
    u8::try_from(scaled / u128::from(whole)).unwrap_or(100)
}

/// One cell as JSON. Numbers stay numbers so a dump can be filtered on them;
/// a dictionary id becomes the text it stands for.
fn json_cell(cell: &Cell, dictionary: &Dictionary) -> Value {
    match cell {
        Cell::Null => Value::Null,
        Cell::I16(v) => json!(v),
        Cell::I32(v) => json!(v),
        Cell::I64(v) | Cell::Ts(v) => json!(v),
        Cell::U32(v) => json!(v),
        Cell::U64(v) => json!(v),
        Cell::F64(v) => json!(v),
        Cell::Bool(v) => json!(v),
        Cell::ListI32(v) => json!(v),
        Cell::StrId(_id) => show(cell, dictionary).map_or(Value::Null, Value::String),
    }
}

/// One JSON document per line, so a long dump streams.
fn say(output: &mut impl Write, value: &Value) -> Result<(), DumpError> {
    writeln!(output, "{value}").map_err(DumpError::Output)
}

#[cfg(test)]
mod tests;
