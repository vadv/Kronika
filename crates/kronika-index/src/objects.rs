//! The objects a segment saw, one row each, as the index stores them.
//!
//! A section that declares an identity has objects: one disk, one process, one
//! table. For each of them the index keeps the labels the segment recorded and
//! one number per numeric column — a cumulative column as its delta over the
//! segment, a gauge as its last value. That is what a request for the objects
//! of a section over a window reads, so it never opens a `.zms`.
//!
//! Which label is the identity and which number is a rate is not stored: the
//! registry contract says so, and a second statement of it could only disagree.

use crate::file::IndexError;

/// One number the index kept for an object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    /// An integer column: a counter's delta or a gauge's last reading.
    Int(i64),
    /// A floating column.
    Float(f64),
    /// The segment held nothing to reduce.
    Null,
}

/// Tags of [`Value`] as the block writes them.
const TAG_INT: u8 = 0;
const TAG_FLOAT: u8 = 1;
const TAG_NULL: u8 = 2;

/// One object of one section.
///
/// `labels` holds every label column of the contract in its declared order,
/// rendered as the segment recorded it. `values` holds every numeric column,
/// also in contract order.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    /// Label columns, contract order.
    pub labels: Vec<String>,
    /// Numeric columns, contract order.
    pub values: Vec<Value>,
}

/// The objects of one section.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionObjects {
    /// Which section these objects belong to.
    pub type_id: u32,
    /// Label columns per object.
    pub label_count: u16,
    /// Numeric columns per object.
    pub value_count: u16,
    /// The objects, ordered by their labels.
    pub objects: Vec<Object>,
}

/// Append the objects block body to `out`.
pub(crate) fn encode(sections: &[SectionObjects], out: &mut Vec<u8>) {
    push_u32(out, u32::try_from(sections.len()).unwrap_or(u32::MAX));
    for section in sections {
        push_u32(out, section.type_id);
        out.extend_from_slice(&section.label_count.to_le_bytes());
        out.extend_from_slice(&section.value_count.to_le_bytes());
        push_u32(
            out,
            u32::try_from(section.objects.len()).unwrap_or(u32::MAX),
        );
        for object in &section.objects {
            for label in &object.labels {
                push_u32(out, u32::try_from(label.len()).unwrap_or(u32::MAX));
                out.extend_from_slice(label.as_bytes());
            }
            for value in &object.values {
                match value {
                    Value::Int(number) => {
                        out.push(TAG_INT);
                        out.extend_from_slice(&number.to_le_bytes());
                    }
                    Value::Float(number) => {
                        out.push(TAG_FLOAT);
                        out.extend_from_slice(&number.to_le_bytes());
                    }
                    Value::Null => {
                        out.push(TAG_NULL);
                        out.extend_from_slice(&0_i64.to_le_bytes());
                    }
                }
            }
        }
    }
}

/// Read the objects block body.
pub(crate) fn decode(bytes: &[u8]) -> Result<Vec<SectionObjects>, IndexError> {
    let mut cursor = Cursor::new(bytes);
    let section_count = cursor.u32()?;
    let mut sections = Vec::new();
    for _ in 0..section_count {
        let type_id = cursor.u32()?;
        let label_count = cursor.u16()?;
        let value_count = cursor.u16()?;
        let object_count = cursor.u32()?;
        let mut objects = Vec::new();
        for _ in 0..object_count {
            let mut labels = Vec::with_capacity(label_count.into());
            for _ in 0..label_count {
                labels.push(cursor.text()?);
            }
            let mut values = Vec::with_capacity(value_count.into());
            for _ in 0..value_count {
                values.push(cursor.value()?);
            }
            objects.push(Object { labels, values });
        }
        sections.push(SectionObjects {
            type_id,
            label_count,
            value_count,
            objects,
        });
    }
    if cursor.left() {
        return Err(IndexError::Truncated);
    }
    Ok(sections)
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// A position in the block body. Every read is bounds-checked, so a damaged
/// file is one error rather than a panic.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], IndexError> {
        let end = self.at.checked_add(len).ok_or(IndexError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(IndexError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16, IndexError> {
        let raw = self.take(2)?;
        Ok(u16::from_le_bytes([raw[0], raw[1]]))
    }

    fn u32(&mut self) -> Result<u32, IndexError> {
        let raw = self.take(4)?;
        Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
    }

    fn i64(&mut self) -> Result<i64, IndexError> {
        let raw = self.take(8)?;
        let mut eight = [0_u8; 8];
        eight.copy_from_slice(raw);
        Ok(i64::from_le_bytes(eight))
    }

    fn text(&mut self) -> Result<String, IndexError> {
        let len = self.u32()? as usize;
        let raw = self.take(len)?;
        String::from_utf8(raw.to_vec()).map_err(|_invalid| IndexError::Truncated)
    }

    fn value(&mut self) -> Result<Value, IndexError> {
        let tag = *self.take(1)?.first().ok_or(IndexError::Truncated)?;
        let raw = self.i64()?;
        match tag {
            TAG_INT => Ok(Value::Int(raw)),
            TAG_FLOAT => Ok(Value::Float(f64::from_bits(raw.cast_unsigned()))),
            TAG_NULL => Ok(Value::Null),
            _unknown => Err(IndexError::Truncated),
        }
    }

    const fn left(&self) -> bool {
        self.at != self.bytes.len()
    }
}

#[cfg(test)]
mod tests;
