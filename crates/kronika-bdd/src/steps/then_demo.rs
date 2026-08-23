use super::table_rows;
use crate::BddWorld;
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::then;

#[then("the demo log contains these lines")]
fn demo_log_contains(world: &mut BddWorld, step: &Step) -> Result<()> {
    let text = world.demo.as_ref().context("the demo was started")?.log()?;
    for row in table_rows(step, &["line"])? {
        let [expected] = row.as_slice() else {
            anyhow::bail!("a demo log row needs one line, got {row:?}");
        };
        anyhow::ensure!(
            text.contains(expected),
            "{expected:?} is missing from:\n{text}"
        );
    }
    Ok(())
}

#[then("PostgreSQL returns these scalar values")]
fn postgres_scalars(world: &mut BddWorld, step: &Step) -> Result<()> {
    let postgres = world
        .postgres
        .as_ref()
        .context("a PostgreSQL was started")?;
    for row in table_rows(step, &["query", "value"])? {
        let [query, expected] = row.as_slice() else {
            anyhow::bail!("a scalar row needs a query and a value, got {row:?}");
        };
        let actual = postgres.scalar(query)?;
        anyhow::ensure!(
            actual == *expected,
            "query {query:?} returned {actual:?}, expected {expected:?}"
        );
    }
    Ok(())
}
