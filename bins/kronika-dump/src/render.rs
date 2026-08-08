//! Printing a segment as a table or as JSON.

use std::fmt::Write as _;

use kronika_index::{Index, OS_PSI_TYPE_ID, points, stalls};
use kronika_reader::{Cell, Dictionary, ReaderError, Resolved, Segment};
use kronika_registry::section_name;

/// Where the output goes, and in which shape.
///
/// JSON is one object per line rather than one array, so a long dump streams
/// and a reader can stop early.
#[derive(Debug)]
pub(crate) struct Output {
    json: bool,
}

impl Output {
    /// Start an output in the requested shape.
    pub(crate) const fn new(json: bool) -> Self {
        Self { json }
    }
}

/// What each section of the segment costs.
pub(crate) fn sizes(out: &Output, segment: &Segment) {
    let total: u64 = segment.sections().map(|(_id, section)| section.bytes).sum();
    if out.json {
        for (type_id, section) in segment.sections() {
            println!(
                r#"{{"segment":{},"type_id":{type_id},"section":{},"rows":{},"bytes":{}}}"#,
                quote(&segment.path().display().to_string()),
                quote(section_name(type_id).unwrap_or("unknown")),
                section.rows,
                section.bytes
            );
        }
        return;
    }
    println!(
        "{}  ts={}..{}  windows={}  section_bytes={total}",
        segment.path().display(),
        segment.min_ts(),
        segment.max_ts(),
        segment.window_count()
    );
    for (type_id, section) in segment.sections() {
        println!(
            "  {type_id:<9} {:<22} rows={:<8} bytes={:<10} {}%",
            section_name(type_id).unwrap_or("unknown"),
            section.rows,
            section.bytes,
            percent(section.bytes, total)
        );
    }
}

/// The health points an index would hold for this segment.
///
/// # Errors
///
/// Returns the reader's error when the pressure section cannot be decoded.
pub(crate) fn index(out: &Output, segment: &Segment) -> Result<(), ReaderError> {
    let built = Index {
        sources: 0,
        points: points(&stalls(&segment.rows(OS_PSI_TYPE_ID)?)),
    };
    if out.json {
        for point in &built.points {
            println!(
                r#"{{"segment":{},"ts":{},"health":{}}}"#,
                quote(&segment.path().display().to_string()),
                point.ts,
                point
                    .health
                    .map_or_else(|| "null".to_owned(), |v| v.to_string())
            );
        }
        return Ok(());
    }
    println!(
        "{}  points={}  idx_bytes={}",
        segment.path().display(),
        built.points.len(),
        kronika_index::HEADER_LEN + built.points.len() * kronika_index::POINT_LEN
    );
    for point in &built.points {
        match point.health {
            Some(health) => println!("  {:<20} {health}", point.ts),
            None => println!("  {:<20} -", point.ts),
        }
    }
    Ok(())
}

/// The rows of one section, with dictionary ids resolved to what they hold.
///
/// # Errors
///
/// Returns the reader's error when the section or the dictionary cannot be
/// decoded.
pub(crate) fn section(
    out: &Output,
    segment: &Segment,
    type_id: u32,
    limit: usize,
) -> Result<(), ReaderError> {
    let rows = segment.rows(type_id)?;
    let dictionary = segment.dictionary()?;
    if !out.json {
        println!(
            "{}  {} ({type_id})  rows={}",
            segment.path().display(),
            section_name(type_id).unwrap_or("unknown"),
            rows.len()
        );
    }
    for row in rows
        .iter()
        .take(if limit == 0 { rows.len() } else { limit })
    {
        let mut fields = Vec::new();
        for (name, cell) in row.iter() {
            fields.push((name, show(cell, &dictionary)));
        }
        if out.json {
            let body: Vec<String> = row
                .iter()
                .map(|(name, cell)| format!("{}:{}", quote(name), json_cell(cell, &dictionary)))
                .collect();
            println!("{{{}}}", body.join(","));
        } else {
            let body: Vec<String> = fields
                .iter()
                .map(|(name, value)| {
                    let shown = value.as_deref().unwrap_or("null");
                    format!("{name}={shown}")
                })
                .collect();
            println!("  {}", body.join(" "));
        }
    }
    if !out.json && limit != 0 && rows.len() > limit {
        println!("  … {} more rows", rows.len() - limit);
    }
    Ok(())
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
            Some(Resolved::Blob(blob)) => blob_text(&blob),
            None => format!("<str {id}>"),
        }),
    }
}

/// A blob, saying how much of it the segment kept.
fn blob_text(blob: &kronika_reader::BlobEntry<'_>) -> String {
    let text = String::from_utf8_lossy(blob.stored_bytes);
    if blob.truncated {
        format!("{text}… (cut, {} bytes in full)", blob.full_len)
    } else {
        text.into_owned()
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
fn json_cell(cell: &Cell, dictionary: &Dictionary) -> String {
    match cell {
        Cell::Null => "null".to_owned(),
        Cell::I16(v) => v.to_string(),
        Cell::I32(v) => v.to_string(),
        Cell::I64(v) | Cell::Ts(v) => v.to_string(),
        Cell::U32(v) => v.to_string(),
        Cell::U64(v) => v.to_string(),
        Cell::F64(v) => v.to_string(),
        Cell::Bool(v) => v.to_string(),
        Cell::ListI32(v) => format!(
            "[{}]",
            v.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Cell::StrId(_id) => {
            show(cell, dictionary).map_or_else(|| "null".to_owned(), |text| quote(&text))
        }
    }
}

/// A JSON string. The characters JSON forbids raw, U+0000 to U+001F along with
/// the quote and the backslash, go out as escapes so one bad log line cannot
/// break the whole dump.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => {
                let _written = write!(out, "\\u{:04x}", other as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests;
