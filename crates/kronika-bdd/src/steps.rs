//! Step definitions.
//!
//! A step carries no value of its own: every setting, threshold, cap and
//! expected list comes from the `.feature` file. What a step contributes is
//! the mechanics — spawn, wait, read the file back, compare.

mod given;
mod then_log;
mod then_segment;
mod when;

use anyhow::{Context as _, Result, bail};
use cucumber::gherkin::Step;

/// The rows of a step's table below its header, checked against `header`.
///
/// A header that drifts from what the step reads is a scenario that no longer
/// says what it asserts, so the mismatch fails the step.
fn table_rows(step: &Step, header: &[&str]) -> Result<Vec<Vec<String>>> {
    let table = step.table.as_ref().context("the step needs a table")?;
    let (first, rest) = table.rows.split_first().context("the table is empty")?;
    let trimmed: Vec<&str> = first.iter().map(|cell| cell.trim()).collect();
    if trimmed != header {
        bail!("table header is {trimmed:?}, the step reads {header:?}");
    }
    Ok(rest
        .iter()
        .map(|row| row.iter().map(|cell| cell.trim().to_owned()).collect())
        .collect())
}

/// A `type_id` cell.
fn type_id(cell: &str) -> Result<u32> {
    cell.parse()
        .with_context(|| format!("{cell:?} is not a type_id"))
}

/// A count cell.
fn count(cell: &str) -> Result<u32> {
    cell.parse()
        .with_context(|| format!("{cell:?} is not a count"))
}
