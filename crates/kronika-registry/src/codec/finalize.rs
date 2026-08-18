//! Bounded coalescing of journal sections into one finished Parquet body.

use std::cmp::Ordering;
use std::io::Write;
use std::ops::Range;
use std::sync::Arc;

use arrow_array::{ArrayRef, RecordBatchReader, UInt32Array};
use arrow_ord::sort::{LexicographicalComparator, SortColumn};
use arrow_select::concat::concat;
use arrow_select::interleave::interleave;
use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::arrow::arrow_writer::{ArrowColumnWriter, compute_leaves, get_column_writers};
use parquet::arrow::{ArrowSchemaConverter, ProjectionMask};
use parquet::file::reader::ChunkReader;
use parquet::file::writer::{SerializedFileWriter, SerializedRowGroupWriter};

use super::{
    CodecError, ColumnType, DECODE_BATCH_SIZE, FINAL_WRITER_PROPS, MAX_LIST_I32_VALUES_PER_SECTION,
    MAX_ROW_GROUPS, MAX_SECTION_BYTES, MAX_SECTION_ROWS, TypeContract, VerifiedSection,
    arrow_schema, check_row_cap, final_data_body_bound, schema_matches, validate_list_i32_batch,
};

/// Validate one input body before the bounded finalizer reopens it by range.
///
/// This consumes and releases the compressed body after checking its CRC was
/// already verified, its bounded Parquet profile, schema, and declared rows.
///
/// # Errors
///
/// Returns [`CodecError`] when the type is unknown or the section violates
/// its bounded Parquet, schema, or row-count contract.
pub fn validate_final_section(
    type_id: u32,
    section: VerifiedSection,
    expected_rows: u32,
) -> Result<(), CodecError> {
    let contract = contract(type_id)?;
    let (reader, _row_groups, rows) = super::decode::capped_reader(section.into_bytes())?;
    if !schema_matches(&reader.schema(), contract) {
        return Err(CodecError::SchemaMismatch);
    }
    if rows != expected_rows as usize {
        return Err(CodecError::RowCountMismatch {
            expected: u64::from(expected_rows),
            got: rows as u64,
        });
    }
    Ok(())
}

/// Encode validated input sections into one final Parquet body.
///
/// `open` returns a fresh bounded random-access view of one compressed input
/// body. The finalizer retains at most one decoded column across the input
/// sections, plus compact row order and one output chunk.
///
/// # Errors
///
/// Returns `E` when opening an input fails, the input violates its registered
/// contract, canonical ordering fails, or the final Parquet body cannot be
/// written.
pub fn encode_final_sections_to<W, R, E>(
    type_id: u32,
    expected_rows: &[u32],
    out: &mut W,
    mut open: impl FnMut(usize) -> Result<R, E>,
) -> Result<(), E>
where
    W: Write + Send,
    R: ChunkReader + 'static,
    E: From<CodecError>,
{
    let contract = contract(type_id)?;
    let rows = aggregate_rows(expected_rows)?;
    if rows == 0 {
        return Err(CodecError::SchemaMismatch.into());
    }

    let order = canonical_order(expected_rows, contract, &mut open)?;
    let schema = arrow_schema(contract);
    let properties = Arc::new(FINAL_WRITER_PROPS.clone());
    let parquet_schema = ArrowSchemaConverter::new()
        .with_coerce_types(properties.coerce_types())
        .convert(&schema)
        .map_err(CodecError::from)?;
    let column_writers =
        get_column_writers(&parquet_schema, &properties, &schema).map_err(CodecError::from)?;
    let mut file = SerializedFileWriter::new(
        out,
        parquet_schema.root_schema_ptr(),
        Arc::clone(&properties),
    )
    .map_err(CodecError::from)?;
    let mut row_group = file.next_row_group().map_err(CodecError::from)?;
    let mut column_writers = column_writers.into_iter();
    let mut total_list_values = 0_usize;

    for (column_index, (field, column)) in schema.fields().iter().zip(contract.columns).enumerate()
    {
        let projected = project_column(
            expected_rows,
            contract,
            column_index,
            (column.ty == ColumnType::ListI32).then_some(column.name),
            &mut open,
        )?;
        if column.ty == ColumnType::ListI32 {
            if projected.list_values > MAX_LIST_I32_VALUES_PER_SECTION {
                return Err(CodecError::TooManyListValues {
                    name: column.name,
                    values: projected.list_values,
                    max: MAX_LIST_I32_VALUES_PER_SECTION,
                }
                .into());
            }
            total_list_values = total_list_values.checked_add(projected.list_values).ok_or(
                CodecError::TooManyListValues {
                    name: column.name,
                    values: usize::MAX,
                    max: MAX_LIST_I32_VALUES_PER_SECTION,
                },
            )?;
        }
        write_column(
            field,
            projected.arrays,
            order.as_ref(),
            &mut column_writers,
            &mut row_group,
        )?;
    }

    final_data_body_bound(type_id, rows, total_list_values)?;
    if column_writers.next().is_some() {
        return Err(CodecError::SchemaMismatch.into());
    }
    row_group.close().map_err(CodecError::from)?;
    file.close().map_err(CodecError::from)?;
    Ok(())
}

