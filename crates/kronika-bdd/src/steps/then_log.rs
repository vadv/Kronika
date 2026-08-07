//! Assertions against the lines an operator reads.

use super::table_rows;
use crate::BddWorld;
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::then;

/// Everything the run wrote to stdout and stderr.
fn log(world: &BddWorld) -> Result<String> {
    world.run.as_ref().context("a collector was started")?.log()
}

#[then(regex = r"^the log has a (\S+) line naming these fields$")]
fn line_with_fields(world: &mut BddWorld, action: String, step: &Step) -> Result<()> {
    let text = log(world)?;
    let line = text
        .lines()
        .find(|line| line.contains(&format!("action={action}")))
        .with_context(|| format!("no {action} line in:\n{text}"))?;
    for row in table_rows(step, &["field"])? {
        let [field] = row.as_slice() else {
            anyhow::bail!("a field row needs one cell, got {row:?}");
        };
        anyhow::ensure!(
            line.contains(&format!("{field}=")),
            "{field} is missing from {line}"
        );
    }
    Ok(())
}

#[then("the log reports these collections as degraded")]
fn degraded_collections(world: &mut BddWorld, step: &Step) -> Result<()> {
    let text = log(world)?;
    for row in table_rows(step, &["collection", "reason"])? {
        let [collection, reason] = row.as_slice() else {
            anyhow::bail!("a degradation row needs a collection and a reason, got {row:?}");
        };
        anyhow::ensure!(
            text.lines().any(|line| {
                line.contains("action=collection_degraded")
                    && line.contains(&format!("collection={collection} "))
                    && (line.contains(&format!("reason={reason}"))
                        || line.contains(&format!("reason=\"{reason}\"")))
            }),
            "no line reports {collection} degraded with reason={reason} in:\n{text}"
        );
    }
    Ok(())
}

#[then("the log reports the journal as damaged")]
fn journal_damaged(world: &mut BddWorld) -> Result<()> {
    let text = log(world)?;
    let line = text
        .lines()
        .find(|line| line.contains("action=journal_damaged"))
        .with_context(|| format!("no journal_damaged line in:\n{text}"))?;
    for field in ["path=", "reason=", "salvaged_parts="] {
        anyhow::ensure!(line.contains(field), "{field} is missing from {line}");
    }
    Ok(())
}

#[then("the log has no error line")]
fn no_error_line(world: &mut BddWorld) -> Result<()> {
    let text = log(world)?;
    let errors: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("level=error"))
        .collect();
    anyhow::ensure!(errors.is_empty(), "unexpected error lines: {errors:?}");
    Ok(())
}
