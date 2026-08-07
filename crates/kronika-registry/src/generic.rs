//! Generic, column-name-addressable section decode.
//!
//! [`decode_rows`] turns any registered section into `Vec<Row>`, where a [`Row`]
//! carries one [`Cell`] per contract column, addressable by column name. This is
//! the primitive the BDD harness uses to assert an arbitrary section's rows by
//! column name, without a per-metric typed struct. `StrId` cells stay as the raw
//! `u64` id; the caller resolves them through the segment dictionary.

use arrow_array::{
    Array, BooleanArray, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array,
    ListArray, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};

use crate::codec::{CodecError, decode_batches};
use crate::contract::{ColumnType, TypeContract};
use crate::{VerifiedSection, registry};

/// One decoded section value, addressed by column name.
///
/// The variants mirror [`ColumnType`]: every on-disk column type has one cell
/// kind. `Ts` carries unix microseconds; `StrId` carries the raw dictionary id
/// (not the resolved bytes). `ListI32` carries list columns such as
/// `pg_locks.blocked_by`. `Null` is a `NULL` cell in a nullable column.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    /// Signed 16-bit integer (also carries `I8`).
    I16(i16),
    /// Signed 32-bit integer.
    I32(i32),
    /// Signed 64-bit integer.
    I64(i64),
    /// Unsigned 32-bit integer (also carries `U8`/`U16`).
    U32(u32),
    /// Unsigned 64-bit integer.
    U64(u64),
    /// 64-bit float (also carries `F32`, widened).
    F64(f64),
    /// Boolean.
    Bool(bool),
    /// Timestamp, unix microseconds.
    Ts(i64),
    /// A dictionary id; resolve through the segment dictionary for the string.
    StrId(u64),
    /// A list of signed 32-bit integers.
    ListI32(Vec<i32>),
    /// A `NULL` in a nullable column.
    Null,
}

/// A decoded section row: cells in contract column order, addressable by name.
///
/// Cells sit in a vector positionally aligned with the contract's columns, so
/// decode is a straight per-column push with no per-cell map insert. Name
/// lookups walk the contract's column list.
#[derive(Debug, Clone)]
pub struct Row {
    /// The contract the row was decoded against; names cells positionally.
    contract: &'static TypeContract,
    /// One cell per contract column, in contract column order.
    cells: Vec<Cell>,
}

impl PartialEq for Row {
    /// Rows are equal when decoded against the same registry contract (compared
    /// by address — contracts are registry statics) with equal cells.
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.contract, other.contract) && self.cells == other.cells
    }
}

impl Row {
    /// Assemble a row of `cells` in `contract` column order.
    ///
    /// The decode path always supplies one cell per column; a shorter vector
    /// leaves the tail columns absent (`get` returns `None`).
    #[must_use]
    pub const fn new(contract: &'static TypeContract, cells: Vec<Cell>) -> Self {
        Self { contract, cells }
    }

    /// The cell under `name`, or `None` when the contract lacks that column.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Cell> {
        let at = self
            .contract
            .columns
            .iter()
            .position(|column| column.name == name)?;
        self.cells.get(at)
    }

    /// The contract this row was decoded against.
    #[must_use]
    pub const fn contract(&self) -> &'static TypeContract {
        self.contract
    }

    /// Cells in contract column order.
    #[must_use]
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// `(column name, cell)` pairs in contract column order.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &Cell)> {
        self.contract
            .columns
            .iter()
            .map(|column| column.name)
            .zip(&self.cells)
    }
}

/// Decode a verified section into generic, column-addressable rows.
///
/// The contract is selected by `type_id`. Each row carries one [`Cell`] per
/// contract column; `StrId` columns keep the raw dictionary id.
///
/// # Errors
///
/// Returns [`CodecError`] for an unknown `type_id`, a schema mismatch, a cap
/// breach, or a Parquet decode failure — the same failures as [`decode_any`].
///
/// [`decode_any`]: crate::decode_any
pub fn decode_rows(type_id: u32, section: VerifiedSection) -> Result<Vec<Row>, CodecError> {
    let contract = registry()
        .iter()
        .find(|contract| contract.type_id.get() == type_id)
        .ok_or(CodecError::UnknownType { type_id })?;
    let bytes_in = section.len();
    decode_rows_with(contract, section).map_err(|source| CodecError::Section {
        type_id,
        bytes_in,
        source: Box::new(source),
    })
}