fn contract(type_id: u32) -> Result<&'static TypeContract, CodecError> {
    crate::registry()
        .iter()
        .find(|contract| contract.type_id.get() == type_id)
        .ok_or(CodecError::UnknownType { type_id })
}

fn aggregate_rows(expected_rows: &[u32]) -> Result<usize, CodecError> {
    let rows = expected_rows
        .iter()
        .try_fold(0_usize, |rows, &additional| {
            rows.checked_add(additional as usize)
                .ok_or(CodecError::TooManyRows {
                    rows: usize::MAX,
                    max: MAX_SECTION_ROWS,
                })
        })?;
    check_row_cap(rows)?;
    Ok(rows)
}

fn canonical_order<R, E>(
    expected_rows: &[u32],
    contract: &TypeContract,
    open: &mut impl FnMut(usize) -> Result<R, E>,
) -> Result<Option<UInt32Array>, E>
where
    R: ChunkReader + 'static,
    E: From<CodecError>,
{
    let rows = aggregate_rows(expected_rows)?;
    if rows <= 1 || contract.columns.is_empty() {
        return Ok(None);
    }

    let mut column_indices = Vec::with_capacity(contract.columns.len());
    for &name in contract.sort_key {
        let index = contract
            .columns
            .iter()
            .position(|column| column.name == name)
            .ok_or(CodecError::MissingColumn { name })?;
        column_indices.push(index);
    }
    for index in 0..contract.columns.len() {
        if !column_indices.contains(&index) {
            column_indices.push(index);
        }
    }

    let mut order = (0..u32::try_from(rows).map_err(|_overflow| CodecError::TooManyRows {
        rows,
        max: MAX_SECTION_ROWS,
    })?)
        .collect::<Vec<_>>();
    #[allow(
        clippy::single_range_in_vec_init,
        reason = "the first refinement pass starts with every row tied"
    )]
    let mut ties = vec![0..rows];

    for column_index in column_indices {
        if ties.is_empty() {
            break;
        }
        let column = &contract.columns[column_index];
        let projected = project_column(
            expected_rows,
            contract,
            column_index,
            (column.ty == ColumnType::ListI32).then_some(column.name),
            open,
        )?;
        let arrays = projected
            .arrays
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>();
        let values = concat(&arrays).map_err(CodecError::from)?;
        drop(arrays);
        drop(projected);
        let sort_columns = [SortColumn {
            values,
            options: None,
        }];
        let comparator =
            LexicographicalComparator::try_new(&sort_columns).map_err(CodecError::from)?;
        let mut next_ties = Vec::new();
        for range in ties {
            order[range.clone()]
                .sort_by(|left, right| comparator.compare(*left as usize, *right as usize));
            collect_ties(&order, range, &comparator, &mut next_ties);
        }
        ties = next_ties;
    }

    if order
        .iter()
        .enumerate()
        .all(|(expected, &actual)| actual as usize == expected)
    {
        Ok(None)
    } else {
        Ok(Some(UInt32Array::from(order)))
    }
}

