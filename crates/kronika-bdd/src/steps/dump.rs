//! Running the dumper and reading what it printed.
//!
//! Assertions reach a segment through the shipped binary rather than through
//! the reader, so a scenario checks the thing an operator runs.

use crate::BddWorld;
use anyhow::{Context as _, Result};
use std::path::PathBuf;
use std::process::Command;

/// The dumper under test, found the way the collector is.
fn binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("KRONIKA_DUMP_BIN") {
        return Ok(PathBuf::from(path));
    }
    let here = std::env::current_exe().context("locate the BDD binary")?;
    let dir = here
        .parent()
        .context("the BDD binary has no parent directory")?;
    Ok(dir.join("kronika-dump"))
}

/// Run the dumper over the run's data root and return what it printed.
pub(crate) fn dump(world: &BddWorld, flags: &[&str]) -> Result<String> {
    let run = world.run.as_ref().context("a collector was started")?;
    let root = run.out_dir();
    let output = Command::new(binary()?)
        .arg(&root)
        .args(flags)
        .output()
        .with_context(|| format!("run the dumper over {}", root.display()))?;
    anyhow::ensure!(
        output.status.success(),
        "the dumper failed over {}: {}",
        root.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// One line of `--json` output.
#[derive(Debug, Clone)]
pub(crate) struct Line(serde_json::Value);

impl Line {
    /// The value of `field` as text, or `None` when the line lacks it.
    ///
    /// A number reads as the digits it was printed with, so a scenario can
    /// compare against what a `.feature` table holds without knowing the type.
    pub(crate) fn get(&self, field: &str) -> Option<String> {
        match self.0.get(field)? {
            serde_json::Value::String(text) => Some(text.clone()),
            serde_json::Value::Null => Some("null".to_owned()),
            other => Some(other.to_string()),
        }
    }

    /// Whether the line carries `field` equal to `value`.
    pub(crate) fn holds(&self, field: &str, value: &str) -> bool {
        self.get(field).as_deref() == Some(value)
    }

    /// A numeric field.
    pub(crate) fn number(&self, field: &str) -> Option<i64> {
        self.0.get(field)?.as_i64()
    }
}

/// Parse the dumper's `--json` output, one object per line.
pub(crate) fn lines(printed: &str) -> Vec<Line> {
    printed
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .map(Line)
        .collect()
}

/// Every line whose `kind` is `kind`.
pub(crate) fn of_kind(printed: &str, kind: &str) -> Vec<Line> {
    lines(printed)
        .into_iter()
        .filter(|line| line.holds("kind", kind))
        .collect()
}