/// Decode against an explicit contract, without the [`CodecError::Section`] wrap.
fn decode_rows_with(
    contract: &'static TypeContract,
    section: VerifiedSection,
) -> Result<Vec<Row>, CodecError> {
    let decoded = decode_batches(contract, section)?;
    let mut rows: Vec<Row> = Vec::with_capacity(decoded.stats.rows);
    for batch in &decoded.batches {
        let arrays: Vec<&dyn Array> = contract
            .columns
            .iter()
            .map(|column| {
                batch
                    .column_by_name(column.name)
                    .map(AsRef::as_ref)
                    .ok_or(CodecError::MissingColumn { name: column.name })
            })
            .collect::<Result<_, _>>()?;
        for i in 0..batch.num_rows() {
            let mut cells = Vec::with_capacity(contract.columns.len());
            for (column, array) in contract.columns.iter().zip(&arrays) {
                cells.push(cell_at(*array, column.ty, column.name, i)?);
            }
            rows.push(Row { contract, cells });
        }
    }
    Ok(rows)
}

/// Read cell `i` of `array` as a [`Cell`], per the column's [`ColumnType`].
fn cell_at(
    array: &dyn Array,
    ty: ColumnType,
    name: &'static str,
    i: usize,
) -> Result<Cell, CodecError> {
    if array.is_null(i) {
        return Ok(Cell::Null);
    }
    let cell = match ty {
        ColumnType::I8 => Cell::I16(i16::from(typed::<Int8Array>(array, name)?.value(i))),
        ColumnType::I16 => Cell::I16(typed::<Int16Array>(array, name)?.value(i)),
        ColumnType::I32 => Cell::I32(typed::<Int32Array>(array, name)?.value(i)),
        ColumnType::I64 => Cell::I64(typed::<Int64Array>(array, name)?.value(i)),
        ColumnType::U8 => Cell::U32(u32::from(typed::<UInt8Array>(array, name)?.value(i))),
        ColumnType::U16 => Cell::U32(u32::from(typed::<UInt16Array>(array, name)?.value(i))),
        ColumnType::U32 => Cell::U32(typed::<UInt32Array>(array, name)?.value(i)),
        ColumnType::U64 => Cell::U64(typed::<UInt64Array>(array, name)?.value(i)),
        ColumnType::F32 => Cell::F64(f64::from(typed::<Float32Array>(array, name)?.value(i))),
        ColumnType::F64 => Cell::F64(typed::<Float64Array>(array, name)?.value(i)),
        ColumnType::Bool => Cell::Bool(typed::<BooleanArray>(array, name)?.value(i)),
        ColumnType::Ts => Cell::Ts(typed::<Int64Array>(array, name)?.value(i)),
        ColumnType::StrId => Cell::StrId(typed::<UInt64Array>(array, name)?.value(i)),
        ColumnType::ListI32 => Cell::ListI32(list_i32_at(array, name, i)?),
    };
    Ok(cell)
}

/// Read one `List<Int32>` row into an owned vector.
fn list_i32_at(array: &dyn Array, name: &'static str, i: usize) -> Result<Vec<i32>, CodecError> {
    let lists = typed::<ListArray>(array, name)?;
    let values = lists.value(i);
    let ints = values
        .as_any()
        .downcast_ref::<Int32Array>()
        .ok_or(CodecError::ColumnType { name })?;
    let mut out = Vec::with_capacity(ints.len());
    for j in 0..ints.len() {
        if ints.is_null(j) {
            return Err(CodecError::NullInRequiredColumn { name });
        }
        out.push(ints.value(j));
    }
    Ok(out)
}

/// Downcast `array` to the concrete Arrow array the column type maps to.
fn typed<'a, A: Array + 'static>(
    array: &'a dyn Array,
    name: &'static str,
) -> Result<&'a A, CodecError> {
    array
        .as_any()
        .downcast_ref::<A>()
        .ok_or(CodecError::ColumnType { name })
}

#[cfg(test)]
mod tests;