fn collect_ties(
    order: &[u32],
    range: Range<usize>,
    comparator: &LexicographicalComparator,
    ties: &mut Vec<Range<usize>>,
) {
    let mut start = range.start;
    for position in range.start.saturating_add(1)..range.end {
        if comparator.compare(order[position - 1] as usize, order[position] as usize)
            != Ordering::Equal
        {
            if position - start > 1 {
                ties.push(start..position);
            }
            start = position;
        }
    }
    if range.end - start > 1 {
        ties.push(start..range.end);
    }
}

struct ProjectedColumn {
    arrays: Vec<ArrayRef>,
    list_values: usize,
}

fn project_column<R, E>(
    expected_rows: &[u32],
    contract: &TypeContract,
    column_index: usize,
    list_name: Option<&'static str>,
    open: &mut impl FnMut(usize) -> Result<R, E>,
) -> Result<ProjectedColumn, E>
where
    R: ChunkReader + 'static,
    E: From<CodecError>,
{
    let mut arrays = Vec::new();
    let mut rows = 0_usize;
    let mut list_values = 0_usize;
    for (section_index, &expected) in expected_rows.iter().enumerate() {
        let source = open(section_index)?;
        let projected =
            decode_projected_column(source, contract, column_index, expected as usize, list_name)?;
        rows = rows
            .checked_add(projected.rows)
            .ok_or(CodecError::TooManyRows {
                rows: usize::MAX,
                max: MAX_SECTION_ROWS,
            })?;
        list_values = list_values.checked_add(projected.list_values).ok_or(
            CodecError::TooManyListValues {
                name: list_name.unwrap_or("ListI32"),
                values: usize::MAX,
                max: MAX_LIST_I32_VALUES_PER_SECTION,
            },
        )?;
        arrays.extend(projected.arrays);
    }
    let expected = aggregate_rows(expected_rows)?;
    if rows != expected {
        return Err(CodecError::RowCountMismatch {
            expected: expected as u64,
            got: rows as u64,
        }
        .into());
    }
    Ok(ProjectedColumn {
        arrays,
        list_values,
    })
}

struct SectionColumn {
    arrays: Vec<ArrayRef>,
    rows: usize,
    list_values: usize,
}

fn decode_projected_column<R: ChunkReader + 'static>(
    source: R,
    contract: &TypeContract,
    column_index: usize,
    expected_rows: usize,
    list_name: Option<&'static str>,
) -> Result<SectionColumn, CodecError> {
    let len = usize::try_from(source.len()).map_err(|_overflow| CodecError::SectionTooLarge {
        len: usize::MAX,
        max: MAX_SECTION_BYTES,
    })?;
    if len > MAX_SECTION_BYTES {
        return Err(CodecError::SectionTooLarge {
            len,
            max: MAX_SECTION_BYTES,
        });
    }
    let options = ArrowReaderOptions::new().with_skip_arrow_metadata(true);
    let builder = ParquetRecordBatchReaderBuilder::try_new_with_options(source, options)?;
    if !schema_matches(builder.schema().as_ref(), contract) {
        return Err(CodecError::SchemaMismatch);
    }
    let groups = builder.metadata().num_row_groups();
    if groups > MAX_ROW_GROUPS {
        return Err(CodecError::TooManyRowGroups {
            groups,
            max: MAX_ROW_GROUPS,
        });
    }
    let claimed = builder.metadata().file_metadata().num_rows();
    let claimed = usize::try_from(claimed)
        .map_err(|_overflow| CodecError::InvalidRowCount { raw: claimed })?;
    check_row_cap(claimed)?;
    if claimed != expected_rows {
        return Err(CodecError::RowCountMismatch {
            expected: expected_rows as u64,
            got: claimed as u64,
        });
    }

    let mask = ProjectionMask::roots(builder.parquet_schema(), [column_index]);
    let reader = builder
        .with_projection(mask)
        .with_batch_size(DECODE_BATCH_SIZE)
        .build()?;
    let mut arrays = Vec::with_capacity(expected_rows.div_ceil(DECODE_BATCH_SIZE).max(1));
    let mut rows = 0_usize;
    let mut list_values = 0_usize;
    for batch in reader {
        let batch = batch?;
        rows = rows
            .checked_add(batch.num_rows())
            .ok_or(CodecError::TooManyRows {
                rows: usize::MAX,
                max: MAX_SECTION_ROWS,
            })?;
        if let Some(name) = list_name {
            list_values = list_values
                .checked_add(validate_list_i32_batch(&batch, name)?)
                .ok_or(CodecError::TooManyListValues {
                    name,
                    values: usize::MAX,
                    max: MAX_LIST_I32_VALUES_PER_SECTION,
                })?;
        }
        let (_schema, mut columns, _rows) = batch.into_parts();
        if columns.len() != 1 {
            return Err(CodecError::SchemaMismatch);
        }
        arrays.push(columns.pop().ok_or(CodecError::SchemaMismatch)?);
    }
    if rows != expected_rows {
        return Err(CodecError::RowCountMismatch {
            expected: expected_rows as u64,
            got: rows as u64,
        });
    }
    Ok(SectionColumn {
        arrays,
        rows,
        list_values,
    })
}

fn write_column<W: Write + Send>(
    field: &arrow_schema::FieldRef,
    arrays: Vec<ArrayRef>,
    order: Option<&UInt32Array>,
    column_writers: &mut impl Iterator<Item = ArrowColumnWriter>,
    row_group: &mut SerializedRowGroupWriter<'_, W>,
) -> Result<(), CodecError> {
    let mut parquet_writers = None;
    if let Some(order) = order {
        let mut ends = Vec::with_capacity(arrays.len());
        let mut rows = 0_usize;
        for array in &arrays {
            rows = rows
                .checked_add(array.len())
                .ok_or(CodecError::TooManyRows {
                    rows: usize::MAX,
                    max: MAX_SECTION_ROWS,
                })?;
            ends.push(rows);
        }
        let values = arrays.iter().map(AsRef::as_ref).collect::<Vec<_>>();
        for chunk in order.values().chunks(DECODE_BATCH_SIZE) {
            let mut locations = Vec::with_capacity(chunk.len());
            for &global in chunk {
                let global = global as usize;
                let source = ends.partition_point(|&end| end <= global);
                let start = if source == 0 { 0 } else { ends[source - 1] };
                if source >= arrays.len() {
                    return Err(CodecError::SchemaMismatch);
                }
                locations.push((source, global - start));
            }
            let canonical = interleave(&values, &locations)?;
            write_array(field, &canonical, column_writers, &mut parquet_writers)?;
        }
    } else {
        for array in arrays {
            write_array(field, &array, column_writers, &mut parquet_writers)?;
        }
    }

    let parquet_writers = parquet_writers.ok_or(CodecError::SchemaMismatch)?;
    for writer in parquet_writers {
        writer.close()?.append_to_row_group(row_group)?;
    }
    Ok(())
}

fn write_array(
    field: &arrow_schema::FieldRef,
    array: &ArrayRef,
    column_writers: &mut impl Iterator<Item = ArrowColumnWriter>,
    parquet_writers: &mut Option<Vec<ArrowColumnWriter>>,
) -> Result<(), CodecError> {
    let leaves = compute_leaves(field, array)?;
    if parquet_writers.is_none() {
        let writers = (0..leaves.len())
            .map(|_index| column_writers.next().ok_or(CodecError::SchemaMismatch))
            .collect::<Result<Vec<_>, _>>()?;
        *parquet_writers = Some(writers);
    }
    let writers = parquet_writers.as_mut().ok_or(CodecError::SchemaMismatch)?;
    if writers.len() != leaves.len() {
        return Err(CodecError::SchemaMismatch);
    }
    for (writer, leaf) in writers.iter_mut().zip(leaves) {
        writer.write(&leaf)?;
    }
    Ok(())
}
